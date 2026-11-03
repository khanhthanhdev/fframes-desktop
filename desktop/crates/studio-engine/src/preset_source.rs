//! Reads the material a preset is made from ([`studio_presets::extract`]): a website, fetched
//! with the system `curl` as [`crate::updates`] does, or a file (a page or style sheet, a
//! brand document, design tokens, an SVG, or an image).

use std::{fs, io::Read, path::Path, process::Command};

use studio_presets::extract::{Derived, Evidence, derive};

/// The most a page, a style sheet or a file may be.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// The linked style sheets read per page.
const MAX_SHEETS: usize = 8;

/// A preset made from `source`, an `http(s)://` address or a file path.
pub fn derive_from(source: &str) -> Result<Derived, String> {
    let source = source.trim();
    let lower = source.to_ascii_lowercase();
    let is_url = lower.starts_with("http://") || lower.starts_with("https://");
    // A bare domain is a website too.
    let looks_like_domain = !source.contains(['\\', ' '])
        && !Path::new(source).exists()
        && source
            .split('/')
            .next()
            .is_some_and(|host| host.contains('.') && !host.starts_with('.'));
    let mut evidence = Evidence::default();
    let (name, label) = if is_url || looks_like_domain {
        let url = if is_url {
            source.to_owned()
        } else {
            format!("https://{source}")
        };
        read_site(&url, &mut evidence)?;
        (site_name(&url), url)
    } else {
        let path = Path::new(source);
        read_file(path, &mut evidence)?;
        let stem = path
            .file_stem()
            .map_or_else(|| "Brand".to_owned(), |s| s.to_string_lossy().into_owned());
        let file = path
            .file_name()
            .map_or_else(|| source.to_owned(), |s| s.to_string_lossy().into_owned());
        (title_case(&stem), file)
    };
    derive(&evidence, &name, &label).map_err(|e| format!("{label}: {e}"))
}

fn read_site(url: &str, evidence: &mut Evidence) -> Result<(), String> {
    let (page, final_url) = fetch(url)?;
    let html = String::from_utf8_lossy(&page);
    let sheets = evidence.add_html(&html);
    for href in sheets.into_iter().take(MAX_SHEETS) {
        let Some(sheet) = resolve(&final_url, &href) else {
            continue;
        };
        // A style sheet that fails to load leaves the rest of the page's evidence.
        if let Ok((css, _)) = fetch(&sheet) {
            evidence.add_css(&String::from_utf8_lossy(&css));
        }
    }
    Ok(())
}

fn read_file(path: &Path, evidence: &mut Evidence) -> Result<(), String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{}: larger than 4 MiB", path.display()));
    }
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => {
            let image = image::load_from_memory(&bytes)
                .map_err(|e| format!("{}: not a readable image ({e})", path.display()))?;
            // A small copy has the same colors and is quick to count.
            let rgba = image.thumbnail(256, 256).to_rgba8();
            evidence.add_pixels(rgba.as_raw());
        }
        "pdf" | "docx" | "pptx" | "key" | "ai" | "psd" => {
            return Err(format!(
                "{}: {extension} files are not read; export the brand guide as Markdown or text, or a page of it as an image",
                path.display()
            ));
        }
        _ => {
            let text = String::from_utf8_lossy(&bytes);
            match extension.as_str() {
                "html" | "htm" => {
                    evidence.add_html(&text);
                }
                "css" | "scss" | "less" => evidence.add_css(&text),
                // Markdown, text, design-token JSON, SVG: color literals and font lines.
                _ => evidence.add_text(&text),
            }
        }
    }
    Ok(())
}

