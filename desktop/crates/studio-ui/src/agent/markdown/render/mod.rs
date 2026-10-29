//! Rendering of agent and thought rows as markdown.
//!
//! [`MarkdownRow`] is the element a transcript row returns. Its state (an incremental parser,
//! one prepared value per top-level block, the veil, the selection and a pending link
//! confirmation) lives in a keyed gpui state under the row's id, so it survives the list
//! re-rendering the row on every frame and is dropped when the row stops rendering.
//!
//! A streamed row is fed to the parser as the text it holds now; the parser keeps the blocks
//! before the tail untouched, and a block whose `Arc` is the one prepared last time reuses its
//! prepared value, so an appended chunk only re-prepares the tail.

mod blocks;
mod code;
mod table;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{App, Context, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px};
use studio_engine::agent::{Row, RowKind};
use studio_markdown::{IncrementalParser, TopBlock};
use studio_theme::{METRICS, TextStyle, Theme, reduced_motion, text_style};

use super::inline_code_links::{Probes, Seen, kind_on_disk, link_block};
use super::link_ui::{Activation, Confirm, LinkSink, accepted, activate, confirm_overlay, os_sink};
use super::selection::Selection;
use super::style::MdStyle;
use super::text_element::{LinkActivate, SelectRoot, SharedSelection};
use super::veil::{RowVeil, veil_wanted};
use blocks::{BuildCx, Prepared, build, prepare, tagged};

/// Whether a row is the agent's answer or its reasoning (set in the quieter text colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Voice {
    Answer,
    Reasoning,
}

/// What a test can see of the row's cache.
#[cfg(test)]
#[derive(Debug, Default, Clone)]
pub struct Probe {
    /// Top-level blocks prepared since the row appeared.
    pub prepared: usize,
    /// Address of the cached prepared value of every top-level block, in order.
    pub cached: Vec<usize>,
    /// Whether the last build asked for another frame.
    pub fading: bool,
    /// The URL waiting for the reader's answer.
    pub confirm: Option<String>,
    /// Destinations of every link in the prepared blocks, in order.
    pub links: Vec<String>,
}

/// A top-level block's prepared value, with what its code-span file links were prepared from.
struct Cached {
    block: Arc<TopBlock>,
    prepared: Rc<Prepared>,
    /// Every project path the block's code spans asked about, with the answer it got. The
    /// prepared value is stale once an answer differs.
    seen: Seen,
}

struct RowState {
    parser: IncrementalParser,
    cache: Vec<Cached>,
    cache_key: Option<(Theme, Option<PathBuf>, Voice)>,
    probes: Probes,
    /// The project the probe answers belong to.
    probe_root: Option<PathBuf>,
    prepared: usize,
    veil: RowVeil,
    sel: SharedSelection,
    confirm: Option<Confirm>,
}

/// What a build needs to know about the row besides its text.
#[derive(Clone, Copy)]
struct Input {
    streaming: bool,
    voice: Voice,
    files: bool,
    row_id: u64,
}

struct Built {
    children: Vec<gpui::AnyElement>,
    fading: bool,
    /// Project paths whose code spans wait for a probe nobody has started.
    to_probe: Vec<String>,
}

impl RowState {
    fn new() -> Self {
        Self {
            parser: IncrementalParser::new(),
            cache: Vec::new(),
            cache_key: None,
            probes: Probes::default(),
            probe_root: None,
            prepared: 0,
            veil: RowVeil::seeded(),
            sel: Rc::new(RefCell::new(Selection::default())),
            confirm: None,
        }
    }

