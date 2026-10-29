//! The deck manifest and the page it is inlined into.
//!
//! The manifest is `serde_json` output with `<`, `>`, `&`, U+2028 and U+2029 written as
//! `\u` escapes. The characters only occur inside JSON strings, so the escaped text is
//! the same JSON value, and it holds no `<` at all: the HTML parser cannot end the data
//! block early (`</script>`) or switch into its escaped states (`<!--`). Nothing is
//! HTML-escaped: `app.js` parses the block and sets every title and note with
//! `textContent`, so markup in them shows literally.
use serde::Serialize;

/// The page; [`DATA_MARKER`] is replaced by the manifest.
pub const INDEX_HTML: &str = include_str!("template/index.html");
pub const STYLE_CSS: &str = include_str!("template/style.css");
pub const APP_JS: &str = include_str!("template/app.js");

/// The only generated text of `index.html`: the inside of its data block.
pub const DATA_MARKER: &str = "__FFRAMES_DECK_MANIFEST__";

pub const MANIFEST_VERSION: u32 = 1;

/// `slides.json`, and the data block of `index.html`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Manifest {
    pub version: u32,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub total_frames: usize,
    pub slides: Vec<Slide>,
}

/// One slide: what plays from the previous stop (or the first frame) to its stop.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slide {
    /// 1-based, as in the poster names and the `#N` of the page's URL.
    pub index: usize,
    pub start_frame: usize,
    pub stop_frame: usize,
    pub title: Option<String>,
    pub notes: Option<String>,
    /// Relative to the deck folder.
    pub poster: String,
    pub thumb: String,
}

/// A stop of the deck: its video frame and text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckStop {
    pub video_frame: usize,
    pub title: Option<String>,
    pub notes: Option<String>,
}

/// The file name of slide `index` (1-based) in `posters/` and `thumbs/`.
pub fn image_name(index: usize) -> String {
    format!("{index:04}.jpg")
}

impl Manifest {
    /// The manifest of `stops`, which are sorted, start after frame 0 and end on the
    /// video's last frame (`total_frames - 1`).
    pub fn new(
        title: &str,
        (width, height): (u32, u32),
        fps: u32,
        stops: &[DeckStop],
    ) -> Result<Self, String> {
        check_stops(stops)?;
        let total_frames = stops.last().map_or(0, |stop| stop.video_frame + 1);
        let mut start_frame = 0;
        let slides = stops
            .iter()
            .enumerate()
            .map(|(position, stop)| {
                let index = position + 1;
                let slide = Slide {
                    index,
                    start_frame,
                    stop_frame: stop.video_frame,
                    title: stop.title.clone(),
                    notes: stop.notes.clone(),
                    poster: format!("posters/{}", image_name(index)),
                    thumb: format!("thumbs/{}", image_name(index)),
                };
                start_frame = stop.video_frame;
                slide
            })
            .collect();
        Ok(Self {
            version: MANIFEST_VERSION,
            title: title.to_owned(),
            width,
            height,
            fps,
            total_frames,
            slides,
        })
    }

    /// The manifest as inert JSON (see the module docs).
    pub fn to_inline_json(&self) -> String {
        let json = serde_json::to_string(self).expect("a manifest serializes");
        escape_inline_json(&json)
    }

    /// `index.html` with this manifest in its data block.
    pub fn index_html(&self) -> String {
        render_index(&self.to_inline_json())
    }
}

/// Stops must be sorted, after frame 0, and not empty.
pub fn check_stops(stops: &[DeckStop]) -> Result<(), String> {
    if stops.is_empty() {
        return Err("the deck has no slides".into());
    }
    let mut previous = 0;
    for stop in stops {
        if stop.video_frame <= previous {
            return Err(format!(
                "slide stops must be sorted and after the first frame (frame {} after {previous})",
                stop.video_frame
            ));
        }
        previous = stop.video_frame;
    }
    Ok(())
}

/// The time the page seeks to so that `frame` is the picture shown: the middle of the
/// frame, away from both of its edges.
pub fn hold_seconds(frame: usize, fps: u32) -> f64 {
    (frame as f64 + 0.5) / f64::from(fps)
}

/// Escapes the characters that could end or change an HTML data block, or a JavaScript
/// string in older engines, as `\u` sequences; valid JSON stays the same value.
pub fn escape_inline_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len() + 16);
    for character in json.chars() {
        match character {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            other => out.push(other),
        }
    }
    out
}

