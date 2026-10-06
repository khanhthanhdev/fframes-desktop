use studio_presets::{CssReport, CssStatus, Design, TokenValue, import_css};

fn run(css: &str) -> CssReport {
    import_css(css, &Design::HD)
}
fn entry<'a>(r: &'a CssReport, property: &str) -> &'a studio_presets::CssEntry {
    r.entries
        .iter()
        .find(|e| e.property == property)
        .unwrap_or_else(|| panic!("no entry for {property}:\n{}", r.to_text()))
}
fn token(r: &CssReport, name: &str) -> Option<TokenValue> {
    r.tokens()
        .resolved_values()
        .into_iter()
        .find(|(k, _)| k.as_str() == name)
        .map(|(_, v)| v)
}

const SUPPORTED: &str = r#"
/* brand */
:root {
  --color-accent: #b3261e;
  --color-glass: #0008;
  --spacing-md: 1.5rem;
  --spacing-raw: 12;
  --radius-sm: 4px;
  --stroke-thin: 2px;
  --shadow-card: 0 8px 24px #00000066;
  --motion-duration-fast: 250ms;
  --motion-duration-base: 0.5s;
  --motion-stagger-tight: 80ms;
  --motion-easing-standard: cubic-bezier(0.4, 0, 0.2, 1);
  --motion-easing-flat: linear;
  --typography-title-family: "DM Sans";
  --typography-title-size: 6rem;
  --typography-title-weight: 700;
  --typography-title-line-height: 1.1;
  --typography-title-letter-spacing: -1px;
}
"#;

