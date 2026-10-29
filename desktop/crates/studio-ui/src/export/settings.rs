//! What the export panel may offer for the open project: the answer of its worker, or the
//! reason there is none.

use fframes_studio_protocol::export::{
    ExportCapabilities, ExportCodec, ExportContainer, ExportFormat, ExportSettings,
};
use studio_engine::export_options::{FORMATS, available_formats, usable_settings};
use studio_engine::slides::{SlidesProblem, SlidesSnapshot};

/// What an export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportTarget {
    /// One video file.
    #[default]
    Video,
    /// A folder that plays the video as a slide deck in a browser.
    Deck,
}

/// Shown on macOS and Linux until their SDK has an H.264 encoder.
pub const DECK_NEEDS_H264: &str =
    "Slide deck export needs an H.264 encoder, which this SDK does not include";

/// The format of every deck: H.264 in MP4.
pub const DECK_FORMAT: ExportFormat = ExportFormat {
    container: ExportContainer::Mp4,
    codec: ExportCodec::H264,
};

/// A deck's settings for the size of `choice`: H.264 in MP4 at the video's frame rate.
pub fn deck_settings(choice: ExportSettings) -> ExportSettings {
    ExportSettings {
        container: DECK_FORMAT.container,
        codec: DECK_FORMAT.codec,
        scale: choice.scale,
        fps: None,
    }
}

/// The slides a deck export would have, from the open project's slides snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeckSummary {
    pub slides: usize,
    /// Stops that did not resolve and are left out of the deck.
    pub skipped: Vec<String>,
    /// Why no stop resolved (no preview yet, a stale layout, an invalid document).
    pub unavailable: Option<String>,
}

impl DeckSummary {
    pub fn of(snapshot: Option<&SlidesSnapshot>) -> Self {
        let Some(snapshot) = snapshot else {
            return Self {
                unavailable: Some("Open a project to export its slides".into()),
                ..Self::default()
            };
        };
        Self {
            slides: snapshot.stops.len(),
            skipped: snapshot
                .problems
                .iter()
                .filter(|problem| problem.is_unresolved())
                .map(ToString::to_string)
                .collect(),
            unavailable: snapshot.unavailable.clone().or_else(|| {
                snapshot.stops.is_empty().then(|| {
                    snapshot
                        .problems
                        .iter()
                        .find(|problem| matches!(problem, SlidesProblem::WorkerTooOld))
                        .map_or_else(
                            || "Build the preview to place the slides on the timeline".into(),
                            ToString::to_string,
                        )
                })
            }),
        }
    }
}

/// Why a deck export cannot start now, or `None` when it can.
pub fn deck_blocker(support: &ExportSupport, deck: &DeckSummary) -> Option<String> {
    match support {
        ExportSupport::Ready(capabilities) if !capabilities.formats.contains(&DECK_FORMAT) => {
            return Some(DECK_NEEDS_H264.into());
        }
        ExportSupport::Unavailable(reason) => {
            return Some(format!(
                "The project's renderer could not be asked: {reason}"
            ));
        }
        _ => {}
    }
    deck.unavailable
        .as_ref()
        .map(|reason| format!("No timeline is laid out for the slides · {reason}"))
}

/// What is known about how the open project can be exported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ExportSupport {
    /// Nothing asked yet.
    #[default]
    Unknown,
    /// The worker is being asked.
    Probing,
    /// The worker answered: its formats and the video's size and frame rate.
    Ready(Box<ExportCapabilities>),
    /// The worker could not be asked; the export itself reports why it cannot run.
    Unavailable(String),
}

impl ExportSupport {
    /// The worker's answer, when there is one.
    pub fn capabilities(&self) -> Option<&ExportCapabilities> {
        match self {
            Self::Ready(capabilities) => Some(capabilities),
            _ => None,
        }
    }

