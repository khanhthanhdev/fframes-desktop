use serde_json::json;
use studio_presets::{
    Code, Color, Design, PresetError, TokenDef, TokenSet, TokenValue,
    model::{Easing, Typography},
    resolve,
};

fn parse(doc: serde_json::Value) -> Result<TokenSet, PresetError> {
    TokenSet::parse(doc.to_string().as_bytes(), &Design::HD)
}
fn tokens(tokens: serde_json::Value) -> Result<TokenSet, PresetError> {
    parse(json!({"schema": 1, "tokens": tokens}))
}
fn code(result: Result<TokenSet, PresetError>, code: Code) -> PresetError {
    let e = result.expect_err("expected failure");
    assert!(e.has(code), "expected {code:?}, got {e}");
    e
}

#[test]
fn accepts_every_token_kind_and_normalizes_units() {
    let set = tokens(json!({
        "color.accent": {"type": "color", "value": "#abc"},
        "color.glass": {"type": "color", "value": "#11223344"},
        "typography.title": {"type": "typography", "value": {
            "family": "DM Sans", "size": "6rem", "weight": 700, "line_height": 1.1, "letter_spacing": "-1px"}},
        "spacing.md": {"type": "dimension", "value": "1.5rem"},
        "radius.sm": {"type": "dimension", "value": 4},
        "stroke.thin": {"type": "dimension", "value": "2px"},
        "motion.duration.fast": {"type": "duration", "value": "250ms"},
        "motion.stagger.tight": {"type": "duration", "value": "0.1s"},
        "motion.easing.standard": {"type": "easing", "value": {"kind": "cubic_bezier", "x1": 0.4, "y1": 0, "x2": 0.2, "y2": 1}},
        "motion.easing.bouncy": {"type": "easing", "value": {"kind": "spring", "mass": 1, "stiffness": 200, "damping": 10}},
        "motion.easing.flat": {"type": "easing", "value": {"kind": "linear"}},
        "shadow.card": {"type": "shadow", "value": {"dx": 0, "dy": "8px", "blur": 24, "color": "#00000066"}},
    }))
    .unwrap();
    let v = set.resolved_values();
    let get = |n: &str| v.iter().find(|(k, _)| k.as_str() == n).unwrap().1.clone();
    assert_eq!(
        get("color.accent"),
        TokenValue::Color(Color::parse_hex("#AABBCC").unwrap())
    );
    assert_eq!(get("spacing.md"), TokenValue::Dimension(24.0));
    assert_eq!(get("motion.duration.fast"), TokenValue::Duration(0.25));
    assert_eq!(get("motion.stagger.tight"), TokenValue::Duration(0.1));
    assert_eq!(
        get("typography.title"),
        TokenValue::Typography(Typography {
            family: "DM Sans".into(),
            size: 96.0,
            weight: 700,
            line_height: 1.1,
            letter_spacing: -1.0
        })
    );
    assert_eq!(
        get("motion.easing.flat"),
        TokenValue::Easing(Easing::Linear)
    );
    // The canonical authored form re-parses to the identical set.
    let again = TokenSet::parse(set.to_canonical_json().as_bytes(), &Design::HD).unwrap();
    assert_eq!(again, set);
}

#[test]
fn runtime_json_matches_the_contract_exactly() {
    let set = tokens(json!({
        "color.accent": {"type": "color", "value": "#112233"},
        "typography.title": {"type": "typography", "value": {
            "family": "DM Sans", "size": 96, "weight": 700, "line_height": 1.1, "letter_spacing": 0}},
        "spacing.md": {"type": "dimension", "value": 24},
        "motion.duration.fast": {"type": "duration", "value": 0.25},
        "motion.easing.standard": {"type": "easing", "value": {"kind": "cubic_bezier", "x1": 0.4, "y1": 0, "x2": 0.2, "y2": 1}},
        "shadow.card": {"type": "shadow", "value": {"dx": 0, "dy": 8, "blur": 24, "color": "#00000066"}},
    }))
    .unwrap();
    let snapshot = resolve(&set, None, None);
    assert_eq!(
        snapshot.runtime_tokens_json(),
        concat!(
            r##"{"schema":1,"tokens":{"##,
            r##""color.accent":{"type":"color","value":"#112233"},"##,
            r##""motion.duration.fast":{"type":"duration","value":0.25},"##,
            r##""motion.easing.standard":{"type":"easing","value":{"kind":"cubic_bezier","x1":0.4,"y1":0.0,"x2":0.2,"y2":1.0}},"##,
            r##""shadow.card":{"type":"shadow","value":{"dx":0.0,"dy":8.0,"blur":24.0,"color":"#00000066"}},"##,
            r##""spacing.md":{"type":"dimension","value":24.0},"##,
            r##""typography.title":{"type":"typography","value":{"family":"DM Sans","size":96.0,"weight":700,"line_height":1.1,"letter_spacing":0.0}}"##,
            r##"}}"##
        )
    );
    let back = studio_presets::ResolvedSnapshot::from_runtime_json(
        snapshot.runtime_tokens_json().as_bytes(),
    )
    .unwrap();
    assert_eq!(back.tokens(), snapshot.tokens());
}

