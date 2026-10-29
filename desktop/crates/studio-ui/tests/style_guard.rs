//! Style guard: screens style themselves through the kit and the theme's roles only.
//!
//! The test reads the sources of `studio-ui/src`, counts forbidden styling patterns per file and
//! compares the counts with an allowlist that may only shrink while the design-system migration
//! runs (`plans/261027-2045-studio-design-system/phase-00-baseline-and-guard.md`, section 0.3).
//!
//! - `screens_style_through_the_kit`: every `.rs` under `src/` except `kit/`, checked against
//!   `tests/style_guard_allowlist.txt`.
//! - `kit_paints_with_theme_roles`: `kit/` may not add colour literals or Radix steps, checked
//!   against `tests/style_guard_kit_allowlist.txt`.
//!
//! Comments and the contents of string literals are ignored, and so are `#[cfg(test)]` modules
//! (inline or in their own file) and inline `mod tests { .. }` blocks.
//!
//! A count above its entry, or a hit in a file that is not listed, fails. A count below its entry
//! is printed with the lower number (`cargo test -p studio-ui --test style_guard -- --nocapture`).
//!
//! Regenerate an allowlist from the tree with `STYLE_GUARD_WRITE=1`; the comment header is kept.
//! Regeneration refuses to raise any count, or to add an entry, unless
//! `STYLE_GUARD_ALLOW_INCREASE=1` is set as well.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// How a pattern is found in comment-free, string-free source.
#[derive(Clone, Copy)]
enum Matcher {
    /// The text, not preceded by an identifier character.
    Word(&'static str),
    /// A method call: the text, then an identifier, then `(` (`.settings_text(`, not the field
    /// `.settings_window`).
    MethodCall(&'static str),
    /// The text, then optional spaces and `-`, then a digit (`px(12.)`, `opacity(0.5)`).
    BeforeDigit(&'static str),
}

struct Pattern {
    /// The name in the allowlist; contains no whitespace.
    name: &'static str,
    matcher: Matcher,
}

const fn word(name: &'static str) -> Pattern {
    Pattern {
        name,
        matcher: Matcher::Word(name),
    }
}

/// Raw colours and Radix steps; forbidden in screens and in the kit.
const COLOUR_PATTERNS: &[Pattern] = &[
    word("gray("),
    word(".gray_"),
    word(".blue_"),
    word(".red_"),
    word(".amber_"),
    word(".green_"),
    word("rgb("),
    word("rgba("),
    word("hsla("),
    // `black()` and `white()` imported from gpui count as well.
    Pattern {
        name: "gpui::black()",
        matcher: Matcher::Word("black()"),
    },
    Pattern {
        name: "gpui::white()",
        matcher: Matcher::Word("white()"),
    },
    word("SETTINGS_ACCENT"),
];

/// Surface helpers, type and size literals; forbidden in screens.
const SCREEN_PATTERNS: &[Pattern] = &[
    Pattern {
        name: ".settings_",
        matcher: Matcher::MethodCall(".settings_"),
    },
    Pattern {
        name: ".body_",
        matcher: Matcher::MethodCall(".body_"),
    },
    word("header_bg"),
    word("header_border"),
    word("text_size("),
    word("text_xs("),
    word("text_sm("),
    word("font_weight("),
    Pattern {
        name: "px(<digit>",
        matcher: Matcher::BeforeDigit("px("),
    },
    word("rounded(px("),
    Pattern {
        name: "opacity(<literal>",
        matcher: Matcher::BeforeDigit("opacity("),
    },
];

const PX_DIGIT: &str = "px(<digit>";

/// Files where `px(` literals are allowed inside `const` items (painters with named sizes).
fn px_in_consts_allowed(path: &str) -> bool {
    path.starts_with("timeline/") || path == "agent/changes/paint.rs"
}

const SCREEN_HEADER: &str = "\
# Style guard allowlist for studio-ui/src outside kit/ (tests/style_guard.rs).
# Format: <path under src/> <pattern> <count>. Counts may only go down; a new file gets no entry.
# Regenerate: STYLE_GUARD_WRITE=1 cargo test --manifest-path desktop/Cargo.toml -p studio-ui --test style_guard
# (refuses to raise a count unless STYLE_GUARD_ALLOW_INCREASE=1 is set too).
#
# Patterns: colours gray( .gray_ .blue_ .red_ .amber_ .green_ rgb( rgba( hsla( gpui::black()
# gpui::white() SETTINGS_ACCENT; surfaces .settings_ .body_ (method calls) header_bg header_border;
# type text_size( text_xs( text_sm( font_weight(; sizes px(<digit> rounded(px( opacity(<literal>.
# Comments, string contents, #[cfg(test)] modules and `mod tests` blocks are not counted.
#
# Exceptions by design:
# - timeline/ (painters) and agent/changes/paint.rs: px(<digit> inside `const` items is not
#   counted, so named painter sizes stay; px( literals in code are counted.
# - agent/syntax/ may use the theme's `syntax` colour set (phase 1); those role names match no
#   pattern here.
";

const KIT_HEADER: &str = "\
# Style guard allowlist for studio-ui/src/kit (tests/style_guard.rs).
# Format: <path under src/> <pattern> <count>. Counts may only go down; a new file gets no entry.
# Regenerate: STYLE_GUARD_WRITE=1 cargo test --manifest-path desktop/Cargo.toml -p studio-ui --test style_guard
# (refuses to raise a count unless STYLE_GUARD_ALLOW_INCREASE=1 is set too).
#
# Patterns: colour literals and Radix steps: gray( .gray_ .blue_ .red_ .amber_ .green_ rgb( rgba(
# hsla( gpui::black() gpui::white() SETTINGS_ACCENT. Comments, string contents and #[cfg(test)]
# modules are not counted.
#
# Exceptions by design: theme plumbing (tone_color, tone_surface and the surface palettes) maps
# roles to steps until phase 1 makes the steps private; those entries go when it lands.
# px( in the kit is migrated in phase 2 and guarded from then on.
";

type Counts = BTreeMap<(String, String), usize>;

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// `path` relative to `src/`, with forward slashes.
fn relative(path: &Path) -> String {
    path.strip_prefix(src_dir())
        .expect("a file under src/")
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The source with comments and the contents of string and char literals replaced by spaces.
/// Line breaks are kept, so line numbers stay.
fn strip_comments_and_strings(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let prev_ident = i > 0 && is_ident(chars[i - 1]);
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[i]));
                    i += 1;
                }
            }
        } else if !prev_ident
            && (c == 'r' || (c == 'b' && next == Some('r')))
            && raw_string_start(&chars, if c == 'b' { i + 1 } else { i }).is_some()
        {
            let start = if c == 'b' { i + 1 } else { i };
            let hashes = raw_string_start(&chars, start).expect("checked");
            // Keep the prefix and quote, blank the contents.
            for &ch in &chars[i..start + 2 + hashes] {
                out.push(ch);
            }
            i = start + 2 + hashes;
            loop {
                if i >= chars.len() {
                    break;
                }
                if chars[i] == '"' && (0..hashes).all(|k| chars.get(i + 1 + k) == Some(&'#')) {
                    out.push('"');
                    out.extend(std::iter::repeat_n('#', hashes));
                    i += 1 + hashes;
                    break;
                }
                out.push(blank(chars[i]));
                i += 1;
            }
        } else if c == '"' {
            out.push('"');
            i += 1;
            while i < chars.len() {
                match chars[i] {
                    '\\' => {
                        out.push(' ');
                        if let Some(&escaped) = chars.get(i + 1) {
                            out.push(blank(escaped));
                        }
                        i += 2;
                    }
                    '"' => {
                        out.push('"');
                        i += 1;
                        break;
                    }
                    other => {
                        out.push(blank(other));
                        i += 1;
                    }
                }
            }
        } else if c == '\'' {
            // A char literal ('x', '\n', '\u{..}'), or a lifetime / label ('a).
            if next == Some('\\') {
                out.push('\'');
                i += 1;
                while i < chars.len() {
                    match chars[i] {
                        '\\' => {
                            out.push_str("  ");
                            i += 2;
                        }
                        '\'' => {
                            out.push('\'');
                            i += 1;
                            break;
                        }
                        other => {
                            out.push(blank(other));
                            i += 1;
                        }
                    }
                }
            } else if chars.get(i + 2) == Some(&'\'') {
                out.push_str("' '");
                i += 3;
            } else {
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// At `chars[at] == 'r'`: the number of `#`s when a raw string starts here.
fn raw_string_start(chars: &[char], at: usize) -> Option<usize> {
    if chars.get(at) != Some(&'r') {
        return None;
    }
    let mut hashes = 0;
    while chars.get(at + 1 + hashes) == Some(&'#') {
        hashes += 1;
    }
    (chars.get(at + 1 + hashes) == Some(&'"')).then_some(hashes)
}

fn count_matches(line: &str, matcher: Matcher) -> usize {
    let (needle, rest_ok): (&str, fn(&str) -> bool) = match matcher {
        Matcher::Word(text) => (text, |_| true),
        Matcher::MethodCall(text) => (text, |rest| {
            let ident = rest.chars().take_while(|c| is_ident(*c)).count();
            ident > 0 && rest[ident..].starts_with('(')
        }),
        Matcher::BeforeDigit(text) => (text, |rest| {
            let rest = rest.trim_start().trim_start_matches('-').trim_start();
            rest.starts_with(|c: char| c.is_ascii_digit())
        }),
    };
    let first_is_ident = needle.starts_with(is_ident);
    line.match_indices(needle)
        .filter(|(at, _)| !first_is_ident || !line[..*at].chars().next_back().is_some_and(is_ident))
        .filter(|(at, _)| rest_ok(&line[at + needle.len()..]))
        .count()
}

/// The files that hold out-of-line `#[cfg(test)]` modules, from `mod name;` declarations.
fn test_module_files(files: &[(PathBuf, String)]) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    for (path, source) in files {
        let dir = path.parent().expect("a parent directory");
        let stem = path.file_stem().expect("a file name").to_string_lossy();
        let module_dir = if stem == "mod" || stem == "lib" {
            dir.to_path_buf()
        } else {
            dir.join(&*stem)
        };
        let mut cfg_test = false;
        let mut path_attr: Option<String> = None;
        for line in source.lines() {
            let line = line.trim();
            if line.starts_with("#[cfg(test)]") {
                cfg_test = true;
            } else if let Some(rest) = line.strip_prefix("#[path = \"") {
                path_attr = rest.split('"').next().map(str::to_owned);
            } else if !line.starts_with("#[") {
                if cfg_test && let Some(name) = declared_module(line) {
                    match &path_attr {
                        Some(file) => {
                            out.insert(dir.join(file));
                        }
                        None => {
                            out.insert(module_dir.join(format!("{name}.rs")));
                            out.insert(module_dir.join(name));
                        }
                    }
                }
                cfg_test = false;
                path_attr = None;
            }
        }
    }
    out
}

/// `name` for a line `[pub(..)] mod name;`.
fn declared_module(line: &str) -> Option<&str> {
    let line = line.strip_prefix("pub ").unwrap_or(line);
    let line = match line.strip_prefix("pub(") {
        Some(rest) => rest.split_once(')')?.1.trim_start(),
        None => line,
    };
    line.strip_prefix("mod ")?.strip_suffix(';').map(str::trim)
}

/// Starts a `const` item (`const NAME`, not `const fn`).
fn starts_const_item(line: &str) -> bool {
    let line = line.strip_prefix("pub ").unwrap_or(line);
    let line = match line.strip_prefix("pub(") {
        Some(rest) => rest
            .split_once(')')
            .map_or(line, |(_, rest)| rest.trim_start()),
        None => line,
    };
    line.strip_prefix("const ")
        .is_some_and(|rest| !rest.starts_with("fn ") && !rest.starts_with('{'))
}

/// Pattern counts of one file, skipping test modules.
fn count_file(path: &str, source: &str, patterns: &[&Pattern], counts: &mut Counts) {
    let code = strip_comments_and_strings(source);
    let consts_exempt = px_in_consts_allowed(path);
    // Brace depth at which a skipped test module closes.
    let mut skip_until: Option<i64> = None;
    let mut depth: i64 = 0;
    let mut cfg_test = false;
    let mut in_const = false;
    for line in code.lines() {
        let trimmed = line.trim();
        let opens = line.matches('{').count() as i64;
        let closes = line.matches('}').count() as i64;
        if let Some(close_at) = skip_until {
            depth += opens - closes;
            if depth <= close_at {
                skip_until = None;
            }
            continue;
        }
        if trimmed.starts_with("#[cfg(test)]") {
            cfg_test = true;
        } else if !trimmed.starts_with("#[") && !trimmed.is_empty() {
            let module_block = {
                let item = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
                let item = item.strip_prefix("pub(crate) ").unwrap_or(item);
                item.starts_with("mod ") && trimmed.contains('{')
            };
            let tests_block = trimmed.starts_with("mod tests") && trimmed.contains('{');
            if (cfg_test && module_block) || tests_block {
                skip_until = Some(depth);
                depth += opens - closes;
                if depth <= skip_until.expect("set") {
                    skip_until = None;
                }
                cfg_test = false;
                continue;
            }
            cfg_test = false;
        }
        depth += opens - closes;

        if consts_exempt && starts_const_item(trimmed) {
            in_const = true;
        }
        for pattern in patterns {
            if in_const && pattern.name == PX_DIGIT {
                continue;
            }
            let hits = count_matches(line, pattern.matcher);
            if hits > 0 {
                *counts
                    .entry((path.to_owned(), pattern.name.to_owned()))
                    .or_default() += hits;
            }
        }
        if in_const && trimmed.ends_with(';') {
            in_const = false;
        }
    }
}

/// Counts for the files `include` selects.
fn measure(include: impl Fn(&str) -> bool, patterns: &[&[Pattern]]) -> Counts {
    let mut paths = Vec::new();
    rust_files(&src_dir(), &mut paths);
    let files: Vec<(PathBuf, String)> = paths
        .into_iter()
        .map(|path| {
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (path, source)
        })
        .collect();
    let test_files = test_module_files(&files);
    let patterns: Vec<&Pattern> = patterns.iter().flat_map(|set| set.iter()).collect();
    let mut counts = Counts::new();
    for (path, source) in &files {
        if test_files
            .iter()
            .any(|test| path == test || (test.is_dir() && path.starts_with(test)))
        {
            continue;
        }
        let rel = relative(path);
        if include(&rel) {
            count_file(&rel, source, &patterns, &mut counts);
        }
    }
    counts
}

/// The entries and the leading comment block of an allowlist file.
fn read_allowlist(path: &Path) -> Option<(String, Counts)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut header = String::new();
    let mut entries = Counts::new();
    let mut in_header = true;
    for (number, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            if in_header {
                header.push_str(line);
                header.push('\n');
            }
            continue;
        }
        in_header = false;
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        let [file, pattern, count] = fields[..] else {
            panic!(
                "{}:{}: expected `<path> <pattern> <count>`, got `{trimmed}`",
                path.display(),
                number + 1
            );
        };
        let count: usize = count.parse().unwrap_or_else(|_| {
            panic!(
                "{}:{}: `{count}` is not a count",
                path.display(),
                number + 1
            )
        });
        entries.insert((file.to_owned(), pattern.to_owned()), count);
    }
    Some((header.trim_end().to_owned() + "\n", entries))
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1")
}

