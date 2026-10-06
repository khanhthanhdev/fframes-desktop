//! Allowlisted CSS custom-property importer.
//!
//! CSS is only an exchange format here. The importer reads `--custom-property: value;`
//! declarations from a `:root { ... }` block or from a bare declaration list, maps a fixed set of
//! variable names to canonical tokens and normalizes a small set of literal forms. It never
//! evaluates selectors, inheritance, `var()`, `calc()`, `url()`, gradients or any other function.
//!
//! # Variable mapping (the complete allowlist)
//!
//! | CSS variable                              | Token                       | Accepted value                                   |
//! |-------------------------------------------|-----------------------------|--------------------------------------------------|
//! | `--color-<n>`                             | `color.<n>`                 | hex `#rgb`/`#rgba`/`#rrggbb`/`#rrggbbaa`         |
//! | `--spacing-<n>` `--radius-<n>` `--stroke-<n>` | `spacing.<n>` etc.      | unitless (px), `px`, `rem`                       |
//! | `--shadow-<n>`                            | `shadow.<n>`                | `<dx> <dy> [<blur>] <hex color>` (px/rem/unitless)|
//! | `--motion-duration-<n>` `--motion-stagger-<n>` | `motion.duration.<n>` / `motion.stagger.<n>` | `<n>s`, `<n>ms` |
//! | `--motion-easing-<n>`                     | `motion.easing.<n>`         | `linear`, `ease-in`, `ease-out`, `ease-in-out`, `cubic-bezier(a, b, c, d)` |
//! | `--typography-<n>-family`                 | `typography.<n>` family     | one quoted family string (no fallback list)      |
//! | `--typography-<n>-size`                   | size (px)                   | unitless (px), `px`, `rem`                       |
//! | `--typography-<n>-weight`                 | weight                      | integer `1..=1000`                               |
//! | `--typography-<n>-line-height`            | line height multiple        | unitless number                                  |
//! | `--typography-<n>-letter-spacing`         | letter spacing (px)         | unitless (px), `px`, `rem`                       |
//!
//! A typography token needs all five `--typography-<n>-*` fields. `rem` uses the explicit
//! [`Design::base_font_px`]; unitless lengths are video pixels. Every declaration yields exactly
//! one [`CssEntry`].

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::{
    model::{
        Code, Color, Design, Diagnostic, Easing, MAX_NAME_LEN, MAX_TOKENS, Shadow, TokenDef,
        TokenName, TokenValue, Typography, canon, parse_duration_str, parse_length_str,
    },
    resolve::TokenSet,
};

pub const MAX_CSS_BYTES: usize = 256 * 1024;
pub const MAX_CSS_ENTRIES: usize = 2048;
pub const MAX_CSS_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Mapped and normalized into a token.
    Accepted,
    /// Valid CSS (or a valid allowlisted name) outside the supported subset; nothing was guessed.
    Unsupported,
    /// In the subset but malformed, out of bounds, duplicated or incomplete.
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CssEntry {
    /// 1-based source line of the declaration (or block) start.
    pub line: usize,
    /// Enclosing selector or at-rule prelude; empty for `:root` and bare declarations.
    pub selector: String,
    pub property: String,
    pub value: String,
    /// Canonical token this declaration maps to, when the variable is allowlisted.
    pub token: Option<String>,
    /// Normalized result for accepted declarations.
    pub normalized: Option<String>,
    pub status: Status,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct CssReport {
    pub entries: Vec<CssEntry>,
    tokens: TokenSet,
}
impl CssReport {
    /// Tokens produced by accepted declarations only.
    pub fn tokens(&self) -> &TokenSet {
        &self.tokens
    }
    pub fn count(&self, status: Status) -> usize {
        self.entries.iter().filter(|e| e.status == status).count()
    }
    /// Human-readable, line-oriented report.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for e in &self.entries {
            let status = match e.status {
                Status::Accepted => "accepted",
                Status::Unsupported => "unsupported",
                Status::Rejected => "rejected",
            };
            out.push_str(&format!("line {}: {} `{}`", e.line, status, e.property));
            if let Some(t) = &e.token {
                out.push_str(&format!(" -> {t}"));
            }
            if let Some(n) = &e.normalized {
                out.push_str(&format!(" = {n}"));
            }
            if !e.reason.is_empty() {
                out.push_str(&format!(" ({})", e.reason));
            }
            out.push('\n');
        }
        out
    }
}