/// GETs `url` (HTTPS, or HTTP), following redirects; its bytes and the address it ended at.
fn fetch(url: &str) -> Result<(Vec<u8>, String), String> {
    let mut command = Command::new("curl");
    command.args([
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        "--proto",
        "=http,https",
        "--max-redirs",
        "5",
        "--max-time",
        "20",
        "--max-filesize",
        &MAX_BYTES.to_string(),
        "--user-agent",
        "fframes-studio-preset/1",
        "--write-out",
        "\n%{url_effective}",
        "--",
        url,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|e| format!("curl is unavailable ({e})"))?;
    if !output.status.success() {
        return Err(format!(
            "Cannot read {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut body = output.stdout;
    let split = body.iter().rposition(|b| *b == b'\n').unwrap_or(0);
    let effective = String::from_utf8_lossy(&body[split..]).trim().to_owned();
    body.truncate(split);
    Ok((
        body,
        if effective.is_empty() {
            url.to_owned()
        } else {
            effective
        },
    ))
}

/// `href` as an absolute address from the page at `base`; `None` for other schemes.
fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(href.to_owned());
    }
    if href.contains(':') && !href.starts_with('/') {
        return None; // data:, javascript:, ...
    }
    let (scheme, rest) = base.split_once("://")?;
    if let Some(network) = href.strip_prefix("//") {
        return Some(format!("{scheme}://{network}"));
    }
    let host = rest.split(['/', '?', '#']).next()?;
    if href.starts_with('/') {
        return Some(format!("{scheme}://{host}{href}"));
    }
    let path = rest[host.len()..].split(['?', '#']).next().unwrap_or("");
    let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
    let dir = if dir.is_empty() { "/" } else { dir };
    Some(format!("{scheme}://{host}{dir}{href}"))
}

/// `stripe.com` for `https://www.stripe.com/pricing`, title-cased: "Stripe".
fn site_name(url: &str) -> String {
    let host = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    title_case(host.split('.').next().unwrap_or(host))
}

fn title_case(text: &str) -> String {
    text.split(['-', '_', ' ', '.'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stylesheet_links_resolve_against_the_page() {
        let page = "https://shop.example/en/home.html?x=1";
        assert_eq!(
            resolve(page, "/a.css").unwrap(),
            "https://shop.example/a.css"
        );
        assert_eq!(
            resolve(page, "css/b.css").unwrap(),
            "https://shop.example/en/css/b.css"
        );
        assert_eq!(
            resolve(page, "//cdn.example/c.css").unwrap(),
            "https://cdn.example/c.css"
        );
        assert_eq!(
            resolve("https://x.example", "d.css").unwrap(),
            "https://x.example/d.css"
        );
        assert_eq!(resolve(page, "data:text/css,body{}"), None);
    }

    #[test]
    fn names_come_from_the_site_or_file() {
        assert_eq!(site_name("https://www.rosa-bakery.com/menu"), "Rosa Bakery");
        assert_eq!(title_case("acme_brand guide"), "Acme Brand Guide");
    }

    #[test]
    fn files_are_read_by_kind() {
        let dir = tempfile::tempdir().unwrap();
        let guide = dir.path().join("acme-brand.md");
        fs::write(
            &guide,
            "Primary: #FF5A1F\nBackground: #0B132B\nText: #F4F4F9\n",
        )
        .unwrap();
        let derived = derive_from(guide.to_str().unwrap()).unwrap();
        assert_eq!(derived.package.manifest().name, "Acme Brand");
        assert_eq!(derived.accent.to_hex(), "#FF5A1F");

        let logo = dir.path().join("logo.png");
        let mut image = image::RgbaImage::from_pixel(40, 40, image::Rgba([0x0B, 0x13, 0x2B, 255]));
        for x in 0..40 {
            for y in 0..6 {
                image.put_pixel(x, y, image::Rgba([0xFF, 0x5A, 0x1F, 255]));
            }
        }
        image.save(&logo).unwrap();
        let derived = derive_from(logo.to_str().unwrap()).unwrap();
        assert_eq!(derived.background.to_hex(), "#0B132B");
        assert_eq!(derived.accent.to_hex(), "#FF5A1F");

        let pdf = dir.path().join("brand.pdf");
        fs::write(&pdf, b"%PDF-1.7").unwrap();
        assert!(
            derive_from(pdf.to_str().unwrap())
                .unwrap_err()
                .contains("Markdown")
        );
    }
}
