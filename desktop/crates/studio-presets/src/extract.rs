//! A preset made from existing material: a website's HTML and CSS, a brand document's text,
//! or an image's pixels.
//!
//! [`Evidence`] collects what the material shows: the colors it paints (and with what: a
//! background, text or an accent such as a button or `--brand` variable), the font families
//! it names and its corner radii. [`derive`] turns that into a complete package on top of the
//! bundled preset that suits it best: the palette and radii come from the material, every
//! other token (type scale, spacing, motion) and the font from the bundled preset. Fonts are
//! never copied from the material, because their licenses are unknown; the guide names the
//! families it found so they can be added with their licenses.
//!
//! Nothing is evaluated: CSS is scanned for declarations, HTML for `<style>`, `style=""`,
//! stylesheet links and `theme-color`, and text for color literals and font lines.

use std::collections::BTreeMap;

use studio_project::ProjectPath;

use crate::{
    builtin,
    model::{Code, Color, PresetError},
    package::{MANIFEST_FILE, Manifest, Package, seal_files},
};

/// What a color painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    Background,
    Text,
    /// Buttons, links, `--primary`, `--brand`, `theme-color`.
    Accent,
    Other,
}

/// What the material shows, weighted by how often and how prominently.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    colors: Vec<(Color, Use, f32)>,
    fonts: BTreeMap<String, f32>,
    radii: Vec<f32>,
}

/// A preset made by [`derive`], with what it took from the material.
#[derive(Debug, Clone)]
pub struct Derived {
    pub package: Package,
    /// The bundled preset it builds on.
    pub base: &'static str,
    pub background: Color,
    pub text: Color,
    pub accent: Color,
    /// The font families the material names, most used first.
    pub fonts: Vec<String>,
}

/// At most this many colors are kept, so a huge stylesheet stays cheap.
const MAX_COLORS: usize = 20_000;

impl Evidence {
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    fn color(&mut self, color: Color, used: Use, weight: f32) {
        // Mostly transparent colors are overlays, not the palette.
        if color.a >= 128 && self.colors.len() < MAX_COLORS {
            self.colors.push((Color { a: 255, ..color }, used, weight));
        }
    }

    fn font(&mut self, list: &str, weight: f32) {
        let Some(first) = list.split(',').next() else {
            return;
        };
        let family = first.trim().trim_matches(['"', '\'']).trim();
        let lower = family.to_ascii_lowercase();
        const GENERIC: [&str; 12] = [
            "sans-serif",
            "serif",
            "monospace",
            "system-ui",
            "-apple-system",
            "inherit",
            "initial",
            "unset",
            "cursive",
            "fantasy",
            "ui-sans-serif",
            "blinkmacsystemfont",
        ];
        if family.is_empty()
            || family.len() > 48
            || GENERIC.contains(&lower.as_str())
            || lower.starts_with("ui-")
            || lower.contains("emoji")
            || family.contains(['(', ')', '{', '}', '<', '>', ':', ';'])
        {
            return;
        }
        *self.fonts.entry(family.to_owned()).or_default() += weight;
    }

    /// Declarations of a style sheet. Colors on `html`, `body` and `:root` weigh most;
    /// colors of buttons and links, and variables named like a brand color, are accents.
    pub fn add_css(&mut self, css: &str) {
        let css = strip_comments(css);
        let mut selectors: Vec<String> = Vec::new();
        let mut buffer = String::new();
        for c in css.chars() {
            match c {
                '{' => {
                    selectors.push(buffer.trim().to_ascii_lowercase());
                    buffer.clear();
                }
                ';' | '}' => {
                    let selector = selectors.last().map_or("", String::as_str);
                    self.declaration(selector, &buffer);
                    buffer.clear();
                    if c == '}' {
                        selectors.pop();
                    }
                }
                _ => buffer.push(c),
            }
        }
        self.declaration(selectors.last().map_or("", String::as_str), &buffer);
    }

