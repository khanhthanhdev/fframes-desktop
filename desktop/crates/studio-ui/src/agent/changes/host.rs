//! The review window: a [`Changes`] view fed from the engine and wired to its per-file review
//! calls. The viewer only emits what the reviewer chose; this is the owner that performs it.
//!
//! Every engine call here may take the controller lock or read checkpoint files, so each one
//! runs on the background executor. Their results arrive as snapshots, which the shell
//! forwards through [`ReviewHost::on_snapshot`].

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{
    AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Window, div, px,
};
use studio_engine::agent::{TaskPhase, WorkflowSnapshot};
use studio_engine::candidate_validation::FileDiff as EngineDiff;
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::paint::Colors;
use super::source::{Facts, Read, ReviewBackend, facts};
use super::{Changes, ChangesEvent, Review};
use crate::agent::ConversationPanel;

/// What a review window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTarget {
    /// The candidate waiting for Apply.
    Candidate,
    /// An applied revision, by its history id.
    Revision(String),
}

/// Asked of the shell by the conversation panel: open the review window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRequest(pub ReviewTarget);

impl EventEmitter<ReviewRequest> for ConversationPanel {}

impl ConversationPanel {
    /// Asks the shell to open the review window for `target`.
    pub fn request_review(&mut self, target: ReviewTarget, cx: &mut Context<Self>) {
        cx.emit(ReviewRequest(target));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewHostEvent {
    /// Leave the review window (the reviewer closed it or what it shows is gone).
    Close,
}

impl EventEmitter<ReviewHostEvent> for ReviewHost {}

/// What the reviewer started and the engine has not answered yet.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Operation {
    Rejecting {
        saw_validating: bool,
        key: Option<String>,
        newest_row: u64,
    },
    Reverting {
        path: String,
        newest_row: u64,
    },
    Discarding,
}

/// A diff read, ready to run off the UI thread.
type DiffRead = Box<dyn FnOnce() -> Result<Vec<EngineDiff>, String> + Send>;
type DiffResult = Result<Read, String>;

const REJECT_DONE: &str = "Rejected files were left out; the remaining files passed validation.";
const REJECT_UNCHANGED: &str = "The reject did not finish; the candidate is unchanged.";

pub struct ReviewHost {
    changes: Entity<Changes>,
    backend: Arc<dyn ReviewBackend>,
    target: ReviewTarget,
    /// Latest diff request; an older answer is dropped.
    serial: u64,
    /// The candidate the shown files belong to.
    key: Option<String>,
    loading: bool,
    reload: bool,
    op: Option<Operation>,
    facts: Facts,
    status: Option<String>,
}

impl ReviewHost {
    pub fn new(
        target: ReviewTarget,
        backend: Arc<dyn ReviewBackend>,
        facts: Facts,
        cx: &mut Context<Self>,
    ) -> Self {
        let changes = cx.new(|_| Changes::new(Vec::new()));
        cx.subscribe(&changes, Self::on_changes_event).detach();
        let review = match target {
            ReviewTarget::Candidate => Review::Pending,
            ReviewTarget::Revision(_) => Review::Applied,
        };
        changes.update(cx, |changes, cx| changes.set_review(review, cx));
        let mut host = Self {
            changes,
            backend,
            target,
            serial: 0,
            key: None,
            loading: false,
            reload: false,
            op: None,
            facts,
            status: None,
        };
        host.sync_changes(cx);
        host.maybe_load(cx);
        host
    }

    pub fn target(&self) -> &ReviewTarget {
        &self.target
    }

    pub fn changes(&self) -> &Entity<Changes> {
        &self.changes
    }

    /// The line above the diff, if there is something to say.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// What the window shows, for the observation interface.
    pub fn telemetry(&self, cx: &gpui::App) -> serde_json::Value {
        let changes = self.changes.read(cx);
        serde_json::json!({
            "target": match &self.target {
                ReviewTarget::Candidate => "candidate".to_owned(),
                ReviewTarget::Revision(revision) => format!("revision {revision}"),
            },
            "files": changes.file_count(),
            "more": changes.more(),
            "busy": changes.busy(),
            "status": self.status,
        })
    }

