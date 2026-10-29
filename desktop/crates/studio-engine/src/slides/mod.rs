//! The project's slides document (`slides.json`) as an open project edits it: slide stops
//! with titles and speaker notes, resolved against the timeline the person sees. It is a
//! run-time document like the props ([`crate::props`]): written at the end of a gesture
//! without touching the build, read and written with the timing document's file code.
//! Its undo steps share one history with the timeline's
//! ([`crate::timing::history::RunTimeDoc`]).
//!
//! The session lives on the [`crate::Controller`], one per open project.
pub mod deck;
pub mod doc;
pub mod resolve;
pub mod suggest;

use std::path::Path;

pub use deck::{Deck, DeckStep};
pub use doc::{SlideStop, SlidesDoc, StopAt};
use fframes_studio_protocol::PreviewTimelineResponse;
pub use resolve::{ResolvedStop, SlidesProblem, StopSource};
use studio_project::revision::SLIDES_FILE;

use crate::timing::file::{self, DocFile, KnownFile};

/// Where staged and displaced copies of `slides.json` live, relative to the project root.
pub const SLIDES_CACHE: &str = ".fframes/cache/slides";

/// `slides.json`.
pub const SLIDES: DocFile = DocFile {
    name: SLIDES_FILE,
    cache: SLIDES_CACHE,
    max_bytes: doc::MAX_SLIDES_DOC_BYTES,
    view: "the slides",
    valid: |json| SlidesDoc::parse(json).is_ok(),
};

/// An edit of the slides document, by stop position in [`SlidesDoc::stops`]. Undo and
/// redo are the timeline's ([`crate::timing::TimelineCommand::Undo`]): one history holds
/// both documents' steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlidesCommand {
    /// Replaces the whole document.
    SetSlides(SlidesDoc),
    /// Adds a stop at the end of the list (the deck orders stops by video frame).
    AddStop(SlideStop),
    /// Explicit acceptance of proposals from this exact preview layout, as one undo step.
    AcceptSuggestions {
        identity: fframes_studio_protocol::PreviewIdentity,
        timing_generation: u64,
        props_generation: u64,
        stops: Vec<SlideStop>,
    },
    /// Moves a stop to another scene frame.
    MoveStop {
        stop: usize,
        scene: String,
        at: StopAt,
    },
    RemoveStop {
        stop: usize,
    },
    /// Sets a stop's title and notes; `None` or an empty text removes it.
    SetStopText {
        stop: usize,
        title: Option<String>,
        notes: Option<String>,
    },
    /// Writes the default stops (`"end"` of every clip of the timeline the person sees)
    /// into a document that has none, so they can be edited one by one.
    MaterializeDefaultStops,
    /// Starts a drag: its edits preview live and become one undo step and one write.
    BeginGesture,
    EndGesture,
}

/// Whether Studio may change the project's slides (`slides.json`) now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlidesEditing {
    Enabled,
    /// Disabled with the reason to show next to the controls.
    Disabled(String),
}

/// What the Slides tab, the timeline markers, Present mode and the deck export show.
#[derive(Debug, Clone, PartialEq)]
pub struct SlidesSnapshot {
    /// Changes with every change of the document.
    pub generation: u64,
    /// The document edits start from; `None` when the live file is invalid.
    pub doc: Option<SlidesDoc>,
    /// The stops at video frames of the timeline the person sees, sorted, ending with
    /// the video's last frame. Empty until a preview reported its timeline.
    pub stops: Vec<ResolvedStop>,
    /// What did not resolve, merged or was dropped.
    pub problems: Vec<SlidesProblem>,
    /// Why no stops resolved, when none could: no preview yet, a stale layout, an
    /// invalid document.
    pub unavailable: Option<String>,
    pub editing: SlidesEditing,
    /// Why the document was reloaded or could not be read, until the next edit.
    pub notice: Option<String>,
}

