//! Plain data model: diagnostics, bounds, token names and typed token values.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Preset package schema version understood by this crate.
pub const PRESET_SCHEMA: u32 = 1;
/// Token file schema version understood by this crate (authored, resolved and override files).
pub const TOKEN_SCHEMA: u32 = 1;

pub const MAX_TOKENS: usize = 512;
pub const MAX_OVERRIDE_SCENES: usize = 128;
pub const MAX_NAME_LEN: usize = 96;
pub const MAX_SEGMENTS: usize = 6;
pub const MAX_SEGMENT_LEN: usize = 32;
pub const MAX_STRING_LEN: usize = 256;
pub const MAX_JSON_DEPTH: usize = 12;
pub const MAX_JSON_BYTES: usize = 256 * 1024;
pub const MAX_DIAGNOSTICS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// Stable machine-readable diagnostic classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    UnsupportedSchema,
    Malformed,
    TooLarge,
    TooDeep,
    InvalidName,
    InvalidType,
    TypeMismatch,
    InvalidValue,
    NonFinite,
    UnsupportedUnit,
    DuplicateName,
    MissingReference,
    Cycle,
    CrossTypeAlias,
    OrphanedOverride,
    InvalidOverride,
    InvalidPath,
    DuplicatePath,
    UnsafeFile,
    HashMismatch,
    MissingResource,
    UndeclaredFile,
    UnsupportedMedia,
    FontMismatch,
    MissingLicense,
    InvalidLicense,
    AlreadyExists,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Code,
    /// Dotted path of the offending field, e.g. `tokens.typography.title.value.size`.
    pub field: String,
    pub message: String,
}
impl Diagnostic {
    pub fn error(code: Code, field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            field: field.into(),
            message: message.into(),
        }
    }
    pub fn warning(code: Code, field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, field, message)
        }
    }
}
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?}: {}", self.field, self.code, self.message)
    }
}

/// One or more field-specific failures. Never partial: callers receive either a value or this.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct PresetError {
    pub diagnostics: Vec<Diagnostic>,
}
impl PresetError {
    pub fn new(mut diagnostics: Vec<Diagnostic>) -> Self {
        diagnostics.truncate(MAX_DIAGNOSTICS);
        Self { diagnostics }
    }
    pub fn one(code: Code, field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(vec![Diagnostic::error(code, field, message)])
    }
    pub fn has(&self, code: Code) -> bool {
        self.diagnostics.iter().any(|d| d.code == code)
    }
}
impl From<Diagnostic> for PresetError {
    fn from(value: Diagnostic) -> Self {
        Self::new(vec![value])
    }
}
impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, d) in self.diagnostics.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{d}")?;
        }
        Ok(())
    }
}

/// Design context needed to convert `rem` and to bound values. Always explicit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Design {
    pub width: u32,
    pub height: u32,
    /// Pixels in one `rem`, in video coordinates.
    pub base_font_px: f64,
}
impl Design {
    pub const HD: Design = Design {
        width: 1920,
        height: 1080,
        base_font_px: 16.0,
    };
    pub fn validate(&self, field: &str) -> Result<(), Diagnostic> {
        if !(1..=16384).contains(&self.width) || !(1..=16384).contains(&self.height) {
            return Err(Diagnostic::error(
                Code::InvalidValue,
                format!("{field}.width"),
                "design width/height must be 1..=16384 pixels",
            ));
        }
        if !self.base_font_px.is_finite() || !(1.0..=256.0).contains(&self.base_font_px) {
            return Err(Diagnostic::error(
                Code::InvalidValue,
                format!("{field}.base_font_px"),
                "base_font_px must be a finite value in 1..=256",
            ));
        }
        Ok(())
    }
}

/// Rounds to micro-units and removes negative zero so equal values serialize identically.
pub fn canon(value: f64) -> f64 {
    let rounded = (value * 1e6).round() / 1e6;
    if rounded == 0.0 { 0.0 } else { rounded }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenKind {
    Color,
    Typography,
    Dimension,
    Duration,
    Easing,
    Shadow,
}
impl TokenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Color => "color",
            Self::Typography => "typography",
            Self::Dimension => "dimension",
            Self::Duration => "duration",
            Self::Easing => "easing",
            Self::Shadow => "shadow",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "color" => Self::Color,
            "typography" => Self::Typography,
            "dimension" => Self::Dimension,
            "duration" => Self::Duration,
            "easing" => Self::Easing,
            "shadow" => Self::Shadow,
            _ => return None,
        })
    }
}