    fn declaration(&mut self, selector: &str, text: &str) {
        let Some((property, value)) = text.split_once(':') else {
            return;
        };
        let property = property.trim().to_ascii_lowercase();
        let value = value.trim();
        let root = selector.is_empty()
            || ["html", "body", ":root", "main"]
                .iter()
                .any(|s| selector.split([',', ' ', '>']).any(|part| part == *s));
        let interactive = ["button", "btn", "cta", "primary", "a:", "link"]
            .iter()
            .any(|s| selector.contains(s))
            || selector.split([',', ' ']).any(|part| part == "a");
        let weight = if root { 4.0 } else { 1.0 };
        let used = if let Some(name) = property.strip_prefix("--") {
            if ["primary", "brand", "accent", "cta", "highlight", "link"]
                .iter()
                .any(|s| name.contains(s))
            {
                Use::Accent
            } else if ["background", "bg", "surface", "base"]
                .iter()
                .any(|s| name.contains(s))
            {
                Use::Background
            } else if ["foreground", "fg", "text", "ink", "body"]
                .iter()
                .any(|s| name.contains(s))
            {
                Use::Text
            } else {
                Use::Other
            }
        } else {
            match property.as_str() {
                "background" | "background-color" if interactive => Use::Accent,
                "background" | "background-color" => Use::Background,
                "color" if interactive => Use::Accent,
                "color" => Use::Text,
                "font-family" => {
                    self.font(value, weight);
                    return;
                }
                "border-radius" => {
                    if let Some(px) = first_length(value) {
                        self.radii.push(px);
                    }
                    return;
                }
                "border-color"
                | "fill"
                | "stroke"
                | "outline-color"
                | "border"
                | "border-top"
                | "border-bottom"
                | "box-shadow"
                | "text-decoration-color" => Use::Other,
                _ => return,
            }
        };
        // A variable counts once per definition; a page usually defines it on :root.
        let weight = if property.starts_with("--") {
            3.0
        } else {
            weight
        };
        for color in colors_in(value) {
            self.color(color, used, weight);
        }
    }

    /// An HTML page: its `<style>` blocks, `style=""` attributes and `theme-color`. Returns
    /// the `href` of every linked style sheet, as written, for the caller to fetch.
    pub fn add_html(&mut self, html: &str) -> Vec<String> {
        let lower = html.to_ascii_lowercase();
        let mut sheets = Vec::new();
        let mut at = 0;
        while let Some(start) = lower[at..].find('<').map(|i| at + i) {
            let Some(end) = lower[start..].find('>').map(|i| start + i) else {
                break;
            };
            let tag = &html[start..=end];
            let tag_lower = &lower[start..=end];
            at = end + 1;
            if tag_lower.starts_with("<style") {
                let close = lower[at..].find("</style").map_or(html.len(), |i| at + i);
                self.add_css(&html[at..close]);
                at = close;
                continue;
            }
            if tag_lower.starts_with("<script") {
                at = lower[at..].find("</script").map_or(html.len(), |i| at + i);
                continue;
            }
            if let Some(style) = attribute(tag, "style") {
                self.add_css(&style);
            }
            if tag_lower.starts_with("<link")
                && attribute(tag, "rel")
                    .is_some_and(|rel| rel.to_ascii_lowercase().contains("stylesheet"))
                && let Some(href) = attribute(tag, "href")
            {
                sheets.push(href);
            }
            if tag_lower.starts_with("<meta")
                && attribute(tag, "name").is_some_and(|n| n.eq_ignore_ascii_case("theme-color"))
                && let Some(content) = attribute(tag, "content")
            {
                for color in colors_in(&content) {
                    self.color(color, Use::Accent, 6.0);
                }
            }
        }
        sheets
    }

    /// A text document (a brand guide in Markdown or plain text, design-token JSON, SVG):
    /// every color literal, used as the words on its line say, and lines that name a font.
    pub fn add_text(&mut self, text: &str) {
        for line in text.lines() {
            let lower = line.to_ascii_lowercase();
            let used = if [
                "primary",
                "brand",
                "accent",
                "cta",
                "highlight",
                "button",
                "link",
            ]
            .iter()
            .any(|s| lower.contains(s))
            {
                Use::Accent
            } else if ["background", "\"bg", "canvas", "paper", "fill"]
                .iter()
                .any(|s| lower.contains(s))
            {
                Use::Background
            } else if ["text", "foreground", "ink", "body copy", "type color"]
                .iter()
                .any(|s| lower.contains(s))
            {
                Use::Text
            } else {
                Use::Other
            };
            for color in colors_in(line) {
                self.color(color, used, 2.0);
            }
            for key in ["font-family", "typeface", "font", "typography"] {
                if let Some(index) = lower.find(key) {
                    let rest = &line[index + key.len()..];
                    if let Some(value) = rest.trim_start().strip_prefix([':', '=']) {
                        let value = value.trim().trim_matches(['"', '\'', ',', '*', '`']);
                        self.font(value.split([';', '(', '|']).next().unwrap_or(""), 2.0);
                    }
                    break;
                }
            }
        }
    }