enum Ctx {
    /// Top level of the document (bare declarations are read here).
    Top,
    Root,
    Other(String),
}

enum Field {
    Family(String),
    Size(f64),
    Weight(u16),
    LineHeight(f64),
    LetterSpacing(f64),
}

#[derive(Default)]
struct Group {
    fields: BTreeMap<&'static str, (usize, Field)>,
    failed: Vec<String>,
}

enum Outcome {
    Token(TokenValue, String),
    Part(&'static str, Field, String),
    Unsupported(String),
    Rejected(String),
}

struct Mapped {
    token: String,
    kind: MapKind,
}
enum MapKind {
    Color,
    Dimension,
    Shadow,
    Duration,
    Easing,
    /// typography group name + field
    Typo(&'static str),
}

const TYPO_FIELDS: [&str; 5] = ["family", "size", "weight", "line-height", "letter-spacing"];

fn map_variable(property: &str) -> Option<Mapped> {
    let rest = property.strip_prefix("--")?;
    let simple: [(&str, &str, MapKind); 8] = [
        ("color-", "color.", MapKind::Color),
        ("spacing-", "spacing.", MapKind::Dimension),
        ("radius-", "radius.", MapKind::Dimension),
        ("stroke-", "stroke.", MapKind::Dimension),
        ("shadow-", "shadow.", MapKind::Shadow),
        ("motion-duration-", "motion.duration.", MapKind::Duration),
        ("motion-stagger-", "motion.stagger.", MapKind::Duration),
        ("motion-easing-", "motion.easing.", MapKind::Easing),
    ];
    for (prefix, token, kind) in simple {
        if let Some(name) = rest.strip_prefix(prefix) {
            return Some(Mapped {
                token: format!("{token}{name}"),
                kind,
            });
        }
    }
    let rest = rest.strip_prefix("typography-")?;
    for field in TYPO_FIELDS {
        if let Some(name) = rest
            .strip_suffix(field)
            .and_then(|n| n.strip_suffix('-'))
            .filter(|n| !n.is_empty())
        {
            return Some(Mapped {
                token: format!("typography.{name}"),
                kind: MapKind::Typo(field),
            });
        }
    }
    None
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    design: &'a Design,
    entries: Vec<CssEntry>,
    seen: HashSet<String>,
    defs: BTreeMap<TokenName, TokenDef>,
    groups: BTreeMap<String, Group>,
    full: bool,
}

/// Import `source` (a `:root { ... }` block or bare declarations) under an explicit design context.
pub fn import_css(source: &str, design: &Design) -> CssReport {
    let mut parser = Parser {
        chars: Vec::new(),
        pos: 0,
        line: 1,
        design,
        entries: Vec::new(),
        seen: HashSet::new(),
        defs: BTreeMap::new(),
        groups: BTreeMap::new(),
        full: false,
    };
    if let Err(d) = design.validate("design") {
        parser.push(1, "", "", "", None, Status::Rejected, d.message);
    } else if source.len() > MAX_CSS_BYTES {
        parser.push(
            1,
            "",
            "",
            "",
            None,
            Status::Rejected,
            format!("input exceeds {MAX_CSS_BYTES} bytes; nothing was read"),
        );
    } else {
        parser.chars = strip_comments(source, &mut parser);
        parser.block(&Ctx::Top, 0);
        parser.finish_groups();
    }
    let tokens = TokenSet::from_defs(parser.defs).expect("literal-only token set is valid");
    CssReport {
        entries: parser.entries,
        tokens,
    }
}

/// Comments become spaces (newlines kept) so line numbers stay exact.
fn strip_comments(source: &str, parser: &mut Parser<'_>) -> Vec<char> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    let mut line = 1;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q || c == '\n' {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            let start = line;
            i += 2;
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    i += 2;
                    closed = true;
                    break;
                }
                if chars[i] == '\n' {
                    out.push('\n');
                    line += 1;
                }
                i += 1;
            }
            out.push(' ');
            if !closed {
                parser.push(
                    start,
                    "",
                    "/*",
                    "",
                    None,
                    Status::Rejected,
                    "unterminated comment; the rest of the input was ignored",
                );
            }
            continue;
        }
        if c == '\n' {
            line += 1;
        }
        out.push(c);
        i += 1;
    }
    out
}