/// Semantic dotted token path such as `color.accent` or `motion.duration.fast`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TokenName(String);
impl TokenName {
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let bad = |why: &str| {
            Diagnostic::error(
                Code::InvalidName,
                format!("tokens.{value}"),
                format!("invalid token name: {why}"),
            )
        };
        if value.is_empty() || value.len() > MAX_NAME_LEN {
            return Err(bad("length must be 1..=96"));
        }
        let segments: Vec<&str> = value.split('.').collect();
        if segments.len() < 2 || segments.len() > MAX_SEGMENTS {
            return Err(bad("expected 2..=6 dot-separated segments"));
        }
        for s in &segments {
            let mut chars = s.bytes();
            let ok = !s.is_empty()
                && s.len() <= MAX_SEGMENT_LEN
                && chars
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && chars.all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_'
                });
            if !ok {
                return Err(bad("segments are lowercase [a-z0-9][a-z0-9_-]{0,31}"));
            }
        }
        family_kind_of(&segments)
            .ok_or_else(|| bad("unknown family; use color, typography, spacing, radius, stroke, shadow, motion.duration, motion.easing or motion.stagger"))?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The only value type this name may carry; derived from its family.
    pub fn kind(&self) -> TokenKind {
        let segments: Vec<&str> = self.0.split('.').collect();
        family_kind_of(&segments).expect("validated at construction")
    }
}
fn family_kind_of(segments: &[&str]) -> Option<TokenKind> {
    Some(match segments {
        ["color", ..] => TokenKind::Color,
        ["typography", ..] => TokenKind::Typography,
        ["spacing" | "radius" | "stroke", ..] => TokenKind::Dimension,
        ["shadow", ..] => TokenKind::Shadow,
        ["motion", "duration" | "stagger", _, ..] => TokenKind::Duration,
        ["motion", "easing", _, ..] => TokenKind::Easing,
        _ => return None,
    })
}
impl TryFrom<String> for TokenName {
    type Error = Diagnostic;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<TokenName> for String {
    fn from(value: TokenName) -> Self {
        value.0
    }
}
impl fmt::Display for TokenName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}
impl Color {
    /// Accepts `#rgb`, `#rgba`, `#rrggbb` and `#rrggbbaa` (case-insensitive); short forms expand exactly.
    pub fn parse_hex(value: &str) -> Option<Self> {
        let hex = value.strip_prefix('#')?;
        if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let nib = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok();
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            3 | 4 => Some(Self {
                r: nib(0)? * 17,
                g: nib(1)? * 17,
                b: nib(2)? * 17,
                a: if hex.len() == 4 { nib(3)? * 17 } else { 255 },
            }),
            6 | 8 => Some(Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: if hex.len() == 8 { byte(6)? } else { 255 },
            }),
            _ => None,
        }
    }
    /// Canonical uppercase `#RRGGBB`, or `#RRGGBBAA` when not opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
        }
    }
}
impl TryFrom<String> for Color {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse_hex(&value).ok_or_else(|| format!("invalid color `{value}`"))
    }
}
impl From<Color> for String {
    fn from(value: Color) -> Self {
        value.to_hex()
    }
}
impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}
impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::try_from(text).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Typography {
    pub family: String,
    /// Pixels in video coordinates.
    pub size: f64,
    pub weight: u16,
    /// Unitless multiple of `size`.
    pub line_height: f64,
    /// Pixels in video coordinates.
    pub letter_spacing: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    CubicBezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    Spring {
        mass: f64,
        stiffness: f64,
        damping: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shadow {
    pub dx: f64,
    pub dy: f64,
    pub blur: f64,
    pub color: Color,
}

/// Fully-resolved typed value; serializes as the runtime `{"type","value"}` token entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum TokenValue {
    Color(Color),
    Typography(Typography),
    /// Pixels in video coordinates.
    Dimension(f64),
    /// Seconds.
    Duration(f64),
    Easing(Easing),
    Shadow(Shadow),
}
impl TokenValue {
    pub fn kind(&self) -> TokenKind {
        match self {
            Self::Color(_) => TokenKind::Color,
            Self::Typography(_) => TokenKind::Typography,
            Self::Dimension(_) => TokenKind::Dimension,
            Self::Duration(_) => TokenKind::Duration,
            Self::Easing(_) => TokenKind::Easing,
            Self::Shadow(_) => TokenKind::Shadow,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AliasDef {
    #[serde(rename = "type")]
    pub kind: TokenKind,
    pub alias: TokenName,
}

/// An authored token: a typed literal or an alias of another token of the same type.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TokenDef {
    Literal(TokenValue),
    Alias(AliasDef),
}
impl TokenDef {
    pub fn kind(&self) -> TokenKind {
        match self {
            Self::Literal(v) => v.kind(),
            Self::Alias(a) => a.kind,
        }
    }
}

// ---------------------------------------------------------------- JSON guard + literal parsing

/// Rejects excessive nesting and duplicate object keys (anywhere) before typed parsing.
pub fn json_guard(bytes: &[u8], field: &str) -> Result<(), Diagnostic> {
    use std::collections::HashSet;
    struct Frame {
        object: bool,
        expect_key: bool,
        keys: HashSet<String>,
    }
    if bytes.len() > MAX_JSON_BYTES {
        return Err(Diagnostic::error(
            Code::TooLarge,
            field,
            format!("document exceeds {MAX_JSON_BYTES} bytes"),
        ));
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                if i >= bytes.len() {
                    return Err(Diagnostic::error(
                        Code::Malformed,
                        field,
                        "unterminated string",
                    ));
                }
                if let Some(top) = stack.last_mut()
                    && top.object
                    && top.expect_key
                {
                    top.expect_key = false;
                    let key: String = serde_json::from_slice(&bytes[start..=i]).map_err(|e| {
                        Diagnostic::error(Code::Malformed, field, format!("invalid key: {e}"))
                    })?;
                    if !top.keys.insert(key.clone()) {
                        return Err(Diagnostic::error(
                            Code::DuplicateName,
                            field,
                            format!("duplicate key `{key}`"),
                        ));
                    }
                }
            }
            b'{' | b'[' => {
                if stack.len() >= MAX_JSON_DEPTH {
                    return Err(Diagnostic::error(
                        Code::TooDeep,
                        field,
                        format!("nesting exceeds {MAX_JSON_DEPTH}"),
                    ));
                }
                let object = bytes[i] == b'{';
                stack.push(Frame {
                    object,
                    expect_key: object,
                    keys: HashSet::new(),
                });
            }
            b'}' | b']' => {
                stack.pop();
            }
            b',' => {
                if let Some(top) = stack.last_mut() {
                    top.expect_key = top.object;
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// Reads the `schema` number before interpreting any other field.
pub fn check_schema(bytes: &[u8], field: &str, supported: u32) -> Result<(), Diagnostic> {
    #[derive(Deserialize)]
    struct Probe {
        schema: Option<Value>,
    }
    let probe: Probe = serde_json::from_slice(bytes)
        .map_err(|e| Diagnostic::error(Code::Malformed, field, format!("invalid JSON: {e}")))?;
    match probe.schema.as_ref().and_then(Value::as_u64) {
        Some(v) if v == u64::from(supported) => Ok(()),
        Some(v) => Err(Diagnostic::error(
            Code::UnsupportedSchema,
            format!("{field}.schema"),
            format!("schema version {v} is unsupported; this build reads version {supported}"),
        )),
        None => Err(Diagnostic::error(
            Code::UnsupportedSchema,
            format!("{field}.schema"),
            "missing or non-integer schema version",
        )),
    }
}

fn finite(value: f64, field: &str) -> Result<f64, Diagnostic> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Diagnostic::error(
            Code::NonFinite,
            field,
            "value must be finite",
        ))
    }
}

fn parse_unit_number(text: &str, field: &str) -> Result<f64, Diagnostic> {
    let ok_shape = text
        .bytes()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'));
    let n: f64 = text.parse().map_err(|_| {
        Diagnostic::error(
            Code::InvalidValue,
            field,
            format!("`{text}` is not a number"),
        )
    })?;
    if !ok_shape {
        return Err(Diagnostic::error(
            Code::InvalidValue,
            field,
            format!("`{text}` is not a number"),
        ));
    }
    finite(n, field)
}

fn bound(value: f64, min: f64, max: f64, field: &str) -> Result<f64, Diagnostic> {
    if value < min || value > max {
        return Err(Diagnostic::error(
            Code::InvalidValue,
            field,
            format!("{value} is outside {min}..={max}"),
        ));
    }
    Ok(canon(value))
}

/// A length in video pixels: a JSON number (px) or a string `<n>px` / `<n>rem`.
pub fn parse_length(
    value: &Value,
    design: &Design,
    field: &str,
    min: f64,
    max: f64,
) -> Result<f64, Diagnostic> {
    let px = match value {
        Value::Number(n) => finite(n.as_f64().unwrap_or(f64::NAN), field)?,
        Value::String(s) => parse_length_str(s, design, field)?,
        _ => {
            return Err(Diagnostic::error(
                Code::InvalidType,
                field,
                "expected a number (px) or a `<n>px`/`<n>rem` string",
            ));
        }
    };
    bound(px, min, max, field)
}

/// `<n>px` or `<n>rem`; every other unit is reported as unsupported.
pub fn parse_length_str(text: &str, design: &Design, field: &str) -> Result<f64, Diagnostic> {
    let text = text.trim();
    if let Some(n) = text.strip_suffix("rem") {
        Ok(parse_unit_number(n, field)? * design.base_font_px)
    } else if let Some(n) = text.strip_suffix("px") {
        parse_unit_number(n, field)
    } else {
        Err(unit_error(text, field, "px or rem"))
    }
}

fn unit_error(text: &str, field: &str, supported: &str) -> Diagnostic {
    let unit: String = text
        .trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
        .to_owned();
    if unit.is_empty() {
        Diagnostic::error(
            Code::UnsupportedUnit,
            field,
            format!("`{text}` has no unit; use {supported}"),
        )
    } else {
        Diagnostic::error(
            Code::UnsupportedUnit,
            field,
            format!("unit `{unit}` is unsupported; use {supported}"),
        )
    }
}

/// Seconds: a JSON number or `<n>s` / `<n>ms`.
pub fn parse_duration(value: &Value, field: &str) -> Result<f64, Diagnostic> {
    let seconds = match value {
        Value::Number(n) => finite(n.as_f64().unwrap_or(f64::NAN), field)?,
        Value::String(s) => parse_duration_str(s, field)?,
        _ => {
            return Err(Diagnostic::error(
                Code::InvalidType,
                field,
                "expected seconds as a number or a `<n>s`/`<n>ms` string",
            ));
        }
    };
    bound(seconds, 0.0, 3600.0, field)
}

pub fn parse_duration_str(text: &str, field: &str) -> Result<f64, Diagnostic> {
    let text = text.trim();
    if let Some(n) = text.strip_suffix("ms") {
        Ok(parse_unit_number(n, field)? / 1000.0)
    } else if let Some(n) = text.strip_suffix('s') {
        parse_unit_number(n, field)
    } else {
        Err(unit_error(text, field, "s or ms"))
    }
}

fn number(value: &Value, field: &str) -> Result<f64, Diagnostic> {
    match value {
        Value::Number(n) => finite(n.as_f64().unwrap_or(f64::NAN), field),
        _ => Err(Diagnostic::error(
            Code::InvalidType,
            field,
            "expected a number",
        )),
    }
}

fn string(value: &Value, field: &str) -> Result<String, Diagnostic> {
    match value {
        Value::String(s) if s.len() <= MAX_STRING_LEN && !s.chars().any(char::is_control) => {
            Ok(s.clone())
        }
        Value::String(_) => Err(Diagnostic::error(
            Code::InvalidValue,
            field,
            "string too long or contains control characters",
        )),
        _ => Err(Diagnostic::error(
            Code::InvalidType,
            field,
            "expected a string",
        )),
    }
}

fn object<'a>(
    value: &'a Value,
    field: &str,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, Diagnostic> {
    let map = value
        .as_object()
        .ok_or_else(|| Diagnostic::error(Code::InvalidType, field, "expected an object"))?;
    if let Some(extra) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(Diagnostic::error(
            Code::InvalidValue,
            format!("{field}.{extra}"),
            format!("unknown field; allowed: {}", allowed.join(", ")),
        ));
    }
    Ok(map)
}

fn required<'a>(
    map: &'a Map<String, Value>,
    key: &str,
    field: &str,
) -> Result<&'a Value, Diagnostic> {
    map.get(key).ok_or_else(|| {
        Diagnostic::error(
            Code::InvalidValue,
            format!("{field}.{key}"),
            "missing required field",
        )
    })
}