fn check(allowlist_name: &str, default_header: &str, actual: Counts) {
    let allowlist_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(allowlist_name);
    let existing = read_allowlist(&allowlist_path);
    let allowed = existing
        .as_ref()
        .map(|(_, entries)| entries.clone())
        .unwrap_or_default();

    let mut over = Vec::new();
    let mut lower = Vec::new();
    for (key, &count) in &actual {
        match allowed.get(key) {
            Some(&limit) if count > limit => {
                over.push(format!("{} {} {count} (allowed {limit})", key.0, key.1))
            }
            Some(&limit) if count < limit => lower.push(format!("{} {} {count}", key.0, key.1)),
            Some(_) => {}
            None => over.push(format!("{} {} {count} (not listed)", key.0, key.1)),
        }
    }
    for key in allowed.keys().filter(|key| !actual.contains_key(*key)) {
        lower.push(format!("{} {} 0 (remove the line)", key.0, key.1));
    }

    if env_flag("STYLE_GUARD_WRITE") {
        if (!over.is_empty() || existing.is_none()) && !env_flag("STYLE_GUARD_ALLOW_INCREASE") {
            panic!(
                "STYLE_GUARD_WRITE refuses to raise counts in {allowlist_name} \
                 (set STYLE_GUARD_ALLOW_INCREASE=1 to allow it):\n  {}",
                if over.is_empty() {
                    "the allowlist does not exist yet".to_owned()
                } else {
                    over.join("\n  ")
                }
            );
        }
        let header = existing
            .map(|(header, _)| header)
            .filter(|header| !header.trim().is_empty())
            .unwrap_or_else(|| default_header.to_owned());
        let mut text = header;
        text.push('\n');
        for ((file, pattern), count) in &actual {
            writeln!(text, "{file} {pattern} {count}").expect("write to a String");
        }
        std::fs::write(&allowlist_path, text)
            .unwrap_or_else(|error| panic!("write {}: {error}", allowlist_path.display()));
        println!(
            "wrote {} ({} entries)",
            allowlist_path.display(),
            actual.len()
        );
        return;
    }

    if !lower.is_empty() {
        println!(
            "{allowlist_name}: counts went down; lower these entries (or run with \
             STYLE_GUARD_WRITE=1):\n  {}",
            lower.join("\n  ")
        );
    }
    assert!(
        over.is_empty(),
        "off-system styling beyond {allowlist_name}; use the kit and the theme's roles \
         (docs/desktop/design-system.md) instead:\n  {}",
        over.join("\n  ")
    );
}