impl Parser<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        line: usize,
        selector: &str,
        property: &str,
        value: &str,
        token: Option<String>,
        status: Status,
        reason: impl Into<String>,
    ) -> usize {
        if self.entries.len() >= MAX_CSS_ENTRIES {
            if !self.full {
                self.full = true;
                self.entries.push(CssEntry {
                    line,
                    selector: String::new(),
                    property: String::new(),
                    value: String::new(),
                    token: None,
                    normalized: None,
                    status: Status::Rejected,
                    reason: format!(
                        "more than {MAX_CSS_ENTRIES} declarations; the rest were not reported"
                    ),
                });
            }
            return self.entries.len() - 1;
        }
        self.entries.push(CssEntry {
            line,
            selector: selector.into(),
            property: property.into(),
            value: value.into(),
            token,
            normalized: None,
            status,
            reason: reason.into(),
        });
        self.entries.len() - 1
    }

    /// Reads items until the matching `}` (or EOF at depth 0).
    fn block(&mut self, ctx: &Ctx, depth: usize) {
        let mut cur = String::new();
        let mut start = self.line;
        let mut paren = 0usize;
        let mut quote: Option<char> = None;
        while self.pos < self.chars.len() {
            let c = self.chars[self.pos];
            self.pos += 1;
            if c == '\n' {
                self.line += 1;
            }
            if let Some(q) = quote {
                cur.push(c);
                if c == q || c == '\n' {
                    quote = None;
                }
                continue;
            }
            match c {
                '"' | '\'' => {
                    quote = Some(c);
                    if cur.trim().is_empty() {
                        start = self.line;
                    }
                    cur.push(c);
                }
                '(' => {
                    paren += 1;
                    cur.push(c);
                }
                ')' => {
                    paren = paren.saturating_sub(1);
                    cur.push(c);
                }
                ';' if paren == 0 => {
                    self.declaration(ctx, start, &std::mem::take(&mut cur));
                    start = self.line;
                }
                '{' if paren == 0 => {
                    let prelude = std::mem::take(&mut cur).trim().to_owned();
                    let block_line = start;
                    let child = if depth == 0 && prelude == ":root" && matches!(ctx, Ctx::Top) {
                        Ctx::Root
                    } else {
                        let label = if let Ctx::Other(parent) = ctx {
                            format!("{parent} {prelude}")
                        } else {
                            prelude.clone()
                        };
                        self.push(
                            block_line,
                            "",
                            &prelude,
                            "{...}",
                            None,
                            Status::Unsupported,
                            "selector and at-rule blocks are not interpreted; only a `:root` block is read",
                        );
                        Ctx::Other(label)
                    };
                    if depth + 1 > MAX_CSS_DEPTH {
                        self.push(
                            block_line,
                            "",
                            &prelude,
                            "{...}",
                            None,
                            Status::Rejected,
                            format!("blocks nested deeper than {MAX_CSS_DEPTH}; contents skipped"),
                        );
                        self.skip_block();
                    } else {
                        self.block(&child, depth + 1);
                    }
                    start = self.line;
                }
                '}' if paren == 0 => {
                    if !cur.trim().is_empty() {
                        self.declaration(ctx, start, &std::mem::take(&mut cur));
                    }
                    if depth == 0 {
                        self.push(
                            self.line,
                            "",
                            "}",
                            "",
                            None,
                            Status::Rejected,
                            "unbalanced `}`",
                        );
                        start = self.line;
                        continue;
                    }
                    return;
                }
                c => {
                    if cur.trim().is_empty() && !c.is_whitespace() {
                        start = self.line;
                    }
                    cur.push(c);
                }
            }
        }
        if !cur.trim().is_empty() {
            self.declaration(ctx, start, &cur);
        }
        if depth > 0 {
            let selector = if let Ctx::Other(s) = ctx {
                s.clone()
            } else {
                ":root".into()
            };
            self.push(
                self.line,
                "",
                &selector,
                "",
                None,
                Status::Rejected,
                "unterminated block (missing `}`)",
            );
        }
    }

    fn skip_block(&mut self) {
        let mut depth = 1usize;
        while self.pos < self.chars.len() && depth > 0 {
            match self.chars[self.pos] {
                '{' => depth += 1,
                '}' => depth -= 1,
                '\n' => self.line += 1,
                _ => {}
            }
            self.pos += 1;
        }
    }

    fn declaration(&mut self, ctx: &Ctx, line: usize, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let Some((property, value)) = text.split_once(':') else {
            self.push(
                line,
                "",
                text,
                "",
                None,
                Status::Rejected,
                "not a declaration (expected `property: value`)",
            );
            return;
        };
        let (property, value) = (property.trim(), value.trim());
        let selector = match ctx {
            Ctx::Other(s) => s.as_str(),
            _ => "",
        };
        let mapped = map_variable(property);
        let token = mapped.as_ref().map(|m| m.token.clone());
        // A typography field that is not accepted keeps its whole group from forming.
        let typo_group = match &mapped {
            Some(Mapped {
                kind: MapKind::Typo(_),
                token,
            }) if !matches!(ctx, Ctx::Other(_)) => Some(token.clone()),
            _ => None,
        };
        let report = |this: &mut Self, status, reason: String, mark: bool| {
            if let (true, Some(group)) = (mark, &typo_group) {
                let failed = &mut this.groups.entry(group.clone()).or_default().failed;
                if !failed.iter().any(|p| p == property) {
                    failed.push(property.to_owned());
                }
            }
            this.push(
                line,
                selector,
                property,
                value,
                token.clone(),
                status,
                reason,
            );
        };
        if let Ctx::Other(sel) = ctx {
            report(
                self,
                Status::Unsupported,
                format!("inside `{sel}`: only `:root` custom properties are imported"),
                false,
            );
            return;
        }
        if !property.starts_with("--") {
            report(
                self,
                Status::Unsupported,
                format!(
                    "ordinary CSS property `{property}` is not interpreted; only `--` custom properties are mapped"
                ),
                false,
            );
            return;
        }
        let lower = value.to_ascii_lowercase();
        if let Some(i) = lower.rfind('!')
            && lower[i + 1..].trim() == "important"
        {
            report(
                self,
                Status::Unsupported,
                "`!important` is cascade behavior and is not evaluated".into(),
                true,
            );
            return;
        }
        let Some(mapped) = mapped else {
            report(
                self,
                Status::Unsupported,
                format!("`{property}` is not in the documented variable mapping"),
                false,
            );
            return;
        };
        if !self.seen.insert(property.to_owned()) {
            report(
                self,
                Status::Rejected,
                format!(
                    "duplicate declaration of `{property}`; cascade order is not evaluated, the first one was kept"
                ),
                false,
            );
            return;
        }
        if value.is_empty() {
            report(self, Status::Rejected, "empty value".into(), true);
            return;
        }
        if mapped.token.len() > MAX_NAME_LEN || TokenName::new(mapped.token.clone()).is_err() {
            report(
                self,
                Status::Rejected,
                format!(
                    "`{}` is not a valid token name (lowercase [a-z0-9_-] segments of at most 32 characters)",
                    mapped.token
                ),
                true,
            );
            return;
        }
        if let Some(reason) = function_reason(value, matches!(mapped.kind, MapKind::Easing)) {
            report(self, Status::Unsupported, reason, true);
            return;
        }
        match self.parse_value(&mapped, value) {
            Outcome::Unsupported(r) => report(self, Status::Unsupported, r, true),
            Outcome::Rejected(r) => report(self, Status::Rejected, r, true),
            Outcome::Token(..) | Outcome::Part(..)
                if self.defs.len() + self.groups.len() >= MAX_TOKENS =>
            {
                report(
                    self,
                    Status::Rejected,
                    format!("more than {MAX_TOKENS} tokens; this declaration was not imported"),
                    false,
                );
            }
            Outcome::Token(v, norm) => {
                let name = TokenName::new(mapped.token.clone()).expect("checked");
                self.defs.insert(name, TokenDef::Literal(v));
                let i = self.push(
                    line,
                    selector,
                    property,
                    value,
                    token.clone(),
                    Status::Accepted,
                    "",
                );
                self.entries[i].normalized = Some(norm);
            }
            Outcome::Part(field, part, norm) => {
                let i = self.push(
                    line,
                    selector,
                    property,
                    value,
                    token.clone(),
                    Status::Accepted,
                    "",
                );
                self.entries[i].normalized = Some(norm);
                self.groups
                    .entry(mapped.token)
                    .or_default()
                    .fields
                    .insert(field, (i, part));
            }
        }
    }

    fn parse_value(&self, mapped: &Mapped, value: &str) -> Outcome {
        let design = self.design;
        match &mapped.kind {
            MapKind::Color => match parse_color(value) {
                Ok(c) => Outcome::Token(TokenValue::Color(c), c.to_hex()),
                Err(o) => o,
            },
            MapKind::Dimension => match length(value, design, 0.0, 100_000.0) {
                Ok(px) => Outcome::Token(TokenValue::Dimension(px), fmt_num(px)),
                Err(o) => o,
            },
            MapKind::Duration => match parse_duration_str(value, "value") {
                Ok(s) if (0.0..=3600.0).contains(&s) => {
                    let s = canon(s);
                    Outcome::Token(TokenValue::Duration(s), format!("{}s", fmt_num(s)))
                }
                Ok(_) => Outcome::Rejected("duration must be within 0..=3600 seconds".into()),
                Err(d) => diag_outcome(&d),
            },
            MapKind::Easing => parse_easing(value),
            MapKind::Shadow => parse_shadow(value, design),
            MapKind::Typo(field) => parse_typo(field, value, design),
        }
    }

    /// Build each complete typography group; reject accepted fields of incomplete ones.
    fn finish_groups(&mut self) {
        let groups = std::mem::take(&mut self.groups);
        for (token, mut group) in groups {
            let missing: Vec<&str> = TYPO_FIELDS
                .iter()
                .copied()
                .filter(|f| !group.fields.contains_key(f))
                .collect();
            if missing.is_empty() && group.failed.is_empty() {
                let take = |g: &mut Group, k: &str| g.fields.remove(k).map(|(_, f)| f);
                if let (
                    Some(Field::Family(family)),
                    Some(Field::Size(size)),
                    Some(Field::Weight(weight)),
                    Some(Field::LineHeight(line_height)),
                    Some(Field::LetterSpacing(letter_spacing)),
                ) = (
                    take(&mut group, "family"),
                    take(&mut group, "size"),
                    take(&mut group, "weight"),
                    take(&mut group, "line-height"),
                    take(&mut group, "letter-spacing"),
                ) {
                    let name = TokenName::new(token.clone()).expect("checked");
                    self.defs.insert(
                        name,
                        TokenDef::Literal(TokenValue::Typography(Typography {
                            family,
                            size,
                            weight,
                            line_height,
                            letter_spacing,
                        })),
                    );
                }
                continue;
            }
            let reason = if !group.failed.is_empty() {
                format!(
                    "`{token}` cannot be built: {} was not accepted",
                    group.failed.join(", ")
                )
            } else {
                let vars: Vec<String> = missing
                    .iter()
                    .map(|f| format!("--{}-{f}", token.replacen("typography.", "typography-", 1)))
                    .collect();
                format!("`{token}` is incomplete: missing {}", vars.join(", "))
            };
            for (i, _) in group.fields.into_values() {
                self.entries[i].status = Status::Rejected;
                self.entries[i].reason = reason.clone();
            }
        }
    }
}