    /// An image as RGBA rows: the colors covering most of it, the largest as the
    /// background, the vivid ones as accents.
    pub fn add_pixels(&mut self, rgba: &[u8]) {
        let mut bins = vec![(0u32, [0u64; 3]); 1 << 15];
        let pixels = rgba.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128);
        let mut total = 0u32;
        for p in pixels {
            let bin =
                ((p[0] as usize >> 3) << 10) | ((p[1] as usize >> 3) << 5) | (p[2] as usize >> 3);
            let entry = &mut bins[bin];
            entry.0 += 1;
            for (sum, value) in entry.1.iter_mut().zip(p) {
                *sum += u64::from(*value);
            }
            total += 1;
        }
        if total == 0 {
            return;
        }
        let mut found: Vec<(Color, u32)> = bins
            .into_iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, sum)| {
                let mean = |i: usize| (sum[i] / u64::from(count)) as u8;
                (
                    Color {
                        r: mean(0),
                        g: mean(1),
                        b: mean(2),
                        a: 255,
                    },
                    count,
                )
            })
            .collect();
        found.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        let palette = merge(
            found.into_iter().map(|(c, n)| (c, n as f32)).collect(),
            40.0,
        );
        for (index, (color, count)) in palette.into_iter().take(12).enumerate() {
            let share = count / total as f32 * 100.0;
            let used = if index == 0 {
                Use::Background
            } else if saturation(color) > 0.35 {
                Use::Accent
            } else {
                Use::Other
            };
            self.color(color, used, share);
        }
    }
}