#[test]
fn rejects_future_and_missing_schema_versions_before_reading_fields() {
    let future = br#"{"schema":2,"tokens":{"nonsense":{"zzz":1}},"extra":true}"#;
    let e = TokenSet::parse(future, &Design::HD).unwrap_err();
    assert!(e.has(Code::UnsupportedSchema), "{e}");
    assert_eq!(e.diagnostics[0].field, "tokens.json.schema");
    for bad in [
        &br#"{"tokens":{}}"#[..],
        br#"{"schema":"1","tokens":{}}"#,
        br#"{"schema":0,"tokens":{}}"#,
    ] {
        assert!(
            TokenSet::parse(bad, &Design::HD)
                .unwrap_err()
                .has(Code::UnsupportedSchema)
        );
    }
}

#[test]
fn malformed_input_returns_errors_without_panicking() {
    for bad in [
        &b""[..],
        b"{",
        b"[]",
        b"null",
        b"\xff\xfe",
        br#"{"schema":1}"#,
        br#"{"schema":1,"tokens":[]}"#,
        br#"{"schema":1,"tokens":{"color.a":5}}"#,
    ] {
        assert!(TokenSet::parse(bad, &Design::HD).is_err());
    }
}

#[test]
fn rejects_unknown_fields_names_and_family_type_mismatches() {
    code(
        parse(json!({"schema":1,"tokens":{},"extra":1})),
        Code::Malformed,
    );
    code(
        tokens(json!({"color.a": {"type": "color", "value": "#000", "note": "x"}})),
        Code::Malformed,
    );
    for name in [
        "Color.Accent",
        "color",
        "colour.accent",
        "color..a",
        "color.-a",
        "motion.duration",
        "motion.speed.fast",
        "spacing.a b",
        "radius.é",
    ] {
        code(
            tokens(json!({name: {"type": "dimension", "value": 1}})),
            Code::InvalidName,
        );
    }
    let e = code(
        tokens(json!({"color.accent": {"type": "dimension", "value": 1}})),
        Code::TypeMismatch,
    );
    assert_eq!(e.diagnostics[0].field, "tokens.color.accent.type");
    code(
        tokens(json!({"color.a": {"type": "paint", "value": 1}})),
        Code::InvalidType,
    );
    code(
        tokens(json!({"color.a": {"type": "color"}})),
        Code::InvalidValue,
    );
    code(
        tokens(json!({"color.a": {"type": "color", "value": "#000", "alias": "color.b"}})),
        Code::InvalidValue,
    );
}

#[test]
fn rejects_duplicate_names_and_keys() {
    let dup = br##"{"schema":1,"tokens":{"color.a":{"type":"color","value":"#000"},"color.a":{"type":"color","value":"#fff"}}}"##;
    let e = TokenSet::parse(dup, &Design::HD).unwrap_err();
    assert!(e.has(Code::DuplicateName), "{e}");
    let nested =
        br##"{"schema":1,"tokens":{"spacing.a":{"type":"dimension","value":1,"value":2}}}"##;
    assert!(
        TokenSet::parse(nested, &Design::HD)
            .unwrap_err()
            .has(Code::DuplicateName)
    );
}

#[test]
fn rejects_bad_units_ranges_and_non_finite_numbers() {
    for value in [
        json!("5em"),
        json!("10%"),
        json!("3pt"),
        json!("12"),
        json!("px"),
        json!("inf px"),
        json!(-1),
        json!(100001),
        json!(true),
    ] {
        assert!(
            tokens(json!({"spacing.a": {"type": "dimension", "value": value.clone()}})).is_err(),
            "{value}"
        );
    }
    code(
        tokens(json!({"spacing.a": {"type": "dimension", "value": "5em"}})),
        Code::UnsupportedUnit,
    );
    code(
        tokens(json!({"motion.duration.a": {"type": "duration", "value": "5px"}})),
        Code::UnsupportedUnit,
    );
    code(
        tokens(json!({"motion.duration.a": {"type": "duration", "value": "NaNs"}})),
        Code::InvalidValue,
    );
    code(
        tokens(json!({"motion.duration.a": {"type": "duration", "value": -1}})),
        Code::InvalidValue,
    );
    code(
        tokens(json!({"motion.duration.a": {"type": "duration", "value": "3601s"}})),
        Code::InvalidValue,
    );
    // 1e999 is not representable as a finite f64 and is refused at parse time.
    let huge = br#"{"schema":1,"tokens":{"spacing.a":{"type":"dimension","value":1e999}}}"#;
    assert!(TokenSet::parse(huge, &Design::HD).is_err());
    code(
        tokens(json!({"color.a": {"type": "color", "value": "red"}})),
        Code::InvalidValue,
    );
    code(
        tokens(json!({"color.a": {"type": "color", "value": "#12"}})),
        Code::InvalidValue,
    );
}