fn diag_outcome(d: &Diagnostic) -> Outcome {
    if d.code == Code::UnsupportedUnit {
        Outcome::Unsupported(d.message.clone())
    } else {
        Outcome::Rejected(d.message.clone())
    }
}

fn fmt_num(v: f64) -> String {
    let v = canon(v);
    if v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

/// Names the first function-like construct; `cubic-bezier(` is allowed for easing values.
fn function_reason(value: &str, easing: bool) -> Option<String> {
    let bytes = value.as_bytes();
    let mut in_quote: Option<u8> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if let Some(q) = in_quote {
            if b == q {
                in_quote = None;
            }
        } else if b == b'"' || b == b'\'' {
            in_quote = Some(b);
        } else if b == b'(' {
            let name: String = value[..i]
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if easing && name == "cubic-bezier" {
                return None;
            }
            let shown = if name.is_empty() {
                "(".to_owned()
            } else {
                format!("{name}()")
            };
            return Some(format!(
                "CSS function `{shown}` (var/calc/url/gradients/colors/...) is not evaluated"
            ));
        }
    }
    None
}

fn bounded(px: f64, min: f64, max: f64) -> Result<f64, Outcome> {
    if (min..=max).contains(&px) {
        Ok(canon(px))
    } else {
        Err(Outcome::Rejected(format!("{px} is outside {min}..={max}")))
    }
}

