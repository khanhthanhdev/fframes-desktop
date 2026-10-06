//! Pure token-set parsing, alias resolution and preset -> project -> scene precedence.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::{
    AliasDef, Code, Design, Diagnostic, MAX_DIAGNOSTICS, MAX_OVERRIDE_SCENES, MAX_TOKENS,
    PresetError, TOKEN_SCHEMA, TokenDef, TokenKind, TokenName, TokenValue, check_schema,
    json_guard, parse_literal,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawToken {
    #[serde(rename = "type")]
    kind: String,
    value: Option<Value>,
    alias: Option<String>,
}

fn normalize(
    name: &str,
    raw: RawToken,
    design: &Design,
    prefix: &str,
) -> Result<(TokenName, TokenDef), Diagnostic> {
    let relabel = |mut d: Diagnostic| {
        if prefix != "tokens" {
            d.field = d.field.replacen("tokens", prefix, 1);
        }
        d
    };
    let name = TokenName::new(name).map_err(relabel)?;
    let field = format!("{prefix}.{name}");
    let kind = TokenKind::parse(&raw.kind).ok_or_else(|| {
        Diagnostic::error(
            Code::InvalidType,
            format!("{field}.type"),
            format!(
                "unsupported type `{}`; use color, typography, dimension, duration, easing or shadow",
                raw.kind
            ),
        )
    })?;
    if kind != name.kind() {
        return Err(Diagnostic::error(
            Code::TypeMismatch,
            format!("{field}.type"),
            format!(
                "`{name}` belongs to the {} family but declares type {}",
                name.kind().as_str(),
                kind.as_str()
            ),
        ));
    }
    let def = match (raw.value, raw.alias) {
        (Some(value), None) => {
            TokenDef::Literal(parse_literal(kind, &name, &value, design).map_err(relabel)?)
        }
        (None, Some(alias)) => TokenDef::Alias(AliasDef {
            kind,
            alias: TokenName::new(alias).map_err(|mut d| {
                d.field = format!("{field}.alias");
                d
            })?,
        }),
        _ => {
            return Err(Diagnostic::error(
                Code::InvalidValue,
                field,
                "exactly one of `value` or `alias` is required",
            ));
        }
    };
    Ok((name, def))
}