#[test]
fn rejects_malformed_composite_values() {
    let typo =
        |v: serde_json::Value| tokens(json!({"typography.t": {"type": "typography", "value": v}}));
    let ok =
        json!({"family":"DM Sans","size":10,"weight":400,"line_height":1.2,"letter_spacing":0});
    assert!(typo(ok.clone()).is_ok());
    for key in ["family", "size", "weight", "line_height", "letter_spacing"] {
        let mut v = ok.clone();
        v.as_object_mut().unwrap().remove(key);
        code(typo(v), Code::InvalidValue);
    }
    for (key, value) in [
        ("weight", json!(0)),
        ("weight", json!(1001)),
        ("weight", json!(400.5)),
        ("size", json!(0)),
        ("line_height", json!(0)),
        ("family", json!("")),
        ("family", json!("a\nb")),
    ] {
        let mut v = ok.clone();
        v[key] = value;
        assert!(typo(v).is_err(), "{key}");
    }
    let mut extra = ok.clone();
    extra["italic"] = json!(true);
    code(typo(extra), Code::InvalidValue);
    let easing =
        |v: serde_json::Value| tokens(json!({"motion.easing.e": {"type": "easing", "value": v}}));
    code(easing(json!({"kind": "bounce"})), Code::InvalidValue);
    code(
        easing(json!({"kind": "linear", "x1": 0})),
        Code::InvalidValue,
    );
    code(
        easing(json!({"kind": "cubic_bezier", "x1": 2, "y1": 0, "x2": 0, "y2": 1})),
        Code::InvalidValue,
    );
    code(
        easing(json!({"kind": "cubic_bezier", "x1": 0, "y1": 0, "x2": 0})),
        Code::InvalidValue,
    );
    code(
        easing(json!({"kind": "spring", "mass": 0, "stiffness": 1, "damping": 1})),
        Code::InvalidValue,
    );
    let shadow = |v: serde_json::Value| tokens(json!({"shadow.s": {"type": "shadow", "value": v}}));
    code(
        shadow(json!({"dx":0,"dy":1,"blur":-1,"color":"#000"})),
        Code::InvalidValue,
    );
    code(shadow(json!({"dx":0,"dy":1,"blur":1})), Code::InvalidValue);
    code(
        shadow(json!({"dx":0,"dy":1,"blur":1,"color":"blue"})),
        Code::InvalidValue,
    );
}

#[test]
fn bounds_tokens_nesting_and_document_size() {
    let many: serde_json::Map<String, serde_json::Value> = (0..513)
        .map(|i| {
            (
                format!("spacing.s{i}"),
                json!({"type":"dimension","value":1}),
            )
        })
        .collect();
    code(tokens(json!(many)), Code::TooLarge);
    let at_limit: serde_json::Map<String, serde_json::Value> = (0..512)
        .map(|i| {
            (
                format!("spacing.s{i}"),
                json!({"type":"dimension","value":1}),
            )
        })
        .collect();
    assert_eq!(tokens(json!(at_limit)).unwrap().len(), 512);
    let deep = format!(
        r#"{{"schema":1,"tokens":{}}}"#,
        "[".repeat(200) + &"]".repeat(200)
    );
    assert!(
        TokenSet::parse(deep.as_bytes(), &Design::HD)
            .unwrap_err()
            .has(Code::TooDeep)
    );
    let big = format!(
        r#"{{"schema":1,"tokens":{{}},"pad":"{}"}}"#,
        "x".repeat(300_000)
    );
    assert!(
        TokenSet::parse(big.as_bytes(), &Design::HD)
            .unwrap_err()
            .has(Code::TooLarge)
    );
    let long_name = format!("color.{}", "a".repeat(40));
    code(
        tokens(json!({long_name: {"type":"color","value":"#000"}})),
        Code::InvalidName,
    );
    code(
        tokens(json!({"color.a": {"type":"color","value":format!("#{}", "0".repeat(300))}})),
        Code::InvalidValue,
    );
}

#[test]
fn rem_requires_a_valid_design_context() {
    let doc = br#"{"schema":1,"tokens":{"spacing.a":{"type":"dimension","value":"2rem"}}}"#;
    let wide = Design {
        base_font_px: 20.0,
        ..Design::HD
    };
    let set = TokenSet::parse(doc, &wide).unwrap();
    assert_eq!(
        set.get("spacing.a"),
        Some(&TokenDef::Literal(TokenValue::Dimension(40.0)))
    );
    for bad in [
        Design {
            base_font_px: 0.0,
            ..Design::HD
        },
        Design {
            base_font_px: f64::NAN,
            ..Design::HD
        },
        Design {
            width: 0,
            ..Design::HD
        },
    ] {
        assert!(TokenSet::parse(doc, &bad).is_err());
    }
}
