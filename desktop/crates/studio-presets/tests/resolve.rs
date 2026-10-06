use serde_json::json;
use studio_presets::{
    Code, Design, Layer, OverrideLayer, OverridesFile, PresetError, ResolvedSnapshot, TokenDef,
    TokenName, TokenSet, TokenValue, reresolve_project_style, resolve,
};

fn set(tokens: serde_json::Value) -> Result<TokenSet, PresetError> {
    TokenSet::parse(
        json!({"schema":1,"tokens":tokens}).to_string().as_bytes(),
        &Design::HD,
    )
}
fn base() -> TokenSet {
    set(json!({
        "color.accent": {"type":"color","value":"#112233"},
        "color.link": {"type":"color","alias":"color.accent"},
        "color.hover": {"type":"color","alias":"color.link"},
        "color.text": {"type":"color","value":"#000000"},
        "spacing.md": {"type":"dimension","value":24},
        "spacing.pad": {"type":"dimension","alias":"spacing.md"},
        "motion.duration.base": {"type":"duration","value":0.5},
        "motion.stagger.tight": {"type":"duration","alias":"motion.duration.base"},
    }))
    .unwrap()
}
fn name(n: &str) -> TokenName {
    TokenName::new(n).unwrap()
}
fn color(hex: &str) -> TokenDef {
    TokenDef::Literal(TokenValue::Color(
        studio_presets::Color::parse_hex(hex).unwrap(),
    ))
}
fn dim(v: f64) -> TokenDef {
    TokenDef::Literal(TokenValue::Dimension(v))
}
fn value(s: &ResolvedSnapshot, n: &str) -> TokenValue {
    s.get(n).unwrap().clone()
}

#[test]
fn alias_chains_resolve_to_typed_values_and_same_type_cross_family_is_allowed() {
    let s = resolve(&base(), None, None);
    assert_eq!(value(&s, "color.hover"), value(&s, "color.accent"));
    assert_eq!(value(&s, "spacing.pad"), TokenValue::Dimension(24.0));
    // duration alias across the duration/stagger families
    assert_eq!(value(&s, "motion.stagger.tight"), TokenValue::Duration(0.5));
    assert!(s.diagnostics().is_empty());
}

#[test]
fn alias_errors_are_field_specific() {
    let missing = set(json!({"color.a": {"type":"color","alias":"color.nope"}})).unwrap_err();
    assert!(missing.has(Code::MissingReference));
    assert_eq!(missing.diagnostics[0].field, "tokens.color.a.alias");

    let cross = set(json!({
        "color.a": {"type":"color","alias":"spacing.a"},
        "spacing.a": {"type":"dimension","value":1}
    }))
    .unwrap_err();
    assert!(cross.has(Code::CrossTypeAlias), "{cross}");

    let cycle = set(json!({
        "color.a": {"type":"color","alias":"color.b"},
        "color.b": {"type":"color","alias":"color.c"},
        "color.c": {"type":"color","alias":"color.a"},
        "color.tail": {"type":"color","alias":"color.a"},
    }))
    .unwrap_err();
    assert_eq!(
        cycle
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::Cycle)
            .count(),
        1,
        "{cycle}"
    );
    let selfref = set(json!({"color.a": {"type":"color","alias":"color.a"}})).unwrap_err();
    assert!(selfref.has(Code::Cycle));
    let bad_target = set(json!({"color.a": {"type":"color","alias":"Color.B"}})).unwrap_err();
    assert!(bad_target.has(Code::InvalidName));
}