/// Unitless numbers are video pixels; `px`/`rem` use the explicit design context.
fn length(value: &str, design: &Design, min: f64, max: f64) -> Result<f64, Outcome> {
    if value.split_whitespace().count() != 1 {
        return Err(Outcome::Rejected("expected a single length".into()));
    }
    let px = if value
        .bytes()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
        && value.parse::<f64>().is_ok()
    {
        let n: f64 = value.parse().expect("checked");
        if !n.is_finite() {
            return Err(Outcome::Rejected("value must be finite".into()));
        }
        n
    } else {
        parse_length_str(value, design, "value").map_err(|d| diag_outcome(&d))?
    };
    bounded(px, min, max)
}

fn parse_color(value: &str) -> Result<Color, Outcome> {
    if value.starts_with('#') {
        Color::parse_hex(value).ok_or_else(|| {
            Outcome::Rejected(format!(
                "`{value}` is not a valid #rgb/#rgba/#rrggbb/#rrggbbaa color"
            ))
        })
    } else {
        Err(Outcome::Unsupported(
            "only hex color literals are supported (no named colors or color functions)".into(),
        ))
    }
}

fn parse_easing(value: &str) -> Outcome {
    let easing = match value {
        "linear" => Easing::Linear,
        "ease-in" => Easing::EaseIn,
        "ease-out" => Easing::EaseOut,
        "ease-in-out" => Easing::EaseInOut,
        v if v.starts_with("cubic-bezier(") && v.ends_with(')') => {
            let inner = &v["cubic-bezier(".len()..v.len() - 1];
            let nums: Result<Vec<f64>, _> =
                inner.split(',').map(|p| p.trim().parse::<f64>()).collect();
            match nums {
                Ok(n) if n.len() == 4 && n.iter().all(|x| x.is_finite()) => {
                    if !(0.0..=1.0).contains(&n[0]) || !(0.0..=1.0).contains(&n[2]) {
                        return Outcome::Rejected("cubic-bezier x1/x2 must be within 0..=1".into());
                    }
                    if !(-4.0..=4.0).contains(&n[1]) || !(-4.0..=4.0).contains(&n[3]) {
                        return Outcome::Rejected(
                            "cubic-bezier y1/y2 must be within -4..=4".into(),
                        );
                    }
                    Easing::CubicBezier {
                        x1: canon(n[0]),
                        y1: canon(n[1]),
                        x2: canon(n[2]),
                        y2: canon(n[3]),
                    }
                }
                _ => return Outcome::Rejected("cubic-bezier() needs four finite numbers".into()),
            }
        }
        other => {
            return Outcome::Unsupported(format!(
                "easing `{other}` is not supported; use linear, ease-in, ease-out, ease-in-out or cubic-bezier(a, b, c, d)"
            ));
        }
    };
    let norm = serde_json::to_string(&easing).expect("easing serializes");
    Outcome::Token(TokenValue::Easing(easing), norm)
}