    fn revision(&self) -> Option<&str> {
        match &self.target {
            ReviewTarget::Candidate => None,
            ReviewTarget::Revision(revision) => Some(revision),
        }
    }

    /// Takes in a new workflow snapshot.
    pub fn on_snapshot(&mut self, snapshot: &WorkflowSnapshot, cx: &mut Context<Self>) {
        let next = facts(snapshot, self.revision());
        self.on_facts(next, cx);
    }

    pub fn on_facts(&mut self, next: Facts, cx: &mut Context<Self>) {
        self.facts = next;
        self.settle(cx);
        if matches!(self.target, ReviewTarget::Candidate) && !self.facts.task_active {
            // Applied, discarded or failed: there is no candidate left to review.
            cx.emit(ReviewHostEvent::Close);
            return;
        }
        self.sync_changes(cx);
        self.maybe_load(cx);
    }

    /// Ends the operation the snapshot has answered, and says how it went.
    fn settle(&mut self, cx: &mut Context<Self>) {
        let facts = &self.facts;
        let refusal = |newest_row: u64| {
            facts
                .refusal
                .as_ref()
                .filter(|(id, _)| *id > newest_row)
                .map(|(_, text)| text.clone())
        };
        let validating = facts.phase == Some(TaskPhase::Validating);
        let outcome = match &mut self.op {
            None | Some(Operation::Discarding) => return,
            Some(Operation::Rejecting {
                saw_validating,
                key,
                newest_row,
            }) => {
                *saw_validating |= validating;
                let answered = *saw_validating
                    || facts.candidate != *key
                    || facts.newest_row > *newest_row
                    || !facts.task_active;
                if validating || !answered {
                    None
                } else {
                    let replaced = facts.candidate != *key;
                    let text = refusal(*newest_row).unwrap_or_else(|| {
                        if replaced {
                            REJECT_DONE
                        } else {
                            REJECT_UNCHANGED
                        }
                        .into()
                    });
                    Some((text, true))
                }
            }
            Some(Operation::Reverting { path, newest_row }) => {
                if facts.reverted.contains(path) {
                    Some((format!("Reverted {path}."), false))
                } else if !facts.undo_in_progress {
                    refusal(*newest_row).map(|text| (text, false))
                } else {
                    None
                }
            }
        };
        if let Some((text, reload)) = outcome {
            self.op = None;
            self.status = Some(text);
            self.reload |= reload;
            cx.notify();
        }
    }

    fn busy(&self) -> bool {
        self.op.is_some()
            || self.facts.phase == Some(TaskPhase::Validating)
            || (self.revision().is_some()
                && (self.facts.task_active || self.facts.undo_in_progress))
    }

    fn sync_changes(&mut self, cx: &mut Context<Self>) {
        let busy = self.busy();
        let reverted: HashSet<String> = self.facts.reverted.iter().cloned().collect();
        self.changes.update(cx, |changes, cx| {
            changes.set_busy(busy, cx);
            changes.set_reverted(reverted, cx);
        });
        cx.notify();
    }

    /// Reads the diffs when the shown ones are not those of the current candidate. Never while
    /// the engine validates a replacement (it has no candidate to read then).
    fn maybe_load(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let (key, read): (Option<String>, DiffRead) = match &self.target {
            ReviewTarget::Candidate => {
                let ready = self.facts.phase == Some(TaskPhase::AwaitingReview)
                    && self.facts.candidate.is_some();
                let stale = self.key != self.facts.candidate || self.reload;
                if !ready || !stale {
                    return;
                }
                (
                    self.facts.candidate.clone(),
                    Box::new(move || backend.candidate_diffs()),
                )
            }
            ReviewTarget::Revision(revision) => {
                if self.loading || self.key.is_some() {
                    return;
                }
                let revision = revision.clone();
                (
                    Some(revision.clone()),
                    Box::new(move || backend.revision_diffs(&revision)),
                )
            }
        };
        self.serial += 1;
        self.loading = true;
        self.reload = false;
        let serial = self.serial;
        let task = cx
            .background_executor()
            .spawn(async move { read().map(Read::from_diffs) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.on_loaded(serial, key, result, cx))
                .ok();
        })
        .detach();
    }