fn normalize_map(
    raw: BTreeMap<String, RawToken>,
    design: &Design,
    prefix: &str,
) -> Result<BTreeMap<TokenName, TokenDef>, PresetError> {
    if raw.len() > MAX_TOKENS {
        return Err(PresetError::one(
            Code::TooLarge,
            prefix,
            format!("more than {MAX_TOKENS} tokens"),
        ));
    }
    let mut out = BTreeMap::new();
    let mut errors = Vec::new();
    for (name, token) in raw {
        match normalize(&name, token, design, prefix) {
            Ok((name, def)) => {
                out.insert(name, def);
            }
            Err(d) => {
                if errors.len() < MAX_DIAGNOSTICS {
                    errors.push(d);
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(PresetError::new(errors))
    }
}

fn guard(bytes: &[u8], field: &str, design: &Design) -> Result<(), PresetError> {
    design.validate("design")?;
    json_guard(bytes, field)?;
    check_schema(bytes, field, TOKEN_SCHEMA)?;
    Ok(())
}

fn invalid_json(field: &str, e: serde_json::Error) -> PresetError {
    PresetError::one(
        Code::Malformed,
        field,
        format!("invalid token document: {e}"),
    )
}

/// Alias graph checks shared by presets and every tentative override merge.
fn validate_aliases(defs: &BTreeMap<TokenName, TokenDef>) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    for (name, def) in defs {
        let TokenDef::Alias(alias) = def else {
            continue;
        };
        let field = format!("tokens.{name}.alias");
        let Some(target) = defs.get(&alias.alias) else {
            errors.push(Diagnostic::error(
                Code::MissingReference,
                field,
                format!("alias target `{}` does not exist", alias.alias),
            ));
            continue;
        };
        if target.kind() != alias.kind {
            errors.push(Diagnostic::error(
                Code::CrossTypeAlias,
                field,
                format!(
                    "`{name}` is {} but `{}` is {}",
                    alias.kind.as_str(),
                    alias.alias,
                    target.kind().as_str()
                ),
            ));
            continue;
        }
        // Follow the chain; report the cycle once, from its smallest member.
        let mut path = vec![name];
        let mut cursor = &alias.alias;
        loop {
            if let Some(at) = path.iter().position(|p| *p == cursor) {
                let cycle = &path[at..];
                if cycle.iter().min().copied() == Some(name) && cycle.contains(&name) {
                    let names: Vec<&str> = cycle.iter().map(|n| n.as_str()).collect();
                    errors.push(Diagnostic::error(
                        Code::Cycle,
                        field,
                        format!("alias cycle: {} -> {}", names.join(" -> "), cursor),
                    ));
                }
                break;
            }
            match defs.get(cursor) {
                Some(TokenDef::Alias(next)) => {
                    path.push(cursor);
                    cursor = &next.alias;
                }
                _ => break,
            }
        }
    }
    errors.truncate(MAX_DIAGNOSTICS);
    errors
}

fn flatten(defs: &BTreeMap<TokenName, TokenDef>) -> BTreeMap<TokenName, TokenValue> {
    let mut out = BTreeMap::new();
    for (name, def) in defs {
        let mut cursor = def;
        // Bounded: validated graphs are acyclic; the guard keeps a bad input from looping.
        for _ in 0..=defs.len() {
            match cursor {
                TokenDef::Literal(v) => {
                    out.insert(name.clone(), v.clone());
                    break;
                }
                TokenDef::Alias(a) => match defs.get(&a.alias) {
                    Some(next) => cursor = next,
                    None => break,
                },
            }
        }
    }
    out
}

/// Validated preset defaults: bounded, typed, alias-checked.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenSet {
    defs: BTreeMap<TokenName, TokenDef>,
}
impl TokenSet {
    /// Parse authored `tokens.json`: `{"schema":1,"tokens":{name:{"type":..,"value"|"alias":..}}}`.
    pub fn parse(bytes: &[u8], design: &Design) -> Result<Self, PresetError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            #[allow(dead_code)]
            schema: u32,
            tokens: BTreeMap<String, RawToken>,
        }
        guard(bytes, "tokens.json", design)?;
        let raw: Raw = serde_json::from_slice(bytes).map_err(|e| invalid_json("tokens.json", e))?;
        Self::from_defs(normalize_map(raw.tokens, design, "tokens")?)
    }

    pub fn from_defs(defs: BTreeMap<TokenName, TokenDef>) -> Result<Self, PresetError> {
        if defs.len() > MAX_TOKENS {
            return Err(PresetError::one(
                Code::TooLarge,
                "tokens",
                format!("more than {MAX_TOKENS} tokens"),
            ));
        }
        let mismatched: Vec<Diagnostic> = defs
            .iter()
            .filter(|(n, d)| n.kind() != d.kind())
            .map(|(n, d)| {
                Diagnostic::error(
                    Code::TypeMismatch,
                    format!("tokens.{n}.type"),
                    format!(
                        "`{n}` is a {} name but holds {}",
                        n.kind().as_str(),
                        d.kind().as_str()
                    ),
                )
            })
            .collect();
        let mut errors = mismatched;
        errors.extend(validate_aliases(&defs));
        if errors.is_empty() {
            Ok(Self { defs })
        } else {
            Err(PresetError::new(errors))
        }
    }

    pub fn defs(&self) -> &BTreeMap<TokenName, TokenDef> {
        &self.defs
    }
    pub fn get(&self, name: &str) -> Option<&TokenDef> {
        self.defs
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .map(|(_, d)| d)
    }
    pub fn len(&self) -> usize {
        self.defs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
    /// All tokens with aliases followed (no overrides).
    pub fn resolved_values(&self) -> BTreeMap<TokenName, TokenValue> {
        flatten(&self.defs)
    }
    /// Normalized authored form, deterministic, accepted back by [`TokenSet::parse`].
    pub fn to_canonical_json(&self) -> String {
        #[derive(Serialize)]
        struct Out<'a> {
            schema: u32,
            tokens: &'a BTreeMap<TokenName, TokenDef>,
        }
        serde_json::to_string(&Out {
            schema: TOKEN_SCHEMA,
            tokens: &self.defs,
        })
        .expect("token set serializes")
    }
}

/// Which layer supplied a directly-overridden token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Preset,
    Project,
    Scene,
}