    /// Feeds the row's current text to the parser and makes the elements of this frame.
    fn build(
        &mut self,
        text: &str,
        input: &Input,
        root: Option<&Path>,
        style: &Rc<MdStyle>,
        window: &Window,
    ) -> Built {
        let Input {
            streaming,
            voice,
            files,
            row_id,
        } = *input;
        self.parser.set_text(text);
        let key = (style.theme, root.map(Path::to_path_buf), voice);
        if self.cache_key.as_ref() != Some(&key) {
            self.cache.clear();
            self.cache_key = Some(key);
        }
        if self.probe_root.as_deref() != root {
            self.probes.clear();
            self.probe_root = root.map(Path::to_path_buf);
        }
        let tree = self.parser.display_tree();
        let canonical = &self.parser.tree().blocks;
        let mut next = Vec::with_capacity(tree.blocks.len());
        let mut unanswered = Vec::new();
        for (i, top) in tree.blocks.iter().enumerate() {
            let probes = &self.probes;
            let fresh = |cached: &&Cached| {
                Arc::ptr_eq(&cached.block, top)
                    && cached
                        .seen
                        .iter()
                        .all(|(k, answer)| probes.get(k) == *answer)
            };
            let hit = self
                .cache
                .get(i)
                .filter(fresh)
                .or_else(|| self.cache.iter().find(fresh));
            if let Some(hit) = hit {
                next.push(Cached {
                    block: top.clone(),
                    prepared: hit.prepared.clone(),
                    seen: hit.seen.clone(),
                });
                continue;
            }
            // Only a block the canonical parse has too is complete: a block the mend changed
            // may end in a code span that is still being written.
            let settled = canonical.get(i).is_some_and(|c| Arc::ptr_eq(c, top));
            let linked = (files && settled).then(|| link_block(&top.block, &|k| probes.get(k)));
            let (block, seen) = match linked {
                Some(linked) => {
                    unanswered.extend(linked.unknown().map(str::to_owned));
                    (linked.block, linked.seen)
                }
                None => (None, Vec::new()),
            };
            self.prepared += 1;
            next.push(Cached {
                block: top.clone(),
                prepared: Rc::new(prepare(block.as_ref().unwrap_or(&top.block), style, files)),
                seen,
            });
        }
        self.cache = next;
        let to_probe = self.probes.claim(unanswered.iter().map(String::as_str));

        let wanted = veil_wanted(reduced_motion(), streaming, self.veil.is_fading());
        let mut cx = BuildCx::new(
            style.clone(),
            self.sel.clone(),
            window,
            row_id,
            wanted.then_some(&mut self.veil),
            Instant::now(),
        );
        let children = self
            .cache
            .iter()
            .enumerate()
            .map(|(i, cached)| tagged(&cached.prepared, i, build(&cached.prepared, &mut cx)))
            .collect();
        let texts = cx.finish();
        self.veil.retain_elements(texts.len());
        self.sel.borrow_mut().set_texts(texts);
        if wanted {
            self.veil.finish_seeding();
        }
        Built {
            children,
            fading: wanted && self.veil.is_fading(),
            to_probe,
        }
    }

