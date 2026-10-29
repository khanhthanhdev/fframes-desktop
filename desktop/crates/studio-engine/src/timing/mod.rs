//! The project's timing document (`timeline.json`) as an open project edits it: the
//! document with its own revision, split/trim/move edits, snapshot undo and redo, and
//! the write at the end of a gesture that never touches the build.
//!
//! The session lives on the [`crate::Controller`], one per open project: switching
//! projects starts a new history, and the history is not kept across restarts.
pub mod doc;
pub mod edits;
pub mod file;
pub mod history;

use std::path::Path;

pub use doc::{CodeScene, TimingClip, TimingDoc};
pub use edits::{ResolvedClip, TrimEdge};
use file::KnownFile;

use crate::TimingEditing;

/// An edit of the timeline, by clip position in [`TimelineSnapshot::clips`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineCommand {
    /// Splits the clip `at` frames after its first played frame.
    Split {
        clip: usize,
        at: usize,
    },
    /// Cuts `frames` frames of the clip's scene at `edge`.
    Trim {
        clip: usize,
        edge: TrimEdge,
        frames: usize,
    },
    /// Moves the clip to position `to`.
    Move {
        clip: usize,
        to: usize,
    },
    /// Starts a drag: its edits preview live and become one undo step and one write.
    BeginGesture,
    EndGesture,
    /// Undoes the latest timing or slides step: both documents share one history.
    Undo,
    Redo,
}

/// What the timeline shows and what the preview plays.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineSnapshot {
    /// Changes with every change of the document the preview should play.
    pub generation: u64,
    /// The document for `SetTiming`; `None` plays the code order.
    pub timing_json: Option<String>,
    /// The clips as they play; empty until a preview of the current build reported its
    /// scenes.
    pub clips: Vec<ResolvedClip>,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Whether edits are possible now, with the reason when not.
    pub editing: TimingEditing,
    /// Why the document was reloaded or could not be read, until the next edit.
    pub notice: Option<String>,
}

/// The scenes of one build, learned from its preview.
#[derive(Debug, Clone)]
struct LearnedScenes {
    /// The project state generation of the build that reported them.
    generation: u64,
    scenes: Result<Vec<CodeScene>, String>,
}

/// The timing document of the open project. Its edit history is shared with the slides
/// document and lives on the controller ([`history::RunTimeDoc`]).
#[derive(Debug)]
pub struct TimingSession {
    /// The document edits start from; `Err` when the live file cannot be read as one.
    doc: Result<TimingDoc, String>,
    /// The live file as last loaded or written (`None`: the project has none). An
    /// inventory entry with this hash is the session's own write.
    known: Option<KnownFile>,
    generation: u64,
    scenes: Option<LearnedScenes>,
    notice: Option<String>,
}

impl TimingSession {
    /// Loads the live file of the project at `root`.
    pub fn load(root: &Path) -> Self {
        let mut session = Self {
            doc: Ok(TimingDoc::default()),
            known: None,
            generation: 0,
            scenes: None,
            notice: None,
        };
        session.read(root);
        session
    }

    fn read(&mut self, root: &Path) {
        self.generation += 1;
        match file::read(root) {
            Ok(None) => {
                self.doc = Ok(TimingDoc::default());
                self.known = None;
            }
            Ok(Some(live)) => {
                self.doc = live.text.and_then(|json| TimingDoc::from_json(&json));
                self.known = Some(live.known);
            }
            Err(error) => {
                self.doc = Err(format!(
                    "{} could not be read: {error}",
                    studio_project::revision::TIMING_FILE
                ));
                self.known = None;
            }
        }
    }

    /// Reloads after an outside change and shows `reason`. The caller clears the
    /// document's steps from the shared history (a running gesture is aborted).
    pub fn reload(&mut self, root: &Path, reason: String) {
        self.read(root);
        self.notice = Some(reason);
    }

    pub fn doc(&self) -> Result<&TimingDoc, String> {
        self.doc.as_ref().map_err(Clone::clone)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn known(&self) -> Option<&KnownFile> {
        self.known.as_ref()
    }

    pub(crate) fn set_known(&mut self, known: KnownFile) {
        self.known = Some(known);
    }

    /// Whether an inventory hash of the live file (`None`: no file) is what the session
    /// last loaded or wrote.
    pub fn is_known(&self, sha256: Option<&str>) -> bool {
        self.known.as_ref().map(|known| known.sha256.as_str()) == sha256
    }

    /// Replaces the document the preview plays.
    pub(crate) fn set_doc(&mut self, doc: TimingDoc) {
        self.doc = Ok(doc);
        self.generation += 1;
        self.notice = None;
    }

    /// The document for the preview and exports; `None` plays the code order. An
    /// unreadable file plays the code order too (its reason is the notice).
    pub fn timing_json(&self) -> Option<String> {
        match &self.doc {
            Ok(doc) if self.known.is_some() || !doc.clips.is_empty() => Some(doc.to_json()),
            _ => None,
        }
    }

    /// Records the scenes a preview of the build at `generation` reported.
    pub(crate) fn learn_scenes(&mut self, generation: u64, scenes: Result<Vec<CodeScene>, String>) {
        self.scenes = Some(LearnedScenes { generation, scenes });
    }

    /// The scenes of the build at `generation`, when its preview reported them.
    pub(crate) fn scenes(&self, generation: u64) -> Option<Result<&[CodeScene], String>> {
        self.scenes
            .as_ref()
            .filter(|learned| learned.generation == generation)
            .map(|learned| learned.scenes.as_deref().map_err(Clone::clone))
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }
}
