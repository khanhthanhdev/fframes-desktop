use sha2::{Digest, Sha256};
use studio_project::{
    ProjectPath, SourceRevision,
    revision::FileKind,
    source_index::{
        MAX_INDEX_FILE_BYTES, SourceAnchor, SourceIndex, SourceIndexError, SourceIndexInput,
    },
};

fn input(path: &str, text: &str) -> SourceIndexInput {
    let bytes = text.as_bytes().to_vec();
    SourceIndexInput {
        path: ProjectPath::try_from(path.to_owned()).unwrap(),
        kind: FileKind::Rust,
        expected_sha256: format!("{:x}", Sha256::digest(&bytes)),
        expected_size: bytes.len() as u64,
        bytes,
    }
}

fn revision() -> SourceRevision {
    "f".repeat(64).try_into().unwrap()
}

#[test]
fn syntax_spans_are_hash_bound_utf8_and_deterministic() {
    let text = r##"
// render_title() in this comment is not indexed as a call.
const LABEL: &str = r#"render_title()"#;
fn render_title() {
    let label = "café";
    helper(); /* selected title anchor */
    let _ = label;
}
fn helper() {}
"##;
    let file = input("src/video.rs", text);
    let anchor = SourceAnchor {
        path: file.path.clone(),
        symbol: "render_title".into(),
        expected_sha256: file.expected_sha256.clone(),
        marker: Some("selected title anchor".into()),
    };
    let first = SourceIndex::build(revision(), vec![file.clone()], &|| false).unwrap();
    let second = SourceIndex::build(revision(), vec![file], &|| false).unwrap();
    let result = first.lookup(&anchor).unwrap();
    assert_eq!(result, second.lookup(&anchor).unwrap());
    assert_eq!(result.snippets[0].confidence, "explicit_marker");
    assert_eq!(
        &text[result.snippets[0].anchor_span.start..result.snippets[0].anchor_span.end],
        "selected title anchor"
    );
    assert!(text.is_char_boundary(result.snippets[0].anchor_span.start));
    assert!(text.is_char_boundary(result.snippets[0].anchor_span.end));
    assert!(result.snippets[0].text.contains("fn render_title"));
    assert!(
        result
            .helper_candidates
            .iter()
            .any(|candidate| candidate.ends_with("helper"))
    );
    assert!(
        result
            .snippets
            .iter()
            .any(|snippet| snippet.text.contains("fn helper"))
    );
}

#[test]
fn method_anchor_returns_containing_impl_and_exact_marker_ranges() {
    let text = "struct Video;\nimpl Video {\n    fn render_frame(&self) {\n        // studio-title-source-anchor\n        draw_title();\n    }\n    fn other(&self) {}\n}\nfn draw_title() {}\n";
    let file = input("src/lib.rs", text);
    let result = SourceIndex::build(revision(), vec![file.clone()], &|| false)
        .unwrap()
        .lookup(&SourceAnchor {
            path: file.path,
            symbol: "render_frame".into(),
            expected_sha256: file.expected_sha256,
            marker: Some("studio-title-source-anchor".into()),
        })
        .unwrap();
    assert_eq!(
        &text[result.snippets[0].anchor_span.start..result.snippets[0].anchor_span.end],
        "studio-title-source-anchor"
    );
    assert!(result.snippets[0].text.contains("fn render_frame"));
    assert!(result.snippets.iter().any(|snippet| {
        snippet.confidence == "containing_impl"
            && snippet.text.contains("fn other")
            && snippet.anchor_span.start > snippet.span.start
    }));
    assert!(
        result
            .snippets
            .iter()
            .any(|snippet| snippet.text.contains("fn draw_title"))
    );
}

#[test]
fn duplicate_marker_and_hash_mismatch_refuse_exact_lookup() {
    let file = input(
        "src/lib.rs",
        "fn draw() { let _ = \"MARKER\"; let _ = \"MARKER\"; }",
    );
    let index = SourceIndex::build(revision(), vec![file.clone()], &|| false).unwrap();
    let anchor = SourceAnchor {
        path: file.path.clone(),
        symbol: "draw".into(),
        expected_sha256: file.expected_sha256.clone(),
        marker: Some("MARKER".into()),
    };
    assert!(matches!(
        index.lookup(&anchor),
        Err(SourceIndexError::InvalidAnchor(message)) if message.contains("exactly once")
    ));

    let stale = SourceAnchor {
        expected_sha256: "0".repeat(64),
        marker: None,
        ..anchor
    };
    assert!(matches!(
        index.lookup(&stale),
        Err(SourceIndexError::HashMismatch(_))
    ));
}

#[test]
fn ambiguity_and_file_budgets_are_reported_instead_of_claiming_resolution() {
    let a = input(
        "src/a.rs",
        "mod one { pub fn helper() {} } fn render() { helper(); }",
    );
    let b = input("src/b.rs", "mod two { pub fn helper() {} }");
    let index = SourceIndex::build(revision(), vec![b, a.clone()], &|| false).unwrap();
    let result = index
        .lookup(&SourceAnchor {
            path: a.path,
            symbol: "render".into(),
            expected_sha256: a.expected_sha256,
            marker: None,
        })
        .unwrap();
    assert!(result.ambiguous);
    assert!(
        result
            .helper_candidates
            .iter()
            .any(|candidate| candidate.contains("ambiguous"))
    );

    let huge = input("src/huge.rs", &" ".repeat(MAX_INDEX_FILE_BYTES + 1));
    let omitted = SourceIndex::build(revision(), vec![huge], &|| false).unwrap();
    assert_eq!(omitted.truncated_files, 1);
    assert!(omitted.truncated_bytes > MAX_INDEX_FILE_BYTES);
}