    /// Finds what `keys` are on disk under `root` on a background task and re-renders the row
    /// when the answers are in; until then their code spans stay plain.
    fn probe(&mut self, root: PathBuf, keys: Vec<String>, cx: &mut Context<Self>) {
        let generation = self.probes.generation();
        cx.spawn(async move |this, cx| {
            let answers = cx
                .background_executor()
                .spawn(async move {
                    keys.into_iter()
                        .map(|key| {
                            let kind = kind_on_disk(&key, &root);
                            (key, kind)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if state.probes.accept(generation, answers) {
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// The markdown body of an agent or thought row.
#[derive(IntoElement)]
pub struct MarkdownRow {
    row: Arc<Row>,
    voice: Voice,
    project_root: Option<PathBuf>,
    sink: LinkSink,
    #[cfg(test)]
    probe: Option<Rc<RefCell<Probe>>>,
}

impl MarkdownRow {
    pub fn new(row: &Arc<Row>, voice: Voice) -> Self {
        Self {
            row: row.clone(),
            voice,
            project_root: None,
            sink: os_sink(),
            #[cfg(test)]
            probe: None,
        }
    }

    /// The project the row's file links point into. Without one, links to project files are
    /// plain text.
    pub fn project_root(mut self, root: Option<PathBuf>) -> Self {
        self.project_root = root;
        self
    }

    /// Where the effect of a link click goes; the OS by default.
    pub fn link_sink(mut self, sink: LinkSink) -> Self {
        self.sink = sink;
        self
    }

    #[cfg(test)]
    pub fn probe(mut self, probe: Rc<RefCell<Probe>>) -> Self {
        self.probe = Some(probe);
        self
    }
}

impl RenderOnce for MarkdownRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        let (text, streaming) = match &self.row.kind {
            RowKind::Agent {
                text, streaming, ..
            }
            | RowKind::Thought {
                text, streaming, ..
            } => (text.as_str(), *streaming),
            _ => ("", false),
        };
        let id = self.row.id.0;
        let state = window.use_keyed_state(("agent-md", id), cx, |_, _| RowState::new());
        let mut style = MdStyle::new(&theme);
        if self.voice == Voice::Reasoning {
            style.text = style.muted;
        }
        let style = Rc::new(style);
        let files = self.project_root.is_some();
        let (built, sel, confirm) = state.update(cx, |state, cx| {
            let input = Input {
                streaming,
                voice: self.voice,
                files,
                row_id: id,
            };
            let mut built = state.build(text, &input, self.project_root.as_deref(), &style, window);
            let to_probe = std::mem::take(&mut built.to_probe);
            if let (Some(root), false) = (&self.project_root, to_probe.is_empty()) {
                state.probe(root.clone(), to_probe, cx);
            }
            (built, state.sel.clone(), state.confirm.clone())
        });
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            let state = state.read(cx);
            *probe.borrow_mut() = Probe {
                prepared: state.prepared,
                cached: state
                    .cache
                    .iter()
                    .map(|c| Rc::as_ptr(&c.prepared) as usize)
                    .collect(),
                links: state
                    .cache
                    .iter()
                    .flat_map(|c| c.prepared.link_dests())
                    .collect(),
                fading: built.fading,
                confirm: state.confirm.as_ref().map(|c| c.url.clone()),
            };
        }
        if built.fading {
            window.request_animation_frame();
        }

        let activate_link: LinkActivate = {
            let (state, root, sink) = (state.clone(), self.project_root.clone(), self.sink.clone());
            Rc::new(
                move |dest, position, _window, cx| match activate(dest, root.as_deref()) {
                    Activation::Perform(effect) => sink(&effect, cx),
                    Activation::Confirm(url) => state.update(cx, |state, cx| {
                        state.confirm = Some(Confirm { url, at: position });
                        cx.notify();
                    }),
                    Activation::Ignored => {}
                },
            )
        };

        let mut content = text_style(
            div(),
            if self.voice == Voice::Reasoning {
                TextStyle::Small
            } else {
                TextStyle::Body
            },
        )
        .text_color(style.text)
        .flex()
        .flex_col()
        .gap(px(METRICS.space_2))
        .children(built.children);
        if let Some(confirm) = confirm {
            let (on_open, on_cancel) = (state.clone(), state.clone());
            let (sink, url) = (self.sink.clone(), confirm.url.clone());
            content = content.child(confirm_overlay(
                &theme,
                id,
                &confirm,
                window.viewport_size(),
                move |_, cx| {
                    // A second click delivered before the overlay goes finds nothing to open.
                    let pending = on_open.update(cx, |state, cx| {
                        cx.notify();
                        state.confirm.take().is_some()
                    });
                    if let Some(effect) = pending.then(|| accepted(&url)).flatten() {
                        sink(&effect, cx);
                    }
                },
                move |_, cx| {
                    on_cancel.update(cx, |state, cx| {
                        state.confirm = None;
                        cx.notify();
                    });
                },
            ));
        }
        SelectRoot::new(content.into_any_element(), sel, activate_link)
    }
}