#[test]
fn precedence_is_preset_then_project_then_scene_and_aliases_follow_overrides() {
    let mut project = OverrideLayer::new("overrides.tokens");
    project.insert(name("color.accent"), color("#FF0000"));
    project.insert(name("spacing.md"), dim(40.0));
    let mut scene = OverrideLayer::new("overrides.scenes.intro");
    scene.insert(name("spacing.md"), dim(8.0));

    let preset_only = resolve(&base(), None, None);
    let with_project = resolve(&base(), Some(&project), None);
    let with_scene = resolve(&base(), Some(&project), Some(&scene));

    assert_eq!(value(&preset_only, "color.link"), color_value("#112233"));
    assert_eq!(value(&with_project, "color.accent"), color_value("#FF0000"));
    assert_eq!(
        value(&with_project, "color.hover"),
        color_value("#FF0000"),
        "alias observes override"
    );
    assert_eq!(
        value(&with_project, "spacing.pad"),
        TokenValue::Dimension(40.0)
    );
    assert_eq!(
        value(&with_scene, "spacing.md"),
        TokenValue::Dimension(8.0),
        "scene beats project"
    );
    assert_eq!(
        value(&with_scene, "spacing.pad"),
        TokenValue::Dimension(8.0)
    );
    assert_eq!(value(&with_scene, "color.accent"), color_value("#FF0000"));
    assert_eq!(
        with_scene.overridden_by()[&name("spacing.md")],
        Layer::Scene
    );
    assert_eq!(
        with_scene.overridden_by()[&name("color.accent")],
        Layer::Project
    );
    assert!(!with_scene.overridden_by().contains_key(&name("color.text")));
    // The preset layer itself is unchanged.
    assert_eq!(resolve(&base(), None, None), preset_only);
}

fn color_value(hex: &str) -> TokenValue {
    TokenValue::Color(studio_presets::Color::parse_hex(hex).unwrap())
}

#[test]
fn orphaned_and_type_mismatched_overrides_are_reported_not_applied() {
    let mut project = OverrideLayer::new("overrides.tokens");
    project.insert(name("color.gone"), color("#FFFFFF"));
    project.insert(name("color.text"), dim(3.0)); // wrong type for an existing color token
    project.insert(name("spacing.md"), dim(10.0));
    let s = resolve(&base(), Some(&project), None);
    let codes: Vec<(Code, &str)> = s
        .diagnostics()
        .iter()
        .map(|d| (d.code, d.field.as_str()))
        .collect();
    assert!(
        codes.contains(&(Code::OrphanedOverride, "overrides.tokens.color.gone")),
        "{codes:?}"
    );
    assert!(
        codes.contains(&(Code::TypeMismatch, "overrides.tokens.color.text")),
        "{codes:?}"
    );
    assert_eq!(value(&s, "color.text"), color_value("#000000"));
    assert_eq!(
        value(&s, "spacing.md"),
        TokenValue::Dimension(10.0),
        "valid entries still apply"
    );
    assert!(s.get("color.gone").is_none());
}

#[test]
fn alias_overrides_that_would_break_the_graph_are_rejected() {
    let alias = |kind, target: &str| {
        TokenDef::Alias(studio_presets::model::AliasDef {
            kind,
            alias: name(target),
        })
    };
    let mut project = OverrideLayer::new("overrides.tokens");
    // accent -> hover -> link -> accent would be a cycle.
    project.insert(
        name("color.accent"),
        alias(studio_presets::TokenKind::Color, "color.hover"),
    );
    // valid alias override
    project.insert(
        name("color.text"),
        alias(studio_presets::TokenKind::Color, "color.accent"),
    );
    // missing target
    project.insert(
        name("spacing.md"),
        alias(studio_presets::TokenKind::Dimension, "spacing.nope"),
    );
    let s = resolve(&base(), Some(&project), None);
    let invalid: Vec<&str> = s
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::InvalidOverride)
        .map(|d| d.field.as_str())
        .collect();
    assert_eq!(
        invalid,
        [
            "overrides.tokens.color.accent",
            "overrides.tokens.spacing.md"
        ]
    );
    assert_eq!(value(&s, "color.text"), color_value("#112233"));
    assert_eq!(value(&s, "spacing.md"), TokenValue::Dimension(24.0));
}

#[test]
fn snapshot_and_hash_are_deterministic_and_sensitive() {
    let a = resolve(&base(), None, None);
    let b = resolve(&base(), None, None);
    assert_eq!(a.runtime_tokens_json(), b.runtime_tokens_json());
    assert_eq!(a.hash(), b.hash());
    let mut project = OverrideLayer::new("overrides.tokens");
    project.insert(name("spacing.md"), dim(25.0));
    assert_ne!(resolve(&base(), Some(&project), None).hash(), a.hash());
    // keys come out sorted regardless of insertion order
    let reversed = set(json!({
        "spacing.md": {"type":"dimension","value":24},
        "color.text": {"type":"color","value":"#000000"},
    }))
    .unwrap();
    let forward = set(json!({
        "color.text": {"type":"color","value":"#000000"},
        "spacing.md": {"type":"dimension","value":24},
    }))
    .unwrap();
    assert_eq!(
        resolve(&reversed, None, None).runtime_tokens_json(),
        resolve(&forward, None, None).runtime_tokens_json()
    );
}

