mod common;

use std::collections::BTreeSet;

use studio_presets::{OverridesFile, Package, TokenValue, builtin, materialize};

fn packages() -> &'static [Package] {
    builtin::packages().unwrap_or_else(|e| panic!("{e}"))
}

fn token(p: &Package, name: &str) -> TokenValue {
    p.snapshot(None, None)
        .get(name)
        .unwrap_or_else(|| panic!("{} lacks {name}", p.id()))
        .clone()
}

#[test]
fn exactly_three_validated_bundles_with_stable_ids() {
    let ids: Vec<&str> = packages().iter().map(Package::id).collect();
    assert_eq!(ids, ["editorial", "pulse", "quiet-motion"]);
    assert_eq!(ids, builtin::IDS);
    assert_eq!(packages().len(), 3);
    for id in ids {
        assert_eq!(builtin::get(id).unwrap().id(), id);
    }
    assert!(builtin::get("nope").is_none());
    let hashes: BTreeSet<&str> = packages().iter().map(Package::hash).collect();
    assert_eq!(hashes.len(), 3);
}

#[test]
fn bundles_share_one_semantic_vocabulary_but_differ_in_typography_layout_and_motion() {
    let names = |p: &Package| -> Vec<String> {
        p.snapshot(None, None)
            .tokens()
            .keys()
            .map(|k| k.as_str().to_owned())
            .collect()
    };
    let first = names(&packages()[0]);
    assert_eq!(first.len(), 40);
    for p in packages() {
        assert_eq!(names(p), first, "{} vocabulary", p.id());
    }
    let distinct = |name: &str| -> usize {
        packages()
            .iter()
            .map(|p| format!("{:?}", token(p, name)))
            .collect::<BTreeSet<_>>()
            .len()
    };
    // typography
    for t in [
        "typography.title",
        "typography.heading",
        "typography.body",
        "typography.caption",
    ] {
        assert_eq!(distinct(t), 3, "{t}");
    }
    // layout
    for t in [
        "spacing.margin",
        "spacing.lg",
        "spacing.xl",
        "radius.md",
        "stroke.regular",
        "shadow.card",
    ] {
        assert_eq!(distinct(t), 3, "{t}");
    }
    // motion
    for t in [
        "motion.duration.base",
        "motion.duration.slow",
        "motion.easing.standard",
        "motion.easing.enter",
        "motion.stagger.tight",
    ] {
        assert_eq!(distinct(t), 3, "{t}");
    }
    for t in ["color.background", "color.accent"] {
        assert_eq!(distinct(t), 3, "{t}");
    }
    // Pulse is the only spring-based preset.
    let springs: Vec<&str> = packages()
        .iter()
        .filter(|p| {
            matches!(
                token(p, "motion.easing.standard"),
                TokenValue::Easing(studio_presets::Easing::Spring { .. })
            )
        })
        .map(Package::id)
        .collect();
    assert_eq!(springs, ["pulse"]);
}

#[test]
fn every_bundled_resource_has_a_matching_license_notice_and_aliases_are_used() {
    for p in packages() {
        let m = p.manifest();
        let notices: BTreeSet<_> = m.licenses.iter().map(|n| n.file.path.clone()).collect();
        assert!(notices.contains(&m.license));
        assert!(!m.fonts.is_empty());
        for f in &m.fonts {
            assert!(
                notices.contains(&f.license),
                "{} font {}",
                p.id(),
                f.file.path.as_str()
            );
            assert_eq!(
                m.licenses
                    .iter()
                    .find(|n| n.file.path == f.license)
                    .unwrap()
                    .spdx,
                "OFL-1.1"
            );
        }
        for e in m.examples.iter().chain(&m.assets) {
            assert!(notices.contains(&e.license));
        }
        // semantic aliases exist in the authored set
        let text = String::from_utf8(p.files()[&m.tokens.path].clone()).unwrap();
        assert!(text.contains("\"alias\""), "{}", p.id());
        // everything on disk is accounted for: manifest + declared files
        assert_eq!(
            p.files().len(),
            1 + 2 + m.licenses.len() + m.fonts.len() + m.assets.len() + m.examples.len()
        );
    }
}