/// `index.html` with `inline_json` (already escaped) in place of [`DATA_MARKER`].
pub fn render_index(inline_json: &str) -> String {
    debug_assert_eq!(INDEX_HTML.matches(DATA_MARKER).count(), 1);
    INDEX_HTML.replacen(DATA_MARKER, inline_json, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: &str = r#"<script type="application/json" id="deck-data">"#;

    fn stop(video_frame: usize, notes: &str) -> DeckStop {
        DeckStop {
            video_frame,
            title: Some(format!("Title <b>{video_frame}</b> & co")),
            notes: Some(notes.to_owned()),
        }
    }

    /// The text of the data block as an HTML parser reads it: from the start tag to the
    /// first `</script` in any case.
    fn data_block(html: &str) -> &str {
        let start = html.find(OPEN).expect("the data block") + OPEN.len();
        let rest = &html[start..];
        let end = rest
            .to_ascii_lowercase()
            .find("</script")
            .expect("the block ends");
        &rest[..end]
    }

    #[test]
    fn slides_run_from_the_previous_stop_and_name_their_images() {
        let manifest = Manifest::new(
            "Deck",
            (1920, 1080),
            30,
            &[stop(29, "a"), stop(89, "b"), stop(119, "c")],
        )
        .unwrap();
        assert_eq!(manifest.total_frames, 120);
        let ranges: Vec<_> = manifest
            .slides
            .iter()
            .map(|slide| (slide.index, slide.start_frame, slide.stop_frame))
            .collect();
        assert_eq!(ranges, [(1, 0, 29), (2, 29, 89), (3, 89, 119)]);
        assert_eq!(manifest.slides[1].poster, "posters/0002.jpg");
        assert_eq!(manifest.slides[1].thumb, "thumbs/0002.jpg");
    }

    #[test]
    fn unsorted_empty_or_first_frame_stops_are_refused() {
        assert!(Manifest::new("x", (2, 2), 30, &[]).is_err());
        assert!(Manifest::new("x", (2, 2), 30, &[stop(0, "")]).is_err());
        assert!(Manifest::new("x", (2, 2), 30, &[stop(9, ""), stop(9, "")]).is_err());
        assert!(Manifest::new("x", (2, 2), 30, &[stop(9, ""), stop(3, "")]).is_err());
    }

    #[test]
    fn a_stop_is_held_in_the_middle_of_its_frame() {
        assert_eq!(hold_seconds(29, 30), 29.5 / 30.);
        assert_eq!(hold_seconds(0, 25), 0.02);
        // Within half a frame of the frame's start, so a decoder lands on that frame.
        let fps = 60;
        let held = hold_seconds(119, fps);
        let (start, end) = (119.0 / 60.0, 120.0 / 60.0);
        assert!((start..end).contains(&held));
    }

    #[test]
    fn markup_in_notes_cannot_end_or_escape_the_data_block() {
        let hostile = [
            "</script><script>alert(1)</script>",
            "</SCRIPT >",
            "<!--<script>",
            "line\u{2028}separator\u{2029}paragraph",
            "&lt;b&gt; stays &amp; literal",
        ];
        let stops: Vec<_> = hostile
            .iter()
            .enumerate()
            .map(|(position, notes)| stop(position + 1, notes))
            .collect();
        let manifest =
            Manifest::new("</title><script>x</script>", (1280, 720), 30, &stops).unwrap();
        let html = manifest.index_html();
        let block = data_block(&html);
        assert_eq!(
            block,
            manifest.to_inline_json(),
            "the whole manifest is inside"
        );
        for forbidden in ['<', '>', '&', '\u{2028}', '\u{2029}'] {
            assert!(
                !block.contains(forbidden),
                "{forbidden:?} in the data block"
            );
        }
        assert!(block.contains(r"\u003c/script\u003e"));
        assert!(block.contains(r"\u003c!--\u003cscript\u003e"));
        assert!(block.contains(r"\u2028") && block.contains(r"\u2029"));
        // The page has no other generated text: the title is set by app.js.
        assert_eq!(html.replacen(block, DATA_MARKER, 1), INDEX_HTML);
    }

    #[test]
    fn the_text_in_the_dom_equals_the_source_text() {
        // What app.js does: JSON.parse(block.textContent), then textContent = notes. A
        // data block's text is its raw characters (no entity decoding), so parsing the
        // block must give back every source string unchanged.
        let notes = "</script> <!--<script> \u{2028} &amp; <b>bold</b> \"quoted\" \\ é 🎉";
        let title = "<i>Q&A</i> \u{2029}";
        let manifest = Manifest::new(
            title,
            (1280, 720),
            30,
            &[DeckStop {
                video_frame: 10,
                title: Some(title.into()),
                notes: Some(notes.into()),
            }],
        )
        .unwrap();
        let html = manifest.index_html();
        let parsed: serde_json::Value = serde_json::from_str(data_block(&html)).unwrap();
        assert_eq!(parsed["title"], title);
        assert_eq!(parsed["slides"][0]["title"], title);
        assert_eq!(parsed["slides"][0]["notes"], notes);
        assert_eq!(parsed["slides"][0]["stop_frame"], 10);
        assert_eq!(parsed["version"], 1);
    }

    #[test]
    fn the_page_sets_a_content_security_policy_and_loads_only_its_own_files() {
        assert!(INDEX_HTML.contains(
            r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; media-src 'self'">"#
        ));
        assert_eq!(INDEX_HTML.matches(DATA_MARKER).count(), 1);
        for needle in [
            "http:",
            "https:",
            "//cdn",
            "fetch(",
            "XMLHttpRequest",
            "eval(",
        ] {
            assert!(!INDEX_HTML.contains(needle), "{needle} in index.html");
            assert!(!APP_JS.contains(needle), "{needle} in app.js");
        }
        assert!(
            !APP_JS.contains("innerHTML"),
            "titles and notes use textContent"
        );
        assert!(STYLE_CSS.contains("@media print"));
    }
}