#[test]
fn numbers_are_canonical() {
    let s = set(json!({
        "spacing.a": {"type":"dimension","value":0.1234567891},
        "spacing.b": {"type":"dimension","value":"-0px"},
        "spacing.c": {"type":"dimension","value":"0.5rem"},
    }));
    // -0px is below the minimum only if negative; -0 is zero and must serialize as 0.0
    let s = resolve(&s.unwrap(), None, None);
    let json = s.runtime_tokens_json();
    assert!(
        json.contains(r#""spacing.a":{"type":"dimension","value":0.123457}"#),
        "{json}"
    );
    assert!(
        json.contains(r#""spacing.b":{"type":"dimension","value":0.0}"#),
        "{json}"
    );
    assert!(
        json.contains(r#""spacing.c":{"type":"dimension","value":8.0}"#),
        "{json}"
    );
}

#[test]
fn overrides_file_round_trips_keeps_orphans_and_validates_scenes() {
    let doc = json!({"schema":1,"tokens":{
        "color.accent":{"type":"color","value":"#FF0000"},
        "color.removed":{"type":"color","value":"#00FF00"}
    },"scenes":{"intro":{"spacing.md":{"type":"dimension","value":"1rem"}}}});
    let file = OverridesFile::parse(doc.to_string().as_bytes(), &Design::HD).unwrap();
    assert_eq!(file.scene("intro").unwrap().defs().len(), 1);
    let bytes = file.to_canonical_bytes();
    assert_eq!(OverridesFile::parse(&bytes, &Design::HD).unwrap(), file);
    assert_eq!(
        OverridesFile::parse(&bytes, &Design::HD)
            .unwrap()
            .to_canonical_bytes(),
        bytes
    );
    let s = resolve(&base(), Some(file.project()), file.scene("intro"));
    assert_eq!(value(&s, "spacing.md"), TokenValue::Dimension(16.0));
    assert!(
        s.diagnostics().iter().any(
            |d| d.code == Code::OrphanedOverride && d.field == "overrides.tokens.color.removed"
        )
    );

    assert!(
        OverridesFile::parse(br#"{"schema":2}"#, &Design::HD)
            .unwrap_err()
            .has(Code::UnsupportedSchema)
    );
    assert!(
        OverridesFile::parse(br#"{"schema":1,"scenes":{"a/b":{}}}"#, &Design::HD)
            .unwrap_err()
            .has(Code::InvalidName)
    );
    assert!(OverridesFile::parse(br#"{"schema":1,"extra":1}"#, &Design::HD).is_err());
    let empty = OverridesFile::default();
    assert_eq!(
        empty.to_canonical_bytes(),
        br#"{"schema":1,"tokens":{},"scenes":{}}"#
    );
    assert!(OverridesFile::default().scene_mut("bad id").is_err());
}

#[test]
fn reresolve_uses_stored_defaults_and_edited_overrides() {
    let package = studio_presets::builtin::get("pulse").unwrap();
    let mut overrides = OverridesFile::default();
    overrides
        .project_mut()
        .insert(name("color.accent"), color("#123456"));
    let style = studio_presets::materialize(package, &overrides).unwrap();
    let file = |p: &str| {
        style
            .files
            .iter()
            .find(|(k, _)| k.as_str() == p)
            .unwrap()
            .1
            .clone()
    };
    let (identity, snapshot) = reresolve_project_style(
        &file("style/preset.json"),
        &file("style/preset-tokens.json"),
        &file("style/overrides.json"),
        None,
    )
    .unwrap();
    assert_eq!(identity, style.identity);
    assert_eq!(
        snapshot.runtime_tokens_json(),
        style.snapshot.runtime_tokens_json()
    );
    assert_eq!(
        file("style/tokens.json"),
        style.snapshot.runtime_tokens_json().into_bytes()
    );
}