fn parse_shadow(value: &str, design: &Design) -> Outcome {
    if value.contains(',') {
        return Outcome::Unsupported("multiple shadows are not supported".into());
    }
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.iter().any(|p| p.eq_ignore_ascii_case("inset")) {
        return Outcome::Unsupported("inset shadows are not supported".into());
    }
    let (lengths, color) = match parts.split_last() {
        Some((last, rest)) if last.starts_with('#') => (rest, *last),
        _ => {
            return Outcome::Unsupported("shadow must be `<dx> <dy> [<blur>] <hex color>`".into());
        }
    };
    if !(2..=3).contains(&lengths.len()) {
        return Outcome::Unsupported(
            "spread radius and other forms are not supported; use `<dx> <dy> [<blur>] <hex color>`"
                .into(),
        );
    }
    let color = match parse_color(color) {
        Ok(c) => c,
        Err(o) => return o,
    };
    let mut px = [0.0; 3];
    for (i, l) in lengths.iter().enumerate() {
        let min = if i == 2 { 0.0 } else { -10_000.0 };
        match length(l, design, min, 10_000.0) {
            Ok(v) => px[i] = v,
            Err(o) => return o,
        }
    }
    let shadow = Shadow {
        dx: px[0],
        dy: px[1],
        blur: px[2],
        color,
    };
    let norm = format!(
        "{} {} {} {}",
        fmt_num(px[0]),
        fmt_num(px[1]),
        fmt_num(px[2]),
        color.to_hex()
    );
    Outcome::Token(TokenValue::Shadow(shadow), norm)
}