/// Overrides for one layer. Not validated against any preset until resolution.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OverrideLayer {
    label: String,
    defs: BTreeMap<TokenName, TokenDef>,
}
impl OverrideLayer {
    /// `label` prefixes diagnostic fields, e.g. `overrides.tokens`.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            defs: BTreeMap::new(),
        }
    }
    pub fn insert(&mut self, name: TokenName, def: TokenDef) -> Option<TokenDef> {
        self.defs.insert(name, def)
    }
    pub fn defs(&self) -> &BTreeMap<TokenName, TokenDef> {
        &self.defs
    }
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// `style/overrides.json`: project overrides plus optional per-scene layers.
#[derive(Debug, Clone, PartialEq)]
pub struct OverridesFile {
    project: OverrideLayer,
    scenes: BTreeMap<String, OverrideLayer>,
}
impl Default for OverridesFile {
    fn default() -> Self {
        Self {
            project: OverrideLayer::new("overrides.tokens"),
            scenes: BTreeMap::new(),
        }
    }
}
impl OverridesFile {
    pub fn parse(bytes: &[u8], design: &Design) -> Result<Self, PresetError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            #[allow(dead_code)]
            schema: u32,
            #[serde(default)]
            tokens: BTreeMap<String, RawToken>,
            #[serde(default)]
            scenes: BTreeMap<String, BTreeMap<String, RawToken>>,
        }
        guard(bytes, "overrides.json", design)?;
        let raw: Raw =
            serde_json::from_slice(bytes).map_err(|e| invalid_json("overrides.json", e))?;
        if raw.scenes.len() > MAX_OVERRIDE_SCENES {
            return Err(PresetError::one(
                Code::TooLarge,
                "scenes",
                format!("more than {MAX_OVERRIDE_SCENES} scene override sets"),
            ));
        }
        let mut file = Self::default();
        file.project.defs = normalize_map(raw.tokens, design, "overrides.tokens")?;
        for (id, tokens) in raw.scenes {
            if !scene_id_ok(&id) {
                return Err(PresetError::one(
                    Code::InvalidName,
                    format!("overrides.scenes.{id}"),
                    "scene ids are 1..=64 characters of [A-Za-z0-9_.-]",
                ));
            }
            let label = format!("overrides.scenes.{id}");
            let defs = normalize_map(tokens, design, &label)?;
            file.scenes.insert(id, OverrideLayer { label, defs });
        }
        Ok(file)
    }

    pub fn project(&self) -> &OverrideLayer {
        &self.project
    }
    pub fn project_mut(&mut self) -> &mut OverrideLayer {
        &mut self.project
    }
    pub fn scene(&self, id: &str) -> Option<&OverrideLayer> {
        self.scenes.get(id)
    }
    /// Creates the layer if absent.
    pub fn scene_mut(&mut self, id: &str) -> Result<&mut OverrideLayer, PresetError> {
        if !scene_id_ok(id) {
            return Err(PresetError::one(
                Code::InvalidName,
                format!("overrides.scenes.{id}"),
                "scene ids are 1..=64 characters of [A-Za-z0-9_.-]",
            ));
        }
        Ok(self
            .scenes
            .entry(id.to_owned())
            .or_insert_with(|| OverrideLayer::new(format!("overrides.scenes.{id}"))))
    }
    pub fn scenes(&self) -> &BTreeMap<String, OverrideLayer> {
        &self.scenes
    }
    /// Deterministic bytes for `style/overrides.json`. Orphaned entries are retained.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        #[derive(Serialize)]
        struct Out<'a> {
            schema: u32,
            tokens: &'a BTreeMap<TokenName, TokenDef>,
            scenes: BTreeMap<&'a str, &'a BTreeMap<TokenName, TokenDef>>,
        }
        serde_json::to_vec(&Out {
            schema: TOKEN_SCHEMA,
            tokens: &self.project.defs,
            scenes: self
                .scenes
                .iter()
                .map(|(id, l)| (id.as_str(), &l.defs))
                .collect(),
        })
        .expect("overrides serialize")
    }
}
fn scene_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        && id != "."
        && id != ".."
}

/// Immutable, flat, alias-resolved tokens plus the diagnostics produced while merging layers.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSnapshot {
    tokens: BTreeMap<TokenName, TokenValue>,
    sources: BTreeMap<TokenName, Layer>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Serialize)]
struct RuntimeOut<'a> {
    schema: u32,
    tokens: &'a BTreeMap<TokenName, TokenValue>,
}