    /// A diff read finished. It is dropped when a newer read was started or when it belongs to
    /// a candidate that is no longer the current one.
    pub fn on_loaded(
        &mut self,
        serial: u64,
        key: Option<String>,
        result: DiffResult,
        cx: &mut Context<Self>,
    ) {
        if serial != self.serial {
            return;
        }
        self.loading = false;
        if matches!(self.target, ReviewTarget::Candidate) && key != self.facts.candidate {
            self.maybe_load(cx);
            return;
        }
        match result {
            Ok(Read { loaded, prepared }) => {
                self.key = key;
                let more = loaded.more;
                self.changes.update(cx, |changes, cx| {
                    changes.set_prepared(prepared, cx);
                    changes.set_more(more, cx);
                });
            }
            Err(error) => {
                // Keep the key so a failing read is not retried in a loop.
                self.key = key;
                self.status = Some(format!("Could not read the changes: {error}"));
            }
        }
        cx.notify();
    }

    fn on_changes_event(
        &mut self,
        _: Entity<Changes>,
        event: &ChangesEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChangesEvent::FileToggled { .. } => {}
            ChangesEvent::Close => cx.emit(ReviewHostEvent::Close),
            ChangesEvent::Reject(paths) => {
                // An empty path names no file; it never goes to the engine.
                let paths: Vec<String> = paths.iter().filter(|p| !p.is_empty()).cloned().collect();
                if paths.is_empty() || self.busy() {
                    return;
                }
                self.start(
                    Operation::Rejecting {
                        saw_validating: false,
                        key: self.facts.candidate.clone(),
                        newest_row: self.facts.newest_row,
                    },
                    "Validating the remaining files…".into(),
                    cx,
                    move |backend| backend.reject_files(&paths),
                );
            }
            ChangesEvent::Discard => {
                if self.busy() {
                    return;
                }
                self.start(
                    Operation::Discarding,
                    "Discarding the candidate…".into(),
                    cx,
                    |backend| backend.discard(),
                );
            }
            ChangesEvent::RevertFile(path) => {
                let Some(revision) = self.revision().map(str::to_owned) else {
                    return;
                };
                if path.is_empty() || self.busy() {
                    return;
                }
                let path = path.clone();
                self.start(
                    Operation::Reverting {
                        path: path.clone(),
                        newest_row: self.facts.newest_row,
                    },
                    format!("Reverting {path}…"),
                    cx,
                    move |backend| backend.revert_path(&revision, &path),
                );
            }
        }
    }

    /// Records the operation, then calls the engine off the UI thread. The answer comes through
    /// the snapshot; only a refusal of the call itself ends the operation here.
    fn start(
        &mut self,
        op: Operation,
        status: String,
        cx: &mut Context<Self>,
        call: impl FnOnce(Arc<dyn ReviewBackend>) -> Result<(), String> + Send + 'static,
    ) {
        self.op = Some(op);
        self.status = Some(status);
        self.sync_changes(cx);
        let backend = self.backend.clone();
        let task = cx.background_executor().spawn(async move { call(backend) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                this.update(cx, |this, cx| {
                    this.op = None;
                    this.status = Some(error);
                    this.sync_changes(cx);
                })
                .ok();
            }
        })
        .detach();
    }
}

impl Render for ReviewHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        let colors = Colors::new(&theme);
        let status = self.status.clone().map(|text| {
            text_style(div(), TextStyle::Label)
                .debug_selector(|| "review-status".into())
                .w_full()
                .flex_none()
                .px(px(METRICS.space_3))
                .py(px(METRICS.space_2))
                .bg(colors.header)
                .border_b_1()
                .border_color(colors.line)
                .text_color(colors.text)
                .child(SharedString::from(text))
        });
        div()
            .id("review-host")
            .size_full()
            .flex()
            .flex_col()
            .children(status)
            .child(div().flex_1().min_h_0().child(self.changes.clone()))
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