fn parse_typo(field: &'static str, value: &str, design: &Design) -> Outcome {
    match field {
        "family" => {
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'');
            let Some(q) = quote else {
                return Outcome::Rejected("font family must be one quoted string".into());
            };
            let inner = &value[1..];
            let Some(end) = inner.find(q) else {
                return Outcome::Rejected("unterminated family string".into());
            };
            if !inner[end + q.len_utf8()..].trim().is_empty() {
                return Outcome::Unsupported(
                    "font fallback lists are not supported; system fonts are never substituted"
                        .into(),
                );
            }
            let family = &inner[..end];
            if family.trim().is_empty()
                || family.contains('\\')
                || family.chars().any(char::is_control)
                || family.len() > 256
            {
                return Outcome::Rejected(
                    "family must be a plain non-empty string without escapes".into(),
                );
            }
            Outcome::Part(
                "family",
                Field::Family(family.to_owned()),
                format!("{family:?}"),
            )
        }
        "size" => match length(value, design, 1.0, 4096.0) {
            Ok(v) => Outcome::Part("size", Field::Size(v), fmt_num(v)),
            Err(o) => o,
        },
        "letter-spacing" => match length(value, design, -256.0, 256.0) {
            Ok(v) => Outcome::Part("letter-spacing", Field::LetterSpacing(v), fmt_num(v)),
            Err(o) => o,
        },
        "weight" => match value.parse::<u16>() {
            Ok(w) if (1..=1000).contains(&w) => {
                Outcome::Part("weight", Field::Weight(w), w.to_string())
            }
            Ok(_) => Outcome::Rejected("weight must be an integer in 1..=1000".into()),
            Err(_) if value.bytes().all(|c| c.is_ascii_alphabetic()) => Outcome::Unsupported(
                "weight keywords (bold, normal, ...) are not mapped; use a number".into(),
            ),
            Err(_) => Outcome::Rejected("weight must be an integer in 1..=1000".into()),
        },
        _ => match value.parse::<f64>() {
            Ok(v) if v.is_finite() => match bounded(v, 0.1, 10.0) {
                Ok(v) => Outcome::Part("line-height", Field::LineHeight(v), fmt_num(v)),
                Err(o) => o,
            },
            _ if value
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
                .is_empty() =>
            {
                Outcome::Rejected("line-height must be a finite number".into())
            }
            _ => Outcome::Unsupported(
                "line-height must be a unitless multiple (units and `normal` are not mapped)"
                    .into(),
            ),
        },
    }
}
