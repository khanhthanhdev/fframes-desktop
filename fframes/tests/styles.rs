#![cfg(feature = "styles")]

use fframes::animation::Easing;
use fframes::{Color, Styles, StylesError};

const TOKENS: &str = r##"{"schema":1,"tokens":{
 "color.accent":{"type":"color","value":"#FF8800"},
 "color.scrim":{"type":"color","value":"#00000080"},
 "typography.title":{"type":"typography","value":{"family":"DM Sans","size":96.0,"weight":700,"line_height":1.1,"letter_spacing":0.5}},
 "spacing.md":{"type":"dimension","value":24.0},
 "motion.duration.fast":{"type":"duration","value":0.25},
 "motion.easing.standard":{"type":"easing","value":{"kind":"cubic_bezier","x1":0.4,"y1":0.0,"x2":0.2,"y2":1.0}},
 "motion.easing.lin":{"type":"easing","value":{"kind":"linear"}},
 "motion.easing.bouncy":{"type":"easing","value":{"kind":"spring","mass":1.0,"stiffness":100.0,"damping":10.0}},
 "shadow.card":{"type":"shadow","value":{"dx":0.0,"dy":8.0,"blur":24.0,"color":"#00000066"}}
}}"##;

fn styles() -> Styles {
    Styles::from_json_str(TOKENS).unwrap()
}

#[test]
fn reads_every_token_type() {
    let s = styles();
    assert_eq!(
        s.color("color.accent").unwrap(),
        Color::rgba(255, 136, 0, 255)
    );
    assert_eq!(s.color("color.scrim").unwrap(), Color::rgba(0, 0, 0, 128));
    let t = s.typography("typography.title").unwrap();
    assert_eq!(
        (t.family.as_str(), t.size, t.weight),
        ("DM Sans", 96.0, 700)
    );
    assert_eq!((t.line_height, t.letter_spacing), (1.1, 0.5));
    assert_eq!(s.dimension("spacing.md").unwrap(), 24.0);
    assert_eq!(s.duration("motion.duration.fast").unwrap(), 0.25);
    assert_eq!(
        s.easing("motion.easing.standard").unwrap(),
        Easing::CubicBezier(0.4, 0.0, 0.2, 1.0)
    );
    assert_eq!(s.easing("motion.easing.lin").unwrap(), Easing::Linear);
    assert_eq!(
        s.easing("motion.easing.bouncy").unwrap(),
        Easing::Spring {
            mass: 1.0,
            stiffness: 100.0,
            damping: 10.0
        }
    );
    let sh = s.shadow("shadow.card").unwrap();
    assert_eq!((sh.dx, sh.dy, sh.blur), (0.0, 8.0, 24.0));
    assert_eq!(sh.color, Color::rgba(0, 0, 0, 0x66));
    assert!(s.contains("spacing.md"));
    assert_eq!(s.names().count(), 9);
}

#[test]
fn bytes_and_text_agree() {
    assert_eq!(
        Styles::from_json_slice(TOKENS.as_bytes()).unwrap(),
        styles()
    );
}

#[test]
fn missing_and_wrong_type_are_structured() {
    let s = styles();
    assert_eq!(
        s.color("color.nope").unwrap_err(),
        StylesError::Missing {
            token: "color.nope".into()
        }
    );
    assert_eq!(
        s.color("spacing.md").unwrap_err(),
        StylesError::WrongType {
            token: "spacing.md".into(),
            expected: "color",
            found: "dimension"
        }
    );
    assert!(matches!(
        s.typography("color.accent"),
        Err(StylesError::WrongType { .. })
    ));
}

#[test]
fn rejects_unknown_schema_and_bad_shape() {
    assert!(matches!(
        Styles::from_json_str(r#"{"schema":2,"tokens":{}}"#),
        Err(StylesError::UnsupportedSchema(_))
    ));
    assert!(matches!(
        Styles::from_json_str(r#"{"tokens":{}}"#),
        Err(StylesError::Parse(_))
    ));
    assert!(matches!(
        Styles::from_json_str("not json"),
        Err(StylesError::Parse(_))
    ));
    assert!(matches!(
        Styles::from_json_slice(&[0xff, 0xfe]),
        Err(StylesError::Parse(_))
    ));
}

#[test]
fn rejects_invalid_token_data() {
    for bad in [
        r#"{"type":"color","value":"red"}"#,
        r##"{"type":"color","value":"#12345"}"##,
        r#"{"type":"dimension","value":"24"}"#,
        r#"{"type":"duration","value":-1}"#,
        r#"{"type":"mystery","value":1}"#,
        r#"{"type":"easing","value":{"kind":"bounce"}}"#,
        r#"{"type":"easing","value":{"kind":"cubic_bezier","x1":2,"y1":0,"x2":0,"y2":1}}"#,
        r#"{"type":"typography","value":{"family":"DM Sans","size":10}}"#,
        r#"{"value":1}"#,
    ] {
        let json = format!(r#"{{"schema":1,"tokens":{{"t":{bad}}}}}"#);
        assert!(
            matches!(Styles::from_json_str(&json), Err(StylesError::Invalid { ref token, .. }) if token == "t"),
            "{bad}"
        );
    }
}
