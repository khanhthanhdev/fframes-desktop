//! The slides document (`slides.json`): the stops where a slide deck holds, with their
//! titles and speaker notes.
//!
//! A stop names a clip by its timing key (`timeline.json`'s `key`) and a frame in the
//! scene's own frame numbers, or `"end"`. Scene frames do not move when a clip is
//! trimmed, and `"end"` follows the trims, so the document needs no rewrite after a
//! timing edit. It is checked whole: any error rejects all of it.
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::timing::doc::MAX_TIMING_KEY_BYTES;

/// The only document version Studio reads.
pub const SLIDES_DOC_VERSION: u32 = 1;
/// Upper bound on a serialized slides document (the budget for everything in it).
pub const MAX_SLIDES_DOC_BYTES: usize = 512 * 1024;
/// Upper bound on stops in one document.
pub const MAX_STOPS: usize = 300;
/// Upper bound on the bytes of one stop's title.
pub const MAX_TITLE_BYTES: usize = 256;
/// Upper bound on the bytes of one stop's notes.
pub const MAX_NOTES_BYTES: usize = 8 * 1024;

/// Slide stops and speaker notes. No stops means the default stops: the end of every
/// clip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlidesDoc {
    pub version: u32,
    pub stops: Vec<SlideStop>,
}

impl Default for SlidesDoc {
    /// No stops: the default stops.
    fn default() -> Self {
        Self {
            version: SLIDES_DOC_VERSION,
            stops: Vec::new(),
        }
    }
}

/// One stop: the deck holds on this frame until the presenter goes on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlideStop {
    /// The clip's timing key (`editor_instance_key()`, else the scene's `name()`).
    pub scene: String,
    pub at: StopAt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Where in its scene a stop is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopAt {
    /// A frame in the scene's own frame numbers (trims do not move it).
    Frame(usize),
    /// The clip's last frame before the next clip starts to overlap it.
    End,
}

impl Serialize for StopAt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Frame(frame) => serializer.serialize_u64(*frame as u64),
            Self::End => serializer.serialize_str("end"),
        }
    }
}