impl ResolvedSnapshot {
    pub fn tokens(&self) -> &BTreeMap<TokenName, TokenValue> {
        &self.tokens
    }
    pub fn get(&self, name: &str) -> Option<&TokenValue> {
        self.tokens
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .map(|(_, v)| v)
    }
    /// Layer of every token a project/scene override replaced; absent means preset default.
    pub fn overridden_by(&self) -> &BTreeMap<TokenName, Layer> {
        &self.sources
    }
    /// Orphaned, type-mismatched or otherwise unapplied override entries (warnings).
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    /// The exact `style/tokens.json` consumed by `fframes::Styles`.
    pub fn runtime_tokens_json(&self) -> String {
        serde_json::to_string(&RuntimeOut {
            schema: TOKEN_SCHEMA,
            tokens: &self.tokens,
        })
        .expect("snapshot serializes")
    }
    /// SHA-256 (hex) of [`Self::runtime_tokens_json`].
    pub fn hash(&self) -> String {
        hex(&Sha256::digest(self.runtime_tokens_json().as_bytes()))
    }
    /// Strictly read a runtime `style/tokens.json`.
    pub fn from_runtime_json(bytes: &[u8]) -> Result<Self, PresetError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct In {
            #[allow(dead_code)]
            schema: u32,
            tokens: BTreeMap<TokenName, TokenValue>,
        }
        json_guard(bytes, "tokens.json")?;
        check_schema(bytes, "tokens.json", TOKEN_SCHEMA)?;
        let parsed: In =
            serde_json::from_slice(bytes).map_err(|e| invalid_json("tokens.json", e))?;
        if parsed.tokens.len() > MAX_TOKENS {
            return Err(PresetError::one(
                Code::TooLarge,
                "tokens",
                "too many tokens",
            ));
        }
        let bad: Vec<Diagnostic> = parsed
            .tokens
            .iter()
            .filter(|(n, v)| n.kind() != v.kind())
            .map(|(n, _)| {
                Diagnostic::error(
                    Code::TypeMismatch,
                    format!("tokens.{n}.type"),
                    "type does not match the token family",
                )
            })
            .collect();
        if !bad.is_empty() {
            return Err(PresetError::new(bad));
        }
        Ok(Self {
            tokens: parsed.tokens,
            sources: BTreeMap::new(),
            diagnostics: Vec::new(),
        })
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// preset defaults -> project overrides -> scene overrides. Overrides can only replace tokens the
/// preset defines, with the same type, without breaking the alias graph; anything else is kept out
/// of the snapshot and reported (orphaned / type-mismatched / invalid), never coerced or silently
/// dropped. Aliases anywhere in the chain observe overridden targets.
pub fn resolve(
    base: &TokenSet,
    project: Option<&OverrideLayer>,
    scene: Option<&OverrideLayer>,
) -> ResolvedSnapshot {
    let mut merged = base.defs.clone();
    let mut sources = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (layer, which) in [(project, Layer::Project), (scene, Layer::Scene)] {
        let Some(layer) = layer else { continue };
        for (name, def) in &layer.defs {
            let field = format!("{}.{name}", layer.label);
            let Some(existing) = base.defs.get(name) else {
                diagnostics.push(Diagnostic::warning(
                    Code::OrphanedOverride,
                    field,
                    format!(
                        "preset defines no `{name}`; override kept in the file but not applied"
                    ),
                ));
                continue;
            };
            if def.kind() != existing.kind() || def.kind() != name.kind() {
                diagnostics.push(Diagnostic::warning(
                    Code::TypeMismatch,
                    field,
                    format!(
                        "override is {} but `{name}` is {}; not applied",
                        def.kind().as_str(),
                        existing.kind().as_str()
                    ),
                ));
                continue;
            }
            if let TokenDef::Alias(_) = def {
                let mut candidate = merged.clone();
                candidate.insert(name.clone(), def.clone());
                if let Some(first) = validate_aliases(&candidate).into_iter().next() {
                    diagnostics.push(Diagnostic::warning(
                        Code::InvalidOverride,
                        field,
                        format!("alias override rejected: {}", first.message),
                    ));
                    continue;
                }
            }
            merged.insert(name.clone(), def.clone());
            sources.insert(name.clone(), which);
        }
    }
    diagnostics.truncate(MAX_DIAGNOSTICS);
    ResolvedSnapshot {
        tokens: flatten(&merged),
        sources,
        diagnostics,
    }
}