    /// The panel's controls change the export.
    pub fn is_editable(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    /// An export may start: the options are known, or the worker could not be asked.
    pub fn can_start(&self) -> bool {
        !matches!(self, Self::Unknown | Self::Probing)
    }

    /// The formats to offer: the worker's, or only the default one without an answer.
    pub fn formats(&self) -> Vec<ExportFormat> {
        match self {
            Self::Ready(capabilities) => available_formats(capabilities),
            _ => vec![FORMATS[0]],
        }
    }

    /// What an export runs with when `saved` is the user's choice.
    pub fn export_settings(&self, saved: ExportSettings) -> ExportSettings {
        match self {
            Self::Ready(capabilities) => usable_settings(saved, capabilities),
            _ => ExportSettings::default(),
        }
    }

    /// A short name for telemetry.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Probing => "probing",
            Self::Ready(_) => "ready",
            Self::Unavailable(_) => "unavailable",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fframes_studio_protocol::export::{ExportCodec, ExportContainer};

    fn ready(formats: &[ExportFormat]) -> ExportSupport {
        ExportSupport::Ready(Box::new(ExportCapabilities {
            version: 1,
            width: 1920,
            height: 1080,
            fps: 60,
            timing: true,
            props: false,
            formats: formats.to_vec(),
        }))
    }

    fn webm() -> ExportSettings {
        ExportSettings {
            container: ExportContainer::Webm,
            codec: ExportCodec::Vp9,
            scale: 0.5,
            fps: Some(30),
        }
    }

    #[test]
    fn an_export_starts_once_the_options_are_known() {
        assert!(!ExportSupport::Unknown.can_start());
        assert!(!ExportSupport::Probing.can_start());
        assert!(ExportSupport::Unavailable("x".into()).can_start());
        assert!(ready(&FORMATS).can_start());
    }

    #[test]
    fn only_a_worker_that_answered_lets_the_settings_change() {
        assert!(ready(&FORMATS).is_editable());
        assert!(!ExportSupport::Unavailable("x".into()).is_editable());
        assert!(!ExportSupport::Probing.is_editable());
    }

    #[test]
    fn without_an_answer_only_the_default_is_exported() {
        let unavailable = ExportSupport::Unavailable("x".into());
        assert_eq!(unavailable.formats(), [FORMATS[0]]);
        assert_eq!(
            unavailable.export_settings(webm()),
            ExportSettings::default()
        );
    }

    fn snapshot(stops: usize, unavailable: Option<&str>) -> SlidesSnapshot {
        SlidesSnapshot {
            generation: 1,
            doc: None,
            stops: (1..=stops)
                .map(|n| studio_engine::slides::ResolvedStop {
                    video_frame: n * 10,
                    title: None,
                    notes: None,
                    source: studio_engine::slides::StopSource::Default,
                })
                .collect(),
            problems: vec![SlidesProblem::Unresolved {
                stop: 3,
                reason: "no clip plays scene \"gone\"".into(),
            }],
            unavailable: unavailable.map(Into::into),
            editing: studio_engine::slides::SlidesEditing::Enabled,
            notice: None,
        }
    }

    #[test]
    fn a_deck_is_h264_at_the_video_rate_and_keeps_the_size() {
        assert_eq!(
            deck_settings(webm()),
            ExportSettings {
                container: ExportContainer::Mp4,
                codec: ExportCodec::H264,
                scale: 0.5,
                fps: None,
            }
        );
    }

    #[test]
    fn a_deck_needs_h264_and_a_laid_out_timeline() {
        let deck = DeckSummary::of(Some(&snapshot(3, None)));
        assert_eq!(deck.slides, 3);
        assert_eq!(deck.skipped.len(), 1);
        assert!(deck.skipped[0].contains("stop 4"), "{:?}", deck.skipped);
        assert_eq!(deck_blocker(&ready(&FORMATS), &deck), None);
        assert_eq!(
            deck_blocker(&ready(&[FORMATS[0]]), &deck).as_deref(),
            Some(DECK_NEEDS_H264)
        );
        let stale = DeckSummary::of(Some(&snapshot(0, Some("not laid out"))));
        assert!(
            deck_blocker(&ready(&FORMATS), &stale)
                .unwrap()
                .contains("not laid out")
        );
        let empty = DeckSummary::of(Some(&snapshot(0, None)));
        assert!(deck_blocker(&ready(&FORMATS), &empty).is_some());
        assert!(deck_blocker(&ExportSupport::Unavailable("x".into()), &deck).is_some());
    }

    #[test]
    fn a_saved_format_the_worker_lacks_falls_back_to_the_default_format() {
        assert_eq!(ready(&FORMATS).export_settings(webm()), webm());
        assert_eq!(
            ready(&[FORMATS[0]]).export_settings(webm()),
            ExportSettings {
                scale: 0.5,
                fps: Some(30),
                ..ExportSettings::default()
            }
        );
    }
}