/// Builds a complete preset named `name` from `evidence`. `source` (a URL or a file name)
/// is recorded in the description and guide.
pub fn derive(evidence: &Evidence, name: &str, source: &str) -> Result<Derived, PresetError> {
    let palette = Palette::choose(evidence).ok_or_else(|| {
        PresetError::one(
            Code::MissingResource,
            "source",
            "no colors were found in it; try a page with its own styles, a brand guide that lists hex colors, or a logo or screenshot",
        )
    })?;
    let mut radii = evidence.radii.clone();
    radii.retain(|r| r.is_finite() && *r >= 0.0 && *r < 200.0);
    radii.sort_by(f32::total_cmp);
    let radius = (radii.len() >= 3).then(|| radii[radii.len() / 2].min(64.0));
    let fonts: Vec<String> = {
        let mut fonts: Vec<_> = evidence.fonts.iter().collect();
        fonts.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
        fonts.into_iter().take(4).map(|(f, _)| f.clone()).collect()
    };
    let dark = luminance(palette.background) < 0.18;
    // Code blocks name a monospace font on most sites; only a site set in one is Terminal.
    let base = if fonts.first().is_some_and(|f| monospace(f)) {
        "terminal"
    } else {
        match (dark, radius) {
            (true, Some(r)) if r < 6.0 => "luxe",
            (true, _) => "pulse",
            (false, Some(r)) if r < 2.0 => "swiss",
            (false, Some(r)) if r >= 20.0 => "fresh",
            (false, _) => "launch",
        }
    };
    let base_package = builtin::get(base).ok_or_else(|| {
        PresetError::one(
            Code::MissingResource,
            "base",
            format!("bundled preset {base} is missing"),
        )
    })?;

    let mut files: BTreeMap<ProjectPath, Vec<u8>> = base_package
        .files()
        .iter()
        .filter(|(path, _)| !path.as_str().starts_with("examples/"))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();

    let mut tokens: serde_json::Value = serde_json::from_slice(&files[&path("tokens.json")])
        .map_err(|e| PresetError::one(Code::Malformed, "tokens.json", e.to_string()))?;
    let mut set = |token: &str, value: serde_json::Value| {
        tokens["tokens"][token]["value"] = value;
    };
    let hex = |c: Color| serde_json::Value::String(c.to_hex());
    set("color.background", hex(palette.background));
    set("color.text", hex(palette.text));
    set("color.accent", hex(palette.accent));
    set("color.highlight", hex(palette.highlight));
    set(
        "color.surface",
        hex(mix(palette.background, palette.text, 0.06)),
    );
    set(
        "color.muted",
        hex(mix(palette.text, palette.background, 0.4)),
    );
    if let Some(r) = radius {
        let round = |v: f32| serde_json::json!((v * 2.0).round() / 2.0);
        set("radius.sm", round(r / 2.0));
        set("radius.md", round(r));
        set("radius.lg", round((r * 2.0).min(96.0)));
    }
    let with_alpha = |c: Color, a: u8| serde_json::Value::String(Color { a, ..c }.to_hex());
    tokens["tokens"]["shadow.glow"]["value"]["color"] = with_alpha(palette.accent, 0x55);
    tokens["tokens"]["shadow.card"]["value"]["color"] = with_alpha(
        if dark {
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }
        } else {
            palette.text
        },
        0x24,
    );
    let mut tokens_text = serde_json::to_string_pretty(&tokens).expect("serializes");
    tokens_text.push('\n');
    files.insert(path("tokens.json"), tokens_text.into_bytes());

    let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    let name = if name.trim().is_empty() {
        "Brand".to_owned()
    } else {
        name
    };
    let base_name = base_package.manifest().name.clone();
    let source: String = source
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    let font_family = base_package
        .manifest()
        .fonts
        .first()
        .map_or("the bundled font".to_owned(), |f| f.family.clone());
    let found_fonts = if fonts.is_empty() {
        "It names no font.".to_owned()
    } else {
        format!(
            "It uses {}. Those fonts are not copied (their licenses are unknown); {font_family} \
             stands in. To use one, import a preset that bundles it with its license.",
            fonts.join(", ")
        )
    };
    let guide = format!(
        "# {name}\n\nMade in fframes Studio from {source}: its colors{} on {base_name}'s type \
         scale, spacing and motion.\n\n## Palette\n\n| Token | Value |\n|---|---|\n\
         | `color.background` | {} |\n| `color.text` | {} |\n| `color.accent` | {} |\n\
         | `color.highlight` | {} |\n\n## Typography\n\n{found_fonts} Bind text to \
         `typography.title`, `typography.heading`, `typography.body` and `typography.caption`.\n\n\
         ## Rules\n\n- Use semantic tokens instead of literal values so the project can switch \
         presets.\n- Check the colors against the brand's own guide before publishing; they were \
         read from {source}, not given by its owner.\n",
        if radius.is_some() {
            " and corner radii"
        } else {
            ""
        },
        palette.background.to_hex(),
        palette.text.to_hex(),
        palette.accent.to_hex(),
        palette.highlight.to_hex(),
    );
    files.insert(path("guide.md"), guide.into_bytes());

    let mut manifest: Manifest = serde_json::from_slice(&files[&path(MANIFEST_FILE)])
        .map_err(|e| PresetError::one(Code::Malformed, MANIFEST_FILE, e.to_string()))?;
    manifest.id = slug(&name);
    manifest.name = name;
    manifest.version = "1.0.0".into();
    manifest.description =
        format!("Colors from {source}, on {base_name}'s type, spacing and motion.")
            .chars()
            .take(512)
            .collect();
    manifest.author = "Made in fframes Studio".into();
    manifest.examples.clear();
    let mut text = serde_json::to_string_pretty(&manifest).expect("manifest serializes");
    text.push('\n');
    files.insert(path(MANIFEST_FILE), text.into_bytes());
    let package = Package::from_files(seal_files(files)?)?;
    Ok(Derived {
        package,
        base,
        background: palette.background,
        text: palette.text,
        accent: palette.accent,
        fonts,
    })
}

fn monospace(family: &str) -> bool {
    let lower = family.to_ascii_lowercase();
    ["mono", "code", "courier", "consol"]
        .iter()
        .any(|s| lower.contains(s))
}

fn path(value: &str) -> ProjectPath {
    ProjectPath::try_from(value.to_owned()).expect("a valid package path")
}