/// The slides document of the open project and the timeline its stops resolve against.
#[derive(Debug)]
pub struct SlidesSession {
    /// The document edits start from; `Err` when the live file cannot be read as one.
    doc: Result<SlidesDoc, String>,
    /// The live file as last loaded or written (`None`: the project has none).
    known: Option<KnownFile>,
    generation: u64,
    /// The latest laid-out timeline of the displayed preview (a build's or a retime's).
    laid_out: Option<(u64, PreviewTimelineResponse)>,
    notice: Option<String>,
}

impl SlidesSession {
    /// Loads the live file of the project at `root`.
    pub fn load(root: &Path) -> Self {
        let mut session = Self {
            doc: Ok(SlidesDoc::default()),
            known: None,
            generation: 0,
            laid_out: None,
            notice: None,
        };
        session.read(root);
        session
    }

    fn read(&mut self, root: &Path) {
        self.generation += 1;
        match file::read_doc(root, &SLIDES) {
            Ok(None) => {
                self.doc = Ok(SlidesDoc::default());
                self.known = None;
            }
            Ok(Some(live)) => {
                self.doc = live.text.and_then(|json| {
                    SlidesDoc::parse(&json).map_err(|error| format!("{SLIDES_FILE}: {error}"))
                });
                self.known = Some(live.known);
            }
            Err(error) => {
                self.doc = Err(format!("{SLIDES_FILE} could not be read: {error}"));
                self.known = None;
            }
        }
    }

    /// Reloads after an outside change and shows `reason`. The caller clears the
    /// document's steps from the shared history.
    pub fn reload(&mut self, root: &Path, reason: String) {
        self.read(root);
        self.notice = Some(reason);
    }

    pub fn doc(&self) -> Result<&SlidesDoc, String> {
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

    pub(crate) fn set_doc(&mut self, doc: SlidesDoc) {
        self.doc = Ok(doc);
        self.generation += 1;
        self.notice = None;
    }

    /// Records the timeline the displayed preview laid out.
    pub(crate) fn learn_layout(&mut self, generation: u64, timeline: &PreviewTimelineResponse) {
        self.laid_out = Some((generation, timeline.clone()));
    }

    /// The latest laid-out timeline, when both its build and timing are current.
    pub(crate) fn layout(
        &self,
        generation: u64,
        timing_generation: u64,
    ) -> Result<&PreviewTimelineResponse, String> {
        match &self.laid_out {
            None => Err("Build the preview to place the slides on the timeline".into()),
            Some((built, _)) if *built != generation => {
                Err("The preview has not laid out the current build yet".into())
            }
            Some((_, timeline)) if timeline.timing_generation != timing_generation => {
                Err("The preview has not laid out the current timeline yet".into())
            }
            Some((_, timeline)) => Ok(timeline),
        }
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_the_default_document_and_invalid_files_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let session = SlidesSession::load(root);
        assert_eq!(session.doc().unwrap(), &SlidesDoc::default());
        assert!(session.is_known(None));

        std::fs::write(root.join(SLIDES_FILE), "{\"version\":2,\"stops\":[]}").unwrap();
        let session = SlidesSession::load(root);
        let error = session.doc().unwrap_err();
        assert!(error.starts_with(SLIDES_FILE), "{error}");
        assert!(!session.is_known(None));
    }

    #[test]
    fn an_interrupted_write_is_recovered() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let doc = SlidesDoc {
            version: 1,
            stops: vec![SlideStop {
                scene: "intro".into(),
                at: StopAt::End,
                title: Some("Welcome".into()),
                notes: None,
            }],
        };
        let crashed = file::write_doc(root, &SLIDES, &doc.to_json(), None, &mut |_| {
            Err(std::io::Error::other("crash"))
        });
        assert!(crashed.is_err());
        let note = file::recover_doc(root, &SLIDES).unwrap().unwrap();
        assert!(note.contains(SLIDES_FILE), "{note}");
        assert_eq!(SlidesSession::load(root).doc().unwrap(), &doc);
    }
}