#[test]
fn malformed_syntax_and_cancelled_build_are_bounded_diagnostics() {
    let malformed = input("src/broken.rs", "fn broken( {");
    let index = SourceIndex::build(revision(), vec![malformed], &|| false).unwrap();
    assert_eq!(index.diagnostics.len(), 1);
    assert!(index.diagnostics[0].message.contains("syntax unavailable"));

    let canceled = SourceIndex::build(revision(), vec![], &|| true);
    assert!(canceled.is_ok());
    let canceled = SourceIndex::build(
        revision(),
        vec![input("src/lib.rs", "fn main() {}")],
        &|| true,
    );
    assert_eq!(canceled.unwrap_err(), SourceIndexError::Cancelled);
}

#[test]
fn syntax_spans_count_unicode_scalars_while_returning_utf8_byte_ranges() {
    let text = "const LABEL: &str = \"café\"; fn render() { let _ = \"naïve\"; helper(); }\nfn helper() {}\n";
    let file = input("src/unicode.rs", text);
    let lookup = SourceIndex::build(revision(), vec![file.clone()], &|| false)
        .unwrap()
        .lookup(&SourceAnchor {
            path: file.path,
            symbol: "render".into(),
            expected_sha256: file.expected_sha256,
            marker: None,
        })
        .unwrap();

    assert_eq!(
        lookup.snippets[0].text,
        "fn render() { let _ = \"naïve\"; helper(); }"
    );
    assert!(
        lookup
            .helper_candidates
            .iter()
            .any(|candidate| candidate.ends_with("helper"))
    );
}

#[test]
fn bounded_symbol_snippet_moves_its_window_to_a_late_source_marker() {
    let padding = "x".repeat(8 * 1024);
    let text = format!("fn render() {{ let _padding = \"{padding}\"; // late-selection-anchor\n}}");
    let file = input("src/long.rs", &text);
    let lookup = SourceIndex::build(revision(), vec![file.clone()], &|| false)
        .unwrap()
        .lookup(&SourceAnchor {
            path: file.path,
            symbol: "render".into(),
            expected_sha256: file.expected_sha256,
            marker: Some("late-selection-anchor".into()),
        })
        .unwrap();

    let snippet = &lookup.snippets[0];
    assert!(lookup.truncated);
    assert!(snippet.text.contains("late-selection-anchor"));
    assert!(snippet.span.start > 0);
    assert_eq!(
        &text[snippet.anchor_span.start..snippet.anchor_span.end],
        "late-selection-anchor"
    );
}

#[test]
fn calls_belong_only_to_their_enclosing_function() {
    let text = r#"
fn render() {
    fn nested() { nested_helper(); }
    outer_helper();
}
const VALUE: usize = const_helper();
static OTHER: usize = static_helper();
fn nested_helper() {}
fn outer_helper() {}
fn const_helper() -> usize { 1 }
fn static_helper() -> usize { 2 }
"#;
    let file = input("src/calls.rs", text);
    let index = SourceIndex::build(revision(), vec![file.clone()], &|| false).unwrap();
    let lookup = |symbol: &str| {
        index
            .lookup(&SourceAnchor {
                path: file.path.clone(),
                symbol: symbol.into(),
                expected_sha256: file.expected_sha256.clone(),
                marker: None,
            })
            .unwrap()
    };

    let outer = lookup("render");
    assert!(
        outer
            .helper_candidates
            .iter()
            .any(|candidate| candidate.ends_with("outer_helper"))
    );
    assert!(
        !outer
            .helper_candidates
            .iter()
            .any(|candidate| candidate.ends_with("nested_helper"))
    );
    assert!(
        lookup("nested")
            .helper_candidates
            .iter()
            .any(|candidate| candidate.ends_with("nested_helper"))
    );
    assert!(lookup("VALUE").helper_candidates.is_empty());
    assert!(lookup("OTHER").helper_candidates.is_empty());
}

#[test]
fn methods_inside_modules_retrieve_their_containing_impl() {
    let text = "mod video { struct Player; impl Player { fn render_frame(&self) { draw(); } fn other(&self) {} } } fn draw() {}\n";
    let file = input("src/nested_impl.rs", text);
    let lookup = SourceIndex::build(revision(), vec![file.clone()], &|| false)
        .unwrap()
        .lookup(&SourceAnchor {
            path: file.path,
            symbol: "render_frame".into(),
            expected_sha256: file.expected_sha256,
            marker: None,
        })
        .unwrap();

    assert!(lookup.snippets.iter().any(|snippet| {
        snippet.confidence == "containing_impl" && snippet.text.contains("fn other")
    }));
}