/// `brand-<name>`: lowercase letters, digits and hyphens, at most 48 characters.
fn slug(name: &str) -> String {
    let mut out = String::from("brand-");
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_end_matches('-').chars().take(48).collect();
    while out.ends_with('-') {
        out.pop();
    }
    out
}

struct Palette {
    background: Color,
    text: Color,
    accent: Color,
    highlight: Color,
}

impl Palette {
    fn choose(evidence: &Evidence) -> Option<Palette> {
        let tally = |wanted: Option<Use>| -> Vec<(Color, f32)> {
            merge(
                evidence
                    .colors
                    .iter()
                    .filter(|(_, used, _)| wanted.is_none_or(|w| w == *used))
                    .map(|(c, _, w)| (*c, *w))
                    .collect(),
                18.0,
            )
        };
        let all = tally(None);
        let backgrounds = tally(Some(Use::Background));
        // A page background is calm: prefer the most used low-saturation background.
        let background = backgrounds
            .iter()
            .max_by(|a, b| {
                let score = |(c, w): &(Color, f32)| w * (1.2 - saturation(*c));
                score(a).total_cmp(&score(b))
            })
            .or(all.first())?
            .0;
        let texts = tally(Some(Use::Text));
        let readable = |c: &Color| contrast(*c, background) >= 4.5;
        // Text named as text, else a calm color the material uses, else black or white.
        let text = texts
            .iter()
            .chain(all.iter().filter(|(c, _)| saturation(*c) < 0.25))
            .find(|(c, _)| readable(c))
            .map(|(c, _)| *c)
            .unwrap_or_else(|| {
                let black = Color {
                    r: 0x11,
                    g: 0x11,
                    b: 0x11,
                    a: 255,
                };
                let white = Color {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                };
                if contrast(black, background) >= contrast(white, background) {
                    black
                } else {
                    white
                }
            });
        let distinct = |c: &Color, from: &[Color]| from.iter().all(|f| distance(*c, *f) > 60.0);
        let vivid = |(c, w): &(Color, f32)| w * (0.2 + saturation(*c));
        let mut accents = tally(Some(Use::Accent));
        accents.sort_by(|a, b| vivid(b).total_cmp(&vivid(a)));
        let mut others = all.clone();
        others.retain(|(c, _)| saturation(*c) > 0.3);
        others.sort_by(|a, b| vivid(b).total_cmp(&vivid(a)));
        let candidates: Vec<Color> = accents
            .iter()
            .chain(&others)
            .map(|(c, _)| *c)
            .filter(|c| contrast(*c, background) >= 1.6 && distinct(c, &[background, text]))
            .collect();
        let accent = candidates
            .first()
            .copied()
            .unwrap_or_else(|| mix(text, background, 0.25));
        let highlight = candidates
            .iter()
            .copied()
            .find(|c| distinct(c, &[background, text, accent]))
            .unwrap_or_else(|| mix(accent, background, 0.4));
        Some(Palette {
            background,
            text,
            accent,
            highlight,
        })
    }
}