impl<'de> Deserialize<'de> for StopAt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = StopAt;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a scene frame number or \"end\"")
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<StopAt, E> {
                usize::try_from(value)
                    .map(StopAt::Frame)
                    .map_err(|_| E::custom("the frame number is too large"))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<StopAt, E> {
                u64::try_from(value)
                    .map_err(|_| E::custom("a frame number cannot be negative"))
                    .and_then(|value| self.visit_u64(value))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<StopAt, E> {
                if value == "end" {
                    Ok(StopAt::End)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(value), &self))
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

impl SlidesDoc {
    /// Parses and validates a serialized document; any error rejects all of it.
    pub fn parse(json: &str) -> Result<Self, String> {
        if json.len() > MAX_SLIDES_DOC_BYTES {
            return Err(format!(
                "the slides document of {} bytes exceeds the limit of {MAX_SLIDES_DOC_BYTES}",
                json.len()
            ));
        }
        let doc: Self = serde_json::from_str(json)
            .map_err(|error| format!("the slides document is malformed: {error}"))?;
        doc.validate()?;
        Ok(doc)
    }

    /// Every rule but the size of the encoded document (see [`SlidesDoc::check_size`]).
    pub fn validate(&self) -> Result<(), String> {
        if self.version != SLIDES_DOC_VERSION {
            return Err(format!(
                "slides version {} is not supported (expected {SLIDES_DOC_VERSION})",
                self.version
            ));
        }
        if self.stops.len() > MAX_STOPS {
            return Err(format!(
                "{} stops exceed the limit of {MAX_STOPS}",
                self.stops.len()
            ));
        }
        for (index, stop) in self.stops.iter().enumerate() {
            let number = index + 1;
            if stop.scene.is_empty()
                || stop.scene.len() > MAX_TIMING_KEY_BYTES
                || stop.scene.chars().any(char::is_control)
            {
                return Err(format!(
                    "stop {number} has an invalid scene key {:?}",
                    stop.scene
                ));
            }
            if let Some(title) = &stop.title {
                if title.len() > MAX_TITLE_BYTES {
                    return Err(format!(
                        "the title of stop {number} has {} bytes, more than the limit of {MAX_TITLE_BYTES}",
                        title.len()
                    ));
                }
                if title.chars().any(char::is_control) {
                    return Err(format!(
                        "the title of stop {number} contains a control character"
                    ));
                }
            }
            if let Some(notes) = &stop.notes
                && notes.len() > MAX_NOTES_BYTES
            {
                return Err(format!(
                    "the notes of stop {number} have {} bytes, more than the limit of {MAX_NOTES_BYTES}",
                    notes.len()
                ));
            }
        }
        Ok(())
    }

    /// Whether the document as written stays within [`MAX_SLIDES_DOC_BYTES`]; edits
    /// check it before they write, like the props document.
    pub fn check_size(&self) -> Result<(), String> {
        let bytes = self.to_json().len();
        if bytes > MAX_SLIDES_DOC_BYTES {
            return Err(format!(
                "slides.json would have {bytes} bytes, more than the limit of {MAX_SLIDES_DOC_BYTES}"
            ));
        }
        Ok(())
    }

    /// The bytes written to `slides.json`: pretty JSON with a final newline, stable for
    /// equal documents.
    pub fn to_json(&self) -> String {
        let mut json =
            serde_json::to_string_pretty(self).expect("a slides document always serializes");
        json.push('\n');
        json
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(scene: &str, at: StopAt) -> SlideStop {
        SlideStop {
            scene: scene.into(),
            at,
            title: None,
            notes: None,
        }
    }

    #[test]
    fn documents_parse_round_trip_and_write_stably() {
        let json = r#"{
            "version": 1,
            "stops": [
                { "scene": "intro", "at": "end", "title": "Welcome", "notes": "Say hello." },
                { "scene": "features", "at": 40, "notes": "First bullet." }
            ]
        }"#;
        let doc = SlidesDoc::parse(json).unwrap();
        assert_eq!(doc.stops[0].at, StopAt::End);
        assert_eq!(doc.stops[0].title.as_deref(), Some("Welcome"));
        assert_eq!(doc.stops[1].at, StopAt::Frame(40));
        assert_eq!(doc.stops[1].title, None);
        let written = doc.to_json();
        assert!(written.ends_with('\n'));
        assert!(!written.contains("null"), "absent fields stay absent");
        assert_eq!(SlidesDoc::parse(&written).unwrap(), doc);
        assert_eq!(SlidesDoc::parse(&written).unwrap().to_json(), written);
        assert_eq!(
            SlidesDoc::parse(r#"{"version":1,"stops":[]}"#).unwrap(),
            SlidesDoc::default()
        );
    }

    #[test]
    fn malformed_documents_are_refused_whole() {
        for bad in [
            r#"{"version":2,"stops":[]}"#,
            r#"{"version":1,"stops":[],"extra":1}"#,
            r#"{"version":1}"#,
            r#"{"version":1,"stops":[{"scene":"a","at":"end","color":"red"}]}"#,
            r#"{"version":1,"stops":[{"scene":"a","at":"start"}]}"#,
            r#"{"version":1,"stops":[{"scene":"a","at":-1}]}"#,
            r#"{"version":1,"stops":[{"scene":"a","at":1.5}]}"#,
            r#"{"version":1,"stops":[{"scene":"a"}]}"#,
            r#"{"version":1,"stops":[{"scene":"","at":0}]}"#,
            r#"{"version":1,"stops":[{"scene":"a\nb","at":0}]}"#,
            r#"{"version":1,"stops":[{"scene":"a","at":3,"title":"two\nlines"}]}"#,
            "not json",
        ] {
            assert!(SlidesDoc::parse(bad).is_err(), "{bad}");
        }
        // Notes may span lines.
        SlidesDoc::parse(r#"{"version":1,"stops":[{"scene":"a","at":3,"notes":"one\ntwo"}]}"#)
            .unwrap();
    }

    #[test]
    fn every_limit_is_enforced() {
        let doc = |stops: Vec<SlideStop>| SlidesDoc { version: 1, stops };
        let at_limit = doc(vec![stop("a", StopAt::End); MAX_STOPS]);
        SlidesDoc::parse(&at_limit.to_json()).unwrap();
        let over = doc(vec![stop("a", StopAt::End); MAX_STOPS + 1]);
        assert!(
            SlidesDoc::parse(&over.to_json())
                .unwrap_err()
                .contains("stops exceed")
        );

        let mut titled = stop("a", StopAt::End);
        titled.title = Some("t".repeat(MAX_TITLE_BYTES));
        doc(vec![titled.clone()]).validate().unwrap();
        titled.title = Some("t".repeat(MAX_TITLE_BYTES + 1));
        assert!(doc(vec![titled]).validate().unwrap_err().contains("title"));

        let mut noted = stop("a", StopAt::End);
        noted.notes = Some("n".repeat(MAX_NOTES_BYTES));
        doc(vec![noted.clone()]).validate().unwrap();
        noted.notes = Some("n".repeat(MAX_NOTES_BYTES + 1));
        assert!(doc(vec![noted]).validate().unwrap_err().contains("notes"));

        doc(vec![stop(&"k".repeat(MAX_TIMING_KEY_BYTES), StopAt::End)])
            .validate()
            .unwrap();
        assert!(
            doc(vec![stop(
                &"k".repeat(MAX_TIMING_KEY_BYTES + 1),
                StopAt::End
            )])
            .validate()
            .is_err()
        );

        // Each field within its limit, the whole over the shared budget.
        let mut full = stop("a", StopAt::End);
        full.notes = Some("n".repeat(MAX_NOTES_BYTES));
        let large = doc(vec![full; MAX_SLIDES_DOC_BYTES / MAX_NOTES_BYTES + 1]);
        large.validate().unwrap();
        assert!(large.check_size().unwrap_err().contains("limit"));
        assert!(
            SlidesDoc::parse(&large.to_json())
                .unwrap_err()
                .contains("exceeds")
        );
    }
}
