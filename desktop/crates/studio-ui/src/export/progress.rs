//! Where an export is, as the panel shows it; the panel draws it with `kit::Progress`.

use std::path::PathBuf;

use studio_engine::services::export_service::ExportProgress;

/// The stage of the export, folded from the engine's progress events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportPhase {
    /// No export ran since the project opened, or the last one was dismissed.
    Idle,
    /// Freezing the source, compiling the worker, starting the encoder.
    Preparing,
    Rendering {
        done: usize,
        total: usize,
    },
    /// The frames are written; the audio and the file are being checked.
    Finishing,
    /// A slide deck's stop frames are decoded into posters.
    Posters {
        done: usize,
        total: usize,
    },
    /// A slide deck's page is written and its folder published.
    WritingDeck,
    Complete {
        destination: PathBuf,
    },
    Failed(String),
    Cancelled,
}

impl ExportPhase {
    pub fn of(status: Option<&ExportProgress>) -> Self {
        match status {
            None => Self::Idle,
            Some(ExportProgress::Started { .. } | ExportProgress::Warning(_)) => Self::Preparing,
            Some(ExportProgress::Rendering { done, total }) => Self::Rendering {
                done: *done,
                total: *total,
            },
            Some(ExportProgress::Audio | ExportProgress::Verifying) => Self::Finishing,
            Some(ExportProgress::Posters { done, total }) => Self::Posters {
                done: *done,
                total: *total,
            },
            Some(ExportProgress::WritingDeck) => Self::WritingDeck,
            Some(ExportProgress::Complete { destination, .. }) => Self::Complete {
                destination: destination.clone(),
            },
            Some(ExportProgress::Failed(reason)) => Self::Failed(reason.clone()),
            Some(ExportProgress::Cancelled) => Self::Cancelled,
        }
    }

    /// An export is running: settings are locked and Cancel is offered.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Preparing
                | Self::Rendering { .. }
                | Self::Finishing
                | Self::Posters { .. }
                | Self::WritingDeck
        )
    }

    /// The bar has a position to show.
    pub fn shows_progress(&self) -> bool {
        matches!(
            self,
            Self::Rendering { .. } | Self::Finishing | Self::Posters { .. } | Self::WritingDeck
        )
    }

    /// How far along the export is, in `0..=1`; `None` before there is a position.
    pub fn fraction(&self) -> Option<f32> {
        match self {
            Self::Rendering { done, total } | Self::Posters { done, total } if *total > 0 => {
                Some((*done as f32 / *total as f32).clamp(0., 1.))
            }
            Self::Rendering { .. } | Self::Posters { .. } => Some(0.),
            Self::Finishing | Self::WritingDeck => Some(1.),
            _ => None,
        }
    }

    /// The line under the bar, or under the buttons once the export ended.
    pub fn message(&self) -> Option<String> {
        Some(match self {
            Self::Idle => return None,
            Self::Preparing => "Preparing the export".into(),
            Self::Rendering { done, total } => format!("Rendering · {done}/{total} frames"),
            Self::Finishing => "Writing and verifying the file".into(),
            Self::Posters { done, total } => format!("Slide posters · {done}/{total}"),
            Self::WritingDeck => "Writing and publishing the deck folder".into(),
            Self::Complete { destination } => format!(
                "Exported {}",
                studio_bootstrap::external_path(destination).display()
            ),
            Self::Failed(reason) => format!("Export failed · {reason}"),
            Self::Cancelled => "Export cancelled · partial output removed".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_fold_into_phases() {
        let started = ExportProgress::Started {
            revision: "r".into(),
            label: "l".into(),
        };
        assert_eq!(ExportPhase::of(None), ExportPhase::Idle);
        assert_eq!(ExportPhase::of(Some(&started)), ExportPhase::Preparing);
        assert_eq!(
            ExportPhase::of(Some(&ExportProgress::Warning("w".into()))),
            ExportPhase::Preparing
        );
        assert_eq!(
            ExportPhase::of(Some(&ExportProgress::Rendering { done: 3, total: 9 })),
            ExportPhase::Rendering { done: 3, total: 9 }
        );
        assert_eq!(
            ExportPhase::of(Some(&ExportProgress::Audio)),
            ExportPhase::Finishing
        );
        assert_eq!(
            ExportPhase::of(Some(&ExportProgress::Verifying)),
            ExportPhase::Finishing
        );
        assert_eq!(
            ExportPhase::of(Some(&ExportProgress::Cancelled)),
            ExportPhase::Cancelled
        );
    }

    #[test]
    fn only_a_running_export_is_busy() {
        let busy = [
            ExportPhase::Preparing,
            ExportPhase::Rendering { done: 0, total: 1 },
            ExportPhase::Finishing,
        ];
        assert!(busy.iter().all(ExportPhase::is_busy));
        let idle = [
            ExportPhase::Idle,
            ExportPhase::Complete {
                destination: "a.mp4".into(),
            },
            ExportPhase::Failed("x".into()),
            ExportPhase::Cancelled,
        ];
        assert!(!idle.iter().any(ExportPhase::is_busy));
    }

    #[test]
    fn the_bar_follows_the_frames() {
        assert_eq!(ExportPhase::Preparing.fraction(), None);
        assert!(!ExportPhase::Preparing.shows_progress());
        let half = ExportPhase::Rendering {
            done: 45,
            total: 90,
        };
        assert!(half.shows_progress());
        assert_eq!(half.fraction(), Some(0.5));
        assert_eq!(
            ExportPhase::Rendering { done: 5, total: 0 }.fraction(),
            Some(0.)
        );
        assert_eq!(ExportPhase::Finishing.fraction(), Some(1.));
    }

    #[test]
    fn a_deck_export_stays_busy_through_its_posters_and_publication() {
        let posters = ExportPhase::of(Some(&ExportProgress::Posters { done: 1, total: 4 }));
        assert_eq!(posters, ExportPhase::Posters { done: 1, total: 4 });
        assert!(posters.is_busy() && posters.shows_progress());
        assert_eq!(posters.fraction(), Some(0.25));
        let writing = ExportPhase::of(Some(&ExportProgress::WritingDeck));
        assert!(writing.is_busy());
        assert!(writing.message().unwrap().contains("deck"));
    }

    #[test]
    fn a_failure_carries_its_reason() {
        let message = ExportPhase::Failed("no encoder".into()).message().unwrap();
        assert!(message.contains("no encoder"), "{message}");
        assert_eq!(ExportPhase::Idle.message(), None);
    }
}