/// Colors within `radius` of a heavier one fold into it; heaviest first.
fn merge(mut colors: Vec<(Color, f32)>, radius: f32) -> Vec<(Color, f32)> {
    colors.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut out: Vec<(Color, f32)> = Vec::new();
    for (color, weight) in colors {
        match out.iter_mut().find(|(c, _)| distance(*c, color) < radius) {
            Some(existing) => existing.1 += weight,
            None => out.push((color, weight)),
        }
    }
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

fn distance(a: Color, b: Color) -> f32 {
    let d = |x: u8, y: u8| (f32::from(x) - f32::from(y)).powi(2);
    (d(a.r, b.r) + d(a.g, b.g) + d(a.b, b.b)).sqrt()
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let m = |x: u8, y: u8| (f32::from(x) * (1.0 - t) + f32::from(y) * t).round() as u8;
    Color {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
        a: 255,
    }
}

/// WCAG relative luminance.
fn luminance(c: Color) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// WCAG contrast ratio, 1 to 21.
fn contrast(a: Color, b: Color) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// HSV saturation, 0 to 1.
fn saturation(c: Color) -> f32 {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    if max == 0 {
        0.0
    } else {
        f32::from(max - min) / f32::from(max)
    }
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..]
            .find("*/")
            .map_or("", |end| &rest[start + 2 + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// The value of attribute `name` in one HTML tag, quoted or bare.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(index) = lower[from..].find(name).map(|i| from + i) {
        from = index + name.len();
        let before = lower[..index].chars().last();
        if !before.is_some_and(|c| c.is_whitespace()) {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => rest[1..].split(quote).next().unwrap_or(""),
            _ => rest
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or(""),
        };
        return Some(decode_entities(value));
    }
    None
}

fn decode_entities(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The first length in a value, in px (`rem` and `em` at 16 px).
fn first_length(value: &str) -> Option<f32> {
    let token = value.split_whitespace().next()?;
    let (number, unit) = token
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or((token, ""), |i| token.split_at(i));
    let number: f32 = number.parse().ok()?;
    match unit {
        "" | "px" => Some(number),
        "rem" | "em" => Some(number * 16.0),
        _ => None,
    }
}

/// Every color literal in `text`: `#rgb`, `#rrggbb` (and their alpha forms), `rgb()` and
/// `rgba()` with numbers or percentages, and `white` and `black`.
pub fn colors_in(text: &str) -> Vec<Color> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let digits = bytes[i + 1..]
                .iter()
                .take_while(|b| b.is_ascii_hexdigit())
                .count();
            let after = bytes.get(i + 1 + digits).copied();
            let ends = after.is_none_or(|b| !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
            if matches!(digits, 3 | 4 | 6 | 8)
                && ends
                && let Some(color) = Color::parse_hex(&text[i..=i + digits])
            {
                out.push(color);
            }
            i += 1 + digits;
            continue;
        }
        let rest = &text[i..];
        let lower: String = rest
            .chars()
            .take(5)
            .collect::<String>()
            .to_ascii_lowercase();
        if (lower.starts_with("rgb(") || lower.starts_with("rgba("))
            && let Some(open) = rest.find('(')
            && let Some(close) = rest[open..].find(')')
        {
            let inner = &rest[open + 1..open + close];
            let parts: Vec<&str> = inner
                .split([',', ' ', '/'])
                .filter(|p| !p.is_empty())
                .collect();
            let channel = |p: &str| -> Option<u8> {
                match p.strip_suffix('%') {
                    Some(pct) => pct
                        .parse::<f32>()
                        .ok()
                        .map(|v| (v.clamp(0.0, 100.0) * 2.55).round() as u8),
                    None => p
                        .parse::<f32>()
                        .ok()
                        .map(|v| v.clamp(0.0, 255.0).round() as u8),
                }
            };
            if parts.len() >= 3
                && let (Some(r), Some(g), Some(b)) =
                    (channel(parts[0]), channel(parts[1]), channel(parts[2]))
            {
                let a = parts.get(3).and_then(|p| match p.strip_suffix('%') {
                    Some(pct) => pct.parse::<f32>().ok().map(|v| v / 100.0),
                    None => p.parse::<f32>().ok(),
                });
                let a = (a.unwrap_or(1.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                out.push(Color { r, g, b, a });
            }
            i += open + close + 1;
            continue;
        }
        let word_start = i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'-';
        if word_start {
            for (word, value) in [("white", 255u8), ("black", 0u8)] {
                if rest.len() >= word.len()
                    && rest[..word.len()].eq_ignore_ascii_case(word)
                    && rest
                        .as_bytes()
                        .get(word.len())
                        .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'-')
                {
                    out.push(Color {
                        r: value,
                        g: value,
                        b: value,
                        a: 255,
                    });
                }
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(c: Color) -> String {
        c.to_hex()
    }

    #[test]
    fn colors_are_found_in_every_literal_form_and_nothing_else() {
        let found: Vec<String> = colors_in(
            "a: #fff; b: #12345678; c: rgb(255, 0, 0); d: rgba(0 128 255 / 50%); e: Black; \
             f: #12345 g: #abcdefg h: blackberry url(#x)",
        )
        .into_iter()
        .map(hex)
        .collect();
        assert_eq!(
            found,
            ["#FFFFFF", "#12345678", "#FF0000", "#0080FF80", "#000000"]
        );
    }

    #[test]
    fn a_website_gives_its_page_background_text_and_brand_color() {
        let mut evidence = Evidence::default();
        let sheets = evidence.add_html(
            r##"<html><head>
            <meta name="theme-color" content="#E4405F">
            <link rel="stylesheet" href="/assets/site.css"><link rel=icon href="/favicon.ico">
            <style>
              /* body { background: #00FF00 } is a comment */
              :root { --brand-primary: #E4405F; --surface-bg: #FFF8F0; }
              body { background-color: #FFF8F0; color: #1E1B18; font-family: "Inter", sans-serif; }
              .card { background: #FFFFFF; border-radius: 12px; }
              .btn { background: #E4405F; color: white; border-radius: 12px; }
              .tag { border-radius: 0.75rem; }
              footer { color: #9C9690 }
            </style>
            <script>var s = "body{background:#000}";</script>
            </head><body style="margin:0"></body></html>"##,
        );
        assert_eq!(sheets, ["/assets/site.css"]);
        let derived = derive(&evidence, "Rosa Bakery", "https://rosa.example").unwrap();
        assert_eq!(hex(derived.background), "#FFF8F0");
        assert_eq!(hex(derived.text), "#1E1B18");
        assert_eq!(hex(derived.accent), "#E4405F");
        assert_eq!(derived.fonts, ["Inter"]);
        // Light with medium corners builds on Launch; its radii follow the site's 12 px.
        assert_eq!(derived.base, "launch");
        let tokens = derived.package.snapshot(None, None);
        assert_eq!(
            format!("{:?}", tokens.get("radius.md").unwrap()),
            format!("{:?}", crate::TokenValue::Dimension(12.0))
        );
        assert_eq!(derived.package.id(), "brand-rosa-bakery");
        assert!(String::from_utf8_lossy(derived.package.guide()).contains("Inter"));
        // The package carries its font and license like any bundled one.
        assert!(!derived.package.manifest().fonts.is_empty());
    }

    #[test]
    fn a_dark_monospace_site_builds_on_terminal_and_text_stays_readable() {
        let mut evidence = Evidence::default();
        evidence.add_css(
            "html { background: #101010; color: #222; font-family: 'JetBrains Mono', monospace }\
             a { color: #00FF9C }",
        );
        let derived = derive(&evidence, "dev", "dev.css").unwrap();
        assert_eq!(derived.base, "terminal");
        assert!(contrast(derived.text, derived.background) >= 4.5);
        assert_eq!(hex(derived.accent), "#00FF9C");
    }

    #[test]
    fn a_brand_guide_names_its_colors_and_font() {
        let mut evidence = Evidence::default();
        evidence.add_text(
            "# Acme brand\n\nPrimary: #FF5A1F\nBackground: #0B132B\nText color: #F4F4F9\n\
             Typeface: Poppins (headings), Inter (body)\n",
        );
        let derived = derive(&evidence, "Acme brand", "acme.md").unwrap();
        assert_eq!(
            [derived.background, derived.text, derived.accent].map(hex),
            ["#0B132B", "#F4F4F9", "#FF5A1F"]
        );
        assert_eq!(derived.fonts, ["Poppins"]);
        assert_eq!(derived.base, "pulse");
    }

    #[test]
    fn an_image_gives_its_dominant_color_as_background_and_a_vivid_one_as_accent() {
        // 70 % navy, 20 % white, 10 % orange.
        let mut rgba = Vec::new();
        for i in 0..1000 {
            let p: [u8; 4] = match i % 10 {
                0..=6 => [0x10, 0x1E, 0x4A, 255],
                7 | 8 => [255, 255, 255, 255],
                _ => [0xFF, 0x7A, 0x00, 255],
            };
            rgba.extend(p);
        }
        let mut evidence = Evidence::default();
        evidence.add_pixels(&rgba);
        let derived = derive(&evidence, "Poster", "poster.png").unwrap();
        assert_eq!(hex(derived.background), "#101E4A");
        assert_eq!(hex(derived.text), "#FFFFFF");
        assert_eq!(hex(derived.accent), "#FF7A00");
    }

    #[test]
    fn material_without_colors_is_refused() {
        let mut evidence = Evidence::default();
        evidence.add_text("No colors here.");
        assert!(derive(&evidence, "Nothing", "notes.txt").is_err());
    }
}