#[test]
fn bundled_fonts_match_the_repository_originals() {
    let medium = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/annotated-video-overlay/media/DMSans-Medium.ttf"
    ))
    .unwrap();
    let ofl = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/annotated-video-overlay/media/OFL.txt"
    ))
    .unwrap();
    for id in ["editorial", "pulse"] {
        let p = builtin::get(id).unwrap();
        assert_eq!(p.files()[&common::p("fonts/DMSans-Medium.ttf")], medium);
        let bundled_ofl =
            String::from_utf8(p.files()[&common::p("licenses/DMSans-OFL.txt")].clone()).unwrap();
        let repository_ofl = String::from_utf8(ofl.clone()).unwrap();
        assert_eq!(
            bundled_ofl.lines().map(str::trim_end).collect::<Vec<_>>(),
            repository_ofl
                .lines()
                .map(str::trim_end)
                .collect::<Vec<_>>(),
            "the bundled OFL notice preserves its text while normalizing whitespace"
        );
    }
}

#[test]
fn materialized_project_files_are_deterministic_and_portable() {
    let package = builtin::get("quiet-motion").unwrap();
    let a = materialize(package, &OverridesFile::default()).unwrap();
    let b = materialize(package, &OverridesFile::default()).unwrap();
    assert_eq!(a.files, b.files);
    let paths: Vec<&str> = a.files.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        [
            "media/preset-fonts-DMSans-Regular.ttf",
            "style/guide.md",
            "style/overrides.json",
            "style/preset-tokens.json",
            "style/preset.json",
            "style/preset/LICENSE.txt",
            "style/preset/examples/title-card.md",
            "style/preset/licenses/DMSans-OFL.txt",
            "style/tokens.json",
        ]
    );
    assert_eq!(a.identity.hash, package.hash());
    assert_eq!(a.identity.tokens_hash, a.snapshot.hash());
    let tokens = &a
        .files
        .iter()
        .find(|(p, _)| p.as_str() == "style/tokens.json")
        .unwrap()
        .1;
    assert_eq!(tokens, a.snapshot.runtime_tokens_json().as_bytes());
    // Switching presets changes the token file while the source-facing vocabulary stays the same.
    let other = materialize(builtin::get("pulse").unwrap(), &OverridesFile::default()).unwrap();
    assert_ne!(
        other.snapshot.runtime_tokens_json(),
        a.snapshot.runtime_tokens_json()
    );
    assert_eq!(
        other.snapshot.tokens().keys().collect::<Vec<_>>(),
        a.snapshot.tokens().keys().collect::<Vec<_>>()
    );
}

#[test]
fn overrides_survive_reapplication_and_orphans_are_reported() {
    let mut overrides = OverridesFile::default();
    let accent = studio_presets::TokenName::new("color.accent").unwrap();
    overrides.project_mut().insert(
        accent.clone(),
        studio_presets::TokenDef::Literal(TokenValue::Color(
            studio_presets::Color::parse_hex("#010203").unwrap(),
        )),
    );
    overrides.project_mut().insert(
        studio_presets::TokenName::new("color.legacy").unwrap(),
        studio_presets::TokenDef::Literal(TokenValue::Color(
            studio_presets::Color::parse_hex("#FFFFFF").unwrap(),
        )),
    );
    for id in studio_presets::builtin::IDS {
        let style = materialize(builtin::get(id).unwrap(), &overrides).unwrap();
        assert_eq!(
            style.snapshot.get("color.accent"),
            overrides.project().defs().get(&accent).map(|d| match d {
                studio_presets::TokenDef::Literal(v) => v,
                _ => unreachable!(),
            })
        );
        assert_eq!(style.snapshot.diagnostics().len(), 1);
        let kept = &style
            .files
            .iter()
            .find(|(p, _)| p.as_str() == "style/overrides.json")
            .unwrap()
            .1;
        assert!(
            String::from_utf8_lossy(kept).contains("color.legacy"),
            "orphan retained in file"
        );
    }
}