#[test]
fn supported_subset_normalizes_exactly() {
    let r = run(SUPPORTED);
    assert_eq!(r.count(CssStatus::Accepted), 17, "{}", r.to_text());
    assert_eq!(
        r.count(CssStatus::Unsupported) + r.count(CssStatus::Rejected),
        0,
        "{}",
        r.to_text()
    );
    assert_eq!(
        entry(&r, "--color-accent").normalized.as_deref(),
        Some("#B3261E")
    );
    assert_eq!(
        entry(&r, "--color-glass").normalized.as_deref(),
        Some("#00000088")
    );
    assert_eq!(
        entry(&r, "--spacing-md").normalized.as_deref(),
        Some("24.0")
    );
    assert_eq!(
        entry(&r, "--motion-duration-fast").normalized.as_deref(),
        Some("0.25s")
    );
    assert_eq!(
        entry(&r, "--spacing-md").token.as_deref(),
        Some("spacing.md")
    );
    assert_eq!(entry(&r, "--spacing-md").line, 6);
    assert_eq!(token(&r, "spacing.raw"), Some(TokenValue::Dimension(12.0)));
    assert_eq!(
        token(&r, "motion.duration.base"),
        Some(TokenValue::Duration(0.5))
    );
    assert_eq!(
        token(&r, "motion.stagger.tight"),
        Some(TokenValue::Duration(0.08))
    );
    match token(&r, "typography.title").unwrap() {
        TokenValue::Typography(t) => {
            assert_eq!(
                (
                    t.family.as_str(),
                    t.size,
                    t.weight,
                    t.line_height,
                    t.letter_spacing
                ),
                ("DM Sans", 96.0, 700, 1.1, -1.0)
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(token(&r, "shadow.card"), Some(TokenValue::Shadow(s)) if s.dy == 8.0 && s.blur == 24.0)
    );
    assert_eq!(r.tokens().len(), 13);
}

#[test]
fn rem_uses_the_explicit_design_context() {
    let css = ":root { --spacing-md: 2rem; }";
    let wide = Design {
        base_font_px: 20.0,
        ..Design::HD
    };
    let r = import_css(css, &wide);
    assert_eq!(token(&r, "spacing.md"), Some(TokenValue::Dimension(40.0)));
    let bad = Design {
        base_font_px: 0.0,
        ..Design::HD
    };
    let r = import_css(css, &bad);
    assert_eq!(r.count(CssStatus::Rejected), 1);
    assert!(r.tokens().is_empty());
}

#[test]
fn bare_declaration_blocks_are_read_too() {
    let r = run("--color-accent: #fff; --spacing-md: 8px");
    assert_eq!(r.count(CssStatus::Accepted), 2);
    assert!(token(&r, "color.accent").is_some());
}

#[test]
fn every_declaration_is_reported_and_only_allowlisted_forms_become_tokens() {
    let css = r#":root {
  --color-a: rgb(1, 2, 3);
  --color-b: red;
  --color-c: #12;
  --color-d: var(--color-accent);
  --spacing-a: calc(1rem + 2px);
  --spacing-b: 2em;
  --spacing-c: 10%;
  --spacing-d: 5px !important;
  --spacing-e: 1px 2px;
  --spacing-f: url(x.png);
  --spacing-g: 100001px;
  --motion-duration-a: 250;
  --motion-duration-b: 2px;
  --motion-duration-c: 4000s;
  --motion-easing-a: ease;
  --motion-easing-b: cubic-bezier(2, 0, 0, 1);
  --motion-easing-c: linear-gradient(red, blue);
  --shadow-a: 0 1px 2px 3px #000;
  --shadow-b: inset 0 1px #000;
  --shadow-c: 0 1px red;
  --shadow-d: 0 1px 2px #000, 0 2px 4px #000;
  --unknown-thing: 4px;
  --Color-Upper: #fff;
  --color-ok: #fff;
  color: red;
  margin: 0 auto;
}
.card { --color-z: #fff; padding: 4px; }
@media (min-width: 600px) { :root { --color-m: #000; } }
@import url("x.css");
:root { --color-late: #000; }
"#;
    let r = run(css);
    // 25 :root custom properties + 2 ordinary properties + selector + decl + @media + nested root + decl + @import
    // + second :root block (which is not the document-level first one but is still :root)
    let text = r.to_text();
    for property in [
        "--color-a",
        "--color-b",
        "--color-c",
        "--color-d",
        "--spacing-a",
        "--spacing-b",
        "--spacing-c",
        "--spacing-d",
        "--spacing-e",
        "--spacing-f",
        "--spacing-g",
        "--motion-duration-a",
        "--motion-duration-b",
        "--motion-duration-c",
        "--motion-easing-a",
        "--motion-easing-b",
        "--motion-easing-c",
        "--shadow-a",
        "--shadow-b",
        "--shadow-c",
        "--shadow-d",
        "--unknown-thing",
        "--Color-Upper",
        "--color-ok",
        "color",
        "margin",
        ".card",
        "--color-z",
        "padding",
        "@media (min-width: 600px)",
        "--color-m",
        "--color-late",
    ] {
        entry(&r, property);
    }
    assert!(
        r.entries
            .iter()
            .any(|e| e.property == "@import url(\"x.css\")" || e.property.starts_with("@import")),
        "{text}"
    );
    let accepted: Vec<&str> = r
        .entries
        .iter()
        .filter(|e| e.status == CssStatus::Accepted)
        .map(|e| e.property.as_str())
        .collect();
    assert_eq!(accepted, ["--color-ok", "--color-late"], "{text}");
    assert_eq!(r.tokens().len(), 2);
    // Reasons are specific.
    assert!(entry(&r, "--color-a").reason.contains("rgb()"));
    assert_eq!(entry(&r, "--color-a").status, CssStatus::Unsupported);
    assert!(entry(&r, "--color-b").reason.contains("hex"));
    assert_eq!(entry(&r, "--color-c").status, CssStatus::Rejected);
    assert!(entry(&r, "--color-d").reason.contains("var()"));
    assert!(entry(&r, "--spacing-a").reason.contains("calc()"));
    assert!(entry(&r, "--spacing-b").reason.contains("unit `em`"));
    assert!(entry(&r, "--spacing-c").reason.contains("unit `%`"));
    assert_eq!(entry(&r, "--spacing-d").status, CssStatus::Unsupported);
    assert!(entry(&r, "--spacing-d").reason.contains("!important"));
    assert_eq!(entry(&r, "--spacing-e").status, CssStatus::Rejected);
    assert!(entry(&r, "--spacing-f").reason.contains("url()"));
    assert_eq!(entry(&r, "--spacing-g").status, CssStatus::Rejected);
    assert_eq!(
        entry(&r, "--motion-duration-a").status,
        CssStatus::Unsupported
    );
    assert_eq!(
        entry(&r, "--motion-duration-b").status,
        CssStatus::Unsupported
    );
    assert_eq!(entry(&r, "--motion-duration-c").status, CssStatus::Rejected);
    assert_eq!(
        entry(&r, "--motion-easing-a").status,
        CssStatus::Unsupported
    );
    assert_eq!(entry(&r, "--motion-easing-b").status, CssStatus::Rejected);
    assert!(
        entry(&r, "--motion-easing-c")
            .reason
            .contains("linear-gradient()")
    );
    assert_eq!(entry(&r, "--shadow-a").status, CssStatus::Unsupported);
    assert_eq!(entry(&r, "--shadow-b").status, CssStatus::Unsupported);
    assert_eq!(entry(&r, "--shadow-c").status, CssStatus::Unsupported);
    assert_eq!(entry(&r, "--shadow-d").status, CssStatus::Unsupported);
    assert_eq!(entry(&r, "--unknown-thing").token, None);
    assert!(entry(&r, "--unknown-thing").reason.contains("documented"));
    assert!(entry(&r, "--Color-Upper").reason.contains("documented"));
    assert!(entry(&r, "color").reason.contains("ordinary"));
    assert!(entry(&r, "--color-z").reason.contains(".card"));
    assert!(entry(&r, "--color-m").reason.contains("@media"));
    assert_eq!(entry(&r, ".card").status, CssStatus::Unsupported);
    // Line numbers point at the source.
    assert_eq!(entry(&r, "--color-a").line, 2);
    assert_eq!(entry(&r, "--color-ok").line, 25);
    assert_eq!(entry(&r, ".card").line, 29);
}

#[test]
fn typography_groups_need_every_valid_field() {
    let r = run(
        ":root { --typography-t-family: \"DM Sans\"; --typography-t-size: 40px; --typography-t-weight: 400; }",
    );
    assert!(r.tokens().is_empty());
    let e = entry(&r, "--typography-t-size");
    assert_eq!(e.status, CssStatus::Rejected);
    assert!(
        e.reason
            .contains("missing --typography-t-line-height, --typography-t-letter-spacing"),
        "{}",
        e.reason
    );
    assert_eq!(e.token.as_deref(), Some("typography.t"));

    let bad = run(
        ":root { --typography-t-family: \"A\", sans-serif; --typography-t-size: 40; --typography-t-weight: bold;
                  --typography-t-line-height: 1.2; --typography-t-letter-spacing: 0; }",
    );
    assert!(bad.tokens().is_empty());
    assert_eq!(
        entry(&bad, "--typography-t-family").status,
        CssStatus::Unsupported
    );
    assert!(
        entry(&bad, "--typography-t-family")
            .reason
            .contains("fallback")
    );
    assert_eq!(
        entry(&bad, "--typography-t-weight").status,
        CssStatus::Unsupported
    );
    // accepted siblings are demoted with the reason
    assert_eq!(
        entry(&bad, "--typography-t-size").status,
        CssStatus::Rejected
    );
    assert!(
        entry(&bad, "--typography-t-size")
            .reason
            .contains("not accepted")
    );

    let unquoted = run(":root { --typography-t-family: DM Sans; }");
    assert_eq!(unquoted.entries[0].status, CssStatus::Rejected);
}

#[test]
fn duplicates_are_rejected_without_evaluating_the_cascade() {
    let r = run(":root { --color-a: #111; --color-a: #222; }");
    assert_eq!(r.entries[0].status, CssStatus::Accepted);
    assert_eq!(r.entries[1].status, CssStatus::Rejected);
    assert!(r.entries[1].reason.contains("duplicate"));
    assert_eq!(
        token(&r, "color.a"),
        Some(TokenValue::Color(
            studio_presets::Color::parse_hex("#111").unwrap()
        ))
    );
}

#[test]
fn malformed_and_oversized_input_is_reported_not_panicked() {
    for css in [
        "",
        "   ",
        ":root {",
        ":root { --color-a: #fff",
        "} }",
        "/* open",
        "--color-a #fff;",
        ":root { --color-a: \"unterminated; }",
        ";;;",
        "{",
        "a{b{c{d{e{f{g{h{i{j{k{l{--color-a:#fff}}}}}}}}}}}}",
    ] {
        let r = run(css);
        let _ = r.to_text();
        assert!(r.tokens().len() <= 1, "{css}");
    }
    let r = run("--color-a #fff;");
    assert_eq!(r.entries[0].status, CssStatus::Rejected);
    assert_eq!(run(":root {").count(CssStatus::Rejected), 1);
    assert_eq!(run("/* open").count(CssStatus::Rejected), 1);
    assert_eq!(run("} }").count(CssStatus::Rejected), 2);

    let big = format!(":root {{ {} }}", "--color-a: #fff;".repeat(20_000));
    let r = run(&big);
    assert_eq!(r.entries.len(), 1);
    assert_eq!(r.entries[0].status, CssStatus::Rejected);
    assert!(r.tokens().is_empty());

    let many = format!(
        ":root {{ {} }}",
        (0..3000)
            .map(|i| format!("--color-c{i}: #fff;"))
            .collect::<String>()
    );
    let r = run(&many);
    assert_eq!(
        r.entries.len(),
        2049,
        "2048 reported declarations plus one overflow notice"
    );
    assert_eq!(r.count(CssStatus::Accepted), 512);
    assert_eq!(r.tokens().len(), 512);
    assert_eq!(r.count(CssStatus::Rejected), 1537);
    assert!(r.entries.last().unwrap().reason.contains("not reported"));
}

#[test]
fn a_semicolon_or_brace_inside_a_string_or_parens_does_not_split_declarations() {
    let r = run(
        ":root { --typography-t-family: \"A;B{\"; --spacing-a: url(data:image/png;base64,AAA); --color-a: #fff; }",
    );
    assert_eq!(r.entries.len(), 3, "{}", r.to_text());
    assert_eq!(entry(&r, "--typography-t-family").value, "\"A;B{\"");
    assert_eq!(entry(&r, "--spacing-a").status, CssStatus::Unsupported);
}