/// Normalize an authored literal of `kind` (units converted, numbers canonical, bounds applied).
pub fn parse_literal(
    kind: TokenKind,
    name: &TokenName,
    value: &Value,
    design: &Design,
) -> Result<TokenValue, Diagnostic> {
    let field = format!("tokens.{name}.value");
    let f = field.as_str();
    Ok(match kind {
        TokenKind::Color => {
            let text = string(value, f)?;
            TokenValue::Color(Color::parse_hex(&text).ok_or_else(|| {
                Diagnostic::error(
                    Code::InvalidValue,
                    f,
                    format!("`{text}` is not a #rgb/#rgba/#rrggbb/#rrggbbaa color"),
                )
            })?)
        }
        TokenKind::Dimension => {
            TokenValue::Dimension(parse_length(value, design, f, 0.0, 100_000.0)?)
        }
        TokenKind::Duration => TokenValue::Duration(parse_duration(value, f)?),
        TokenKind::Typography => {
            let map = object(
                value,
                f,
                &["family", "size", "weight", "line_height", "letter_spacing"],
            )?;
            let family = string(required(map, "family", f)?, &format!("{f}.family"))?;
            if family.trim().is_empty() {
                return Err(Diagnostic::error(
                    Code::InvalidValue,
                    format!("{f}.family"),
                    "font family must not be empty",
                ));
            }
            let weight_field = format!("{f}.weight");
            let weight = required(map, "weight", f)?
                .as_u64()
                .filter(|w| (1..=1000).contains(w))
                .ok_or_else(|| {
                    Diagnostic::error(
                        Code::InvalidValue,
                        &weight_field,
                        "weight must be an integer in 1..=1000",
                    )
                })?;
            let size = parse_length(
                required(map, "size", f)?,
                design,
                &format!("{f}.size"),
                1.0,
                4096.0,
            )?;
            let lh_field = format!("{f}.line_height");
            let line_height = bound(
                number(required(map, "line_height", f)?, &lh_field)?,
                0.1,
                10.0,
                &lh_field,
            )?;
            let letter_spacing = parse_length(
                required(map, "letter_spacing", f)?,
                design,
                &format!("{f}.letter_spacing"),
                -256.0,
                256.0,
            )?;
            TokenValue::Typography(Typography {
                family,
                size,
                weight: weight as u16,
                line_height,
                letter_spacing,
            })
        }
        TokenKind::Easing => {
            let map = value.as_object().ok_or_else(|| {
                Diagnostic::error(Code::InvalidType, f, "expected an object with `kind`")
            })?;
            let kind = required(map, "kind", f)?.as_str().ok_or_else(|| {
                Diagnostic::error(Code::InvalidType, format!("{f}.kind"), "expected a string")
            })?;
            let allowed: &[&str] = match kind {
                "linear" | "ease_in" | "ease_out" | "ease_in_out" => &["kind"],
                "cubic_bezier" => &["kind", "x1", "y1", "x2", "y2"],
                "spring" => &["kind", "mass", "stiffness", "damping"],
                other => {
                    return Err(Diagnostic::error(
                        Code::InvalidValue,
                        format!("{f}.kind"),
                        format!(
                            "unsupported easing `{other}`; use linear, ease_in, ease_out, ease_in_out, cubic_bezier or spring"
                        ),
                    ));
                }
            };
            object(value, f, allowed)?;
            let n = |key: &str, min: f64, max: f64| -> Result<f64, Diagnostic> {
                let fld = format!("{f}.{key}");
                bound(number(required(map, key, f)?, &fld)?, min, max, &fld)
            };
            TokenValue::Easing(match kind {
                "linear" => Easing::Linear,
                "ease_in" => Easing::EaseIn,
                "ease_out" => Easing::EaseOut,
                "ease_in_out" => Easing::EaseInOut,
                "cubic_bezier" => Easing::CubicBezier {
                    x1: n("x1", 0.0, 1.0)?,
                    y1: n("y1", -4.0, 4.0)?,
                    x2: n("x2", 0.0, 1.0)?,
                    y2: n("y2", -4.0, 4.0)?,
                },
                _ => Easing::Spring {
                    mass: n("mass", 0.001, 10_000.0)?,
                    stiffness: n("stiffness", 0.001, 100_000.0)?,
                    damping: n("damping", 0.0, 100_000.0)?,
                },
            })
        }
        TokenKind::Shadow => {
            let map = object(value, f, &["dx", "dy", "blur", "color"])?;
            let len = |key: &str, min: f64| {
                parse_length(
                    required(map, key, f)?,
                    design,
                    &format!("{f}.{key}"),
                    min,
                    10_000.0,
                )
            };
            let color_text = string(required(map, "color", f)?, &format!("{f}.color"))?;
            TokenValue::Shadow(Shadow {
                dx: len("dx", -10_000.0)?,
                dy: len("dy", -10_000.0)?,
                blur: len("blur", 0.0)?,
                color: Color::parse_hex(&color_text).ok_or_else(|| {
                    Diagnostic::error(
                        Code::InvalidValue,
                        format!("{f}.color"),
                        format!("`{color_text}` is not a hex color"),
                    )
                })?,
            })
        }
    })
}