#[test]
fn screens_style_through_the_kit() {
    let actual = measure(
        |path| !path.starts_with("kit/"),
        &[COLOUR_PATTERNS, SCREEN_PATTERNS],
    );
    check("style_guard_allowlist.txt", SCREEN_HEADER, actual);
}

#[test]
fn kit_paints_with_theme_roles() {
    let actual = measure(|path| path.starts_with("kit/"), &[COLOUR_PATTERNS]);
    check("style_guard_kit_allowlist.txt", KIT_HEADER, actual);
}

#[test]
fn matcher_ignores_comments_strings_and_fields() {
    let code = strip_comments_and_strings(
        "let a = gray(3); // gray(4)\nlet b = \"gray(5)\"; let c = r#\"rgb(1)\"#; let d = '{';\n",
    );
    let lines: Vec<&str> = code.lines().collect();
    assert_eq!(count_matches(lines[0], Matcher::Word("gray(")), 1);
    assert_eq!(count_matches(lines[1], Matcher::Word("gray(")), 0);
    assert_eq!(count_matches(lines[1], Matcher::Word("rgb(")), 0);
    assert!(!lines[1].contains('{'));
    assert_eq!(
        count_matches("x.settings_window = 1; t.settings_text()", {
            Matcher::MethodCall(".settings_")
        }),
        1
    );
    assert_eq!(
        count_matches(
            "px(12.) px(METRICS.space_2) px(-1.)",
            Matcher::BeforeDigit("px(")
        ),
        2
    );
    assert_eq!(count_matches("bgray(1) gray(2)", Matcher::Word("gray(")), 1);
}
