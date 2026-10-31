//! Present mode: the project plays as a slide deck. Each slide plays with its audio from
//! one stop to the next and holds there until the presenter moves on.
//!
//! The player section renders a presentation stage in place of the player card, and the
//! rest of the window is hidden. The stage has its own focus handle and the `Present` key
//! context (never `Player`), so its arrows, space, Home, End and Escape reach the deck.
//! The window enters full screen; leaving restores the editing layout and the full screen
//! state from before, and focuses the preview. `toggle_fullscreen` is asynchronous on
//! Windows, so the shell keeps its own record of what it asked for ([`Fullscreen`]).
//!
//! While presenting, nothing starts playback on its own: a new build waits until the
//! presentation ends ("Update ready"), and a timing or slides change rebuilds the deck
//! and holds at the stop nearest to the shown frame.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ObjectFit,
    ParentElement, StatefulInteractiveElement, Styled, StyledImage, Task, Window, canvas, div, px,
};
use studio_engine::ReadyPreview;
use studio_engine::slides::{Deck, DeckStep};
use studio_theme::{METRICS, TextStyle, text_style};

use super::StudioShell;
use super::keymap::PRESENT;
use crate::kit::Progress;

/// How long the progress strip shows after a change.
const STRIP_SHOWN: Duration = Duration::from_secs(2);
/// How long the pointer stays still before it is hidden.
const CURSOR_IDLE: Duration = Duration::from_secs(2);
/// A printable key nothing binds in the `Present` context. gpui hides the pointer until it
/// moves only on typing, so the idle timer types this key on the stage.
const HIDE_CURSOR_KEY: &str = "x";

/// The presentation under way.
pub(super) struct PresentState {
    pub(super) deck: Deck,
    pub(super) focus: FocusHandle,
    /// Whether the window was full screen before; `None` until the first render applied
    /// the window effects (focus, full screen).
    was_fullscreen: Option<bool>,
    /// The presentation wants full screen; false once the system left it (a display
    /// unplugged, Win+Down), and then leaving does not toggle it back.
    fullscreen: bool,
    /// The window was seen full screen during the presentation.
    seen_fullscreen: bool,
    /// The newest build that arrived while presenting, installed when it ends.
    held: Option<Arc<ReadyPreview>>,
    /// The progress strip shows until then.
    strip_until: Instant,
    /// Digits typed for "number + Enter".
    typed: String,
    strip_timer: Option<Task<()>>,
    cursor_timer: Option<Task<()>>,
}

impl PresentState {
    pub(super) fn new(deck: Deck, focus: FocusHandle) -> Self {
        Self {
            deck,
            focus,
            was_fullscreen: None,
            fullscreen: true,
            seen_fullscreen: false,
            held: None,
            strip_until: Instant::now() + STRIP_SHOWN,
            typed: String::new(),
            strip_timer: None,
            cursor_timer: None,
        }
    }

    /// Keeps `ready` for the end of the presentation.
    pub(super) fn hold_build(&mut self, ready: Arc<ReadyPreview>) {
        self.held = Some(ready);
        self.strip_until = Instant::now() + STRIP_SHOWN;
    }
}

/// The full screen state the shell asked for. A toggle takes effect later on some
/// systems, so a request waits until the window reports it, and a later wish waits
/// behind it instead of toggling twice.
#[derive(Debug, Default)]
pub(super) struct Fullscreen {
    in_flight: Option<bool>,
    queued: Option<bool>,
}

impl Fullscreen {
    /// Wants the window full screen or not; true when the caller must toggle now.
    pub(super) fn want(&mut self, is_fullscreen: bool, want: bool) -> bool {
        if self.in_flight.is_some() {
            self.queued = Some(want);
            return false;
        }
        self.queued = None;
        let toggle = is_fullscreen != want;
        if toggle {
            self.in_flight = Some(want);
        }
        toggle
    }

    /// The window's state at a render; true when a queued wish must toggle now.
    pub(super) fn observe(&mut self, is_fullscreen: bool) -> bool {
        if self.in_flight != Some(is_fullscreen) {
            return false;
        }
        self.in_flight = None;
        match self.queued.take() {
            Some(want) => self.want(is_fullscreen, want),
            None => false,
        }
    }

    /// No toggle is on its way.
    pub(super) fn settled(&self) -> bool {
        self.in_flight.is_none()
    }
}

/// What the next render restores after a presentation.
pub(super) struct AfterPresent {
    fullscreen: Option<bool>,
}

impl StudioShell {
    /// The deck of the installed video (the clock's length) and its resolved stops.
    fn current_deck(&self) -> Deck {
        let stops = self
            .presentation
            .slides
            .as_ref()
            .map_or(&[][..], |slides| &slides.stops[..]);
        Deck::new(stops, self.transport.total_frames())
    }

    /// Starts presenting, held at frame 0 or at the stop before video frame `from` (the
    /// end of the slide "Present from here" was asked on).
    pub(super) fn present(&mut self, from: Option<usize>, cx: &mut Context<Self>) {
        if self.presenting.is_some() || self.displayed.is_none() {
            return;
        }
        let deck = self.current_deck();
        let start = match from.map(|frame| deck.previous(frame)) {
            Some(DeckStep::HoldAt(frame)) => frame,
            _ => 0,
        };
        self.resume_install = false;
        self.overflow_open = false;
        if self.transport.playing() {
            self.pause_playback(cx);
        }
        let mut state = PresentState::new(deck, cx.focus_handle());
        // A build being prepared waits for the end, like one that arrives later.
        if let Some(pending) = self.pending_ready.take() {
            self.audio.stop(self.transport.epoch());
            self.output = None;
            self.audio_ready_epoch = None;
            state.held = Some(pending.ready);
        }
        self.presenting = Some(state);
        self.seek_preview(start, cx);
        self.show_strip(cx);
        cx.notify();
    }

    /// Leaves the presentation: the editing layout and the window state come back on the
    /// next render, which focuses the preview, and a build that waited is installed.
    pub(super) fn exit_present(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.presenting.take() else {
            return;
        };
        self.resume_install = false;
        if self.transport.playing() {
            self.pause_playback(cx);
        }
        self.transport.set_play_until(None);
        self.after_present = Some(AfterPresent {
            // The system left full screen on its own: leave the window as it is.
            fullscreen: state.was_fullscreen.filter(|_| state.fullscreen),
        });
        if let Some(ready) = state.held
            && let Some(project) = self.presentation.state.clone()
        {
            self.offer_ready(ready, &project, cx);
        }
        cx.notify();
    }

    pub(super) fn present_next(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.presenting else {
            return;
        };
        let playing = self.transport.playing() || self.resume_install;
        let step = state.deck.next(self.transport.position(), playing);
        self.present_step(step, cx);
    }

    pub(super) fn present_previous(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.presenting else {
            return;
        };
        let step = state.deck.previous(self.transport.position());
        self.present_step(step, cx);
    }

    /// Holds at the start of slide `slide` (1-based, clamped to the deck).
    pub(super) fn present_slide(&mut self, slide: usize, cx: &mut Context<Self>) {
        let Some(state) = &self.presenting else {
            return;
        };
        let slide = slide.clamp(1, state.deck.slides().max(1));
        let step = state
            .deck
            .start_of(slide)
            .map_or(DeckStep::HoldAt(0), DeckStep::HoldAt);
        self.present_step(step, cx);
    }

    fn present_step(&mut self, step: DeckStep, cx: &mut Context<Self>) {
        match step {
            DeckStep::PlayTo(frame) => {
                if self.transport.playing() || self.resume_install {
                    self.resume_install = false;
                    self.pause_playback(cx);
                }
                // The clock stops at the stop and the audio goes silent there.
                self.transport.set_play_until(Some(frame));
                self.toggle_playback(cx);
            }
            DeckStep::HoldAt(frame) => self.hold_presentation_at(frame, cx),
            DeckStep::Stay => {}
        }
        self.show_strip(cx);
    }

    fn hold_presentation_at(&mut self, frame: usize, cx: &mut Context<Self>) {
        self.resume_install = false;
        if self.transport.playing() {
            self.pause_playback(cx);
        }
        self.seek_preview(frame, cx);
    }

    /// Stops playback and holds at the deck's stop nearest to the shown frame.
    pub(super) fn hold_at_nearest_stop(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.presenting else {
            return;
        };
        let frame = state.deck.nearest(self.transport.position());
        self.hold_presentation_at(frame, cx);
    }

    /// Rebuilds the deck when the resolved stops or the video's length changed, and holds
    /// at the nearest stop. Stops resolved against an older layout wait for the new one.
    pub(super) fn sync_deck(&mut self, cx: &mut Context<Self>) {
        if self.presenting.is_none()
            || self
                .presentation
                .slides
                .as_ref()
                .is_some_and(|slides| slides.unavailable.is_some())
        {
            return;
        }
        let deck = self.current_deck();
        let Some(state) = &mut self.presenting else {
            return;
        };
        if state.deck != deck {
            state.deck = deck;
            self.hold_at_nearest_stop(cx);
            self.show_strip(cx);
        }
    }

    /// Shows the progress strip for two seconds.
    fn show_strip(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.presenting else {
            return;
        };
        state.strip_until = Instant::now() + STRIP_SHOWN;
        state.strip_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STRIP_SHOWN).await;
            let _ = this.update(cx, |shell, cx| shell.notify_player(cx));
        }));
        self.notify_player(cx);
    }

    /// Hides the pointer once it stayed still for two seconds; a move shows it again.
    fn arm_cursor_hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &mut self.presenting else {
            return;
        };
        state.cursor_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(CURSOR_IDLE).await;
            let presenting = this
                .read_with(cx, |shell, _| shell.presenting.is_some())
                .unwrap_or(false);
            // Outside the shell's update: the keystroke draws the window, which renders
            // the shell.
            if presenting && let Ok(keystroke) = gpui::Keystroke::parse(HIDE_CURSOR_KEY) {
                let _ = cx.update(|window, cx| window.dispatch_keystroke(keystroke, cx));
            }
        }));
    }

    /// Number + Enter goes to a slide; Backspace takes a digit back.
    fn present_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(state) = &mut self.presenting else {
            return;
        };
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.number_of_modifiers() > 0 {
            return;
        }
        if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() {
            if state.typed.len() < 4 {
                state.typed.push_str(key);
            }
        } else if key == "backspace" && !state.typed.is_empty() {
            state.typed.pop();
        } else if key == "enter" && !state.typed.is_empty() {
            let slide = std::mem::take(&mut state.typed).parse().unwrap_or(1);
            cx.stop_propagation();
            self.present_slide(slide, cx);
            return;
        } else {
            return;
        }
        cx.stop_propagation();
        self.show_strip(cx);
    }

    /// Applies the window effects of entering and leaving: focus, full screen, and
    /// whether the system left full screen. Runs at the top of every render.
    pub(super) fn sync_present_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_fullscreen = window.is_fullscreen();
        if self.fullscreen.observe(is_fullscreen) {
            window.toggle_fullscreen();
        }
        if let Some(after) = self.after_present.take() {
            if let Some(want) = after.fullscreen
                && self.fullscreen.want(is_fullscreen, want)
            {
                window.toggle_fullscreen();
            }
            // The preview, whose keys step frames again. What had the focus before may
            // not be drawn any more (a button of a cached drawer), which would leave the
            // keys without the workspace's context.
            window.focus(&self.preview_focus, cx);
        }
        let Some(state) = &mut self.presenting else {
            return;
        };
        if state.was_fullscreen.is_none() {
            state.was_fullscreen = Some(is_fullscreen);
            state.seen_fullscreen = is_fullscreen;
            window.focus(&state.focus, cx);
            if self.fullscreen.want(is_fullscreen, true) {
                window.toggle_fullscreen();
            }
            self.arm_cursor_hide(window, cx);
        } else if is_fullscreen {
            state.seen_fullscreen = true;
        } else if state.fullscreen && state.seen_fullscreen && self.fullscreen.settled() {
            state.fullscreen = false;
        }
    }

    /// The window's content while presenting: the player section alone, on black. It
    /// replaces the shell's root, so it sets the UI font itself.
    pub(super) fn render_presenting_root(
        &self,
        player: gpui::AnyElement,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        Self::on_actions(div(), cx)
            .font_family(studio_theme::UI_FONT)
            .size_full()
            .key_context(super::keymap::WORKSPACE)
            .track_focus(&self.focus)
            .bg(self.theme.colors.present_bg)
            .flex()
            .child(player)
            .into_any_element()
    }

    /// The presentation stage: the frame letterboxed on black, the progress strip and the
    /// keys of the `Present` context.
    pub(super) fn render_present_stage(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(state) = &self.presenting else {
            return div().into_any_element();
        };
        let entity = cx.entity();
        let mut stage = div()
            .id("present-stage")
            .key_context(PRESENT)
            .track_focus(&state.focus)
            .role(gpui::Role::Group)
            .aria_label("Presentation")
            .relative()
            .flex_1()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(self.theme.colors.present_bg)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|shell, _, _, cx| shell.present_next(cx)),
            )
            .on_mouse_move(cx.listener(|shell, _, window, cx| shell.arm_cursor_hide(window, cx)))
            .on_key_down(
                cx.listener(|shell, event: &KeyDownEvent, _, cx| shell.present_key(event, cx)),
            )
            // Asks for frames at the stage's size, as the player's stage does.
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let dpi = window.scale_factor();
                        let extent = (
                            (f32::from(bounds.size.width) * dpi).round().max(1.) as u32,
                            (f32::from(bounds.size.height) * dpi).round().max(1.) as u32,
                        );
                        entity.update(cx, |shell, cx| {
                            shell.preview_extent = extent;
                            if shell.displayed.as_ref().is_some_and(|d| {
                                crate::frame_image::preview_scale(
                                    d.timeline.width,
                                    d.timeline.height,
                                    extent,
                                ) != shell.preview_state.scale()
                            }) {
                                shell.request_frame(shell.transport.position(), cx);
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        if let Some(image) = self.images.current_image() {
            stage = stage.child(
                gpui::img(image)
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            );
        }
        let now = Instant::now();
        if now < state.strip_until || !state.typed.is_empty() {
            stage = stage.child(self.render_progress_strip(state));
        }
        stage.into_any_element()
    }

    /// A thin strip at the foot: slide N of M, the typed slide number, "Update ready".
    fn render_progress_strip(&self, state: &PresentState) -> gpui::AnyElement {
        let slides = state.deck.slides().max(1);
        let slide = state.deck.slide_at(self.transport.position());
        let mut label = format!("Slide {slide} of {slides}");
        if !state.typed.is_empty() {
            label.push_str(&format!(" · go to {}", state.typed));
        }
        if state.held.is_some() {
            label.push_str(" · Update ready (Esc to apply)");
        }
        let fraction = slide as f32 / slides as f32;
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .flex_col()
            .child(
                text_style(div(), TextStyle::Body)
                    .px(px(METRICS.space_4))
                    .py(px(METRICS.space_2))
                    .text_color(self.theme.colors.on_accent)
                    .child(label),
            )
            .child(Progress::linear(&self.theme, fraction).on_dark())
            // The scrim keeps the label readable on a bright frame.
            .bg(self.theme.colors.scrim)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_engine::PlaybackClock;
    use studio_engine::containment::OwnershipPolicy;
    use studio_engine::engine::backend::StudioEnvironment;
    use studio_engine::slides::{ResolvedStop, SlidesEditing, SlidesSnapshot, StopSource};

    fn stops(frames: &[usize]) -> Vec<ResolvedStop> {
        frames
            .iter()
            .map(|&video_frame| ResolvedStop {
                video_frame,
                title: None,
                notes: None,
                source: StopSource::Default,
            })
            .collect()
    }

    fn slides(frames: &[usize]) -> SlidesSnapshot {
        SlidesSnapshot {
            generation: frames.len() as u64,
            doc: None,
            stops: stops(frames),
            problems: vec![],
            unavailable: None,
            editing: SlidesEditing::Enabled,
            notice: None,
        }
    }

    /// A shell without a project, presenting a 90-frame video with stops at 30 and 60,
    /// held at frame 0 with the stage focused.
    fn presenting_shell<'a>(
        temp: &tempfile::TempDir,
        cx: &'a mut gpui::TestAppContext,
    ) -> (gpui::Entity<StudioShell>, &'a mut gpui::VisualTestContext) {
        cx.update(crate::shell::bind_keys);
        let environment = StudioEnvironment {
            app_data: Some(temp.path().join("app")),
            sdk_home: Some(temp.path().join("sdk")),
            builds: studio_engine::services::worker_project::shared_build_service(),
            ownership: OwnershipPolicy::Validated,
        };
        cx.executor().allow_parking();
        let (shell, cx) = cx.add_window_view(|window, cx| {
            StudioShell::with_environment(environment, "0.0.0", window, cx)
        });
        // The engine answers `Initialize` on its own thread; its presentation must not
        // replace the one set up below.
        for _ in 0..1000 {
            cx.run_until_parked();
            if !shell.read_with(cx, |shell, _| shell.busy) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !shell.read_with(cx, |shell, _| shell.busy),
            "the engine never answered"
        );
        shell.update(cx, |shell, cx| {
            shell.transport = PlaybackClock::install(30, 90, 0, 0.).unwrap();
            shell.presentation.slides = Some(slides(&[30, 60, 89]));
            let deck = shell.current_deck();
            shell.presenting = Some(PresentState::new(deck, cx.focus_handle()));
            cx.notify();
        });
        cx.run_until_parked();
        (shell, cx)
    }

    #[gpui::test]
    fn right_while_presenting_is_present_next_not_step_forward(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let (shell, cx) = presenting_shell(&temp, cx);
        let focused = shell.update_in(cx, |shell, window, _| {
            shell.presenting.as_ref().unwrap().focus.is_focused(window)
        });
        assert!(focused, "the stage takes the focus on entry");
        cx.simulate_keystrokes("right");
        shell.read_with(cx, |shell, _| {
            // StepForward would pause on frame 1; PresentNext plays slide 1 to its stop.
            assert!(shell.transport.playing());
            assert_eq!(shell.transport.position(), 0);
            assert_eq!(shell.transport.play_until(), Some(30));
        });
        // Again while it plays: jump to the stop and hold.
        cx.simulate_keystrokes("right");
        shell.read_with(cx, |shell, _| {
            assert!(!shell.transport.playing());
            assert_eq!(shell.transport.position(), 30);
        });
        cx.simulate_keystrokes("left");
        shell.read_with(cx, |shell, _| assert_eq!(shell.transport.position(), 0));
        cx.simulate_keystrokes("end");
        shell.read_with(cx, |shell, _| assert_eq!(shell.transport.position(), 60));
        cx.simulate_keystrokes("2 enter");
        shell.read_with(cx, |shell, _| assert_eq!(shell.transport.position(), 30));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert!(shell.presenting.is_none());
            assert!(!shell.transport.playing());
        });
        let focused = shell.update_in(cx, |shell, window, _| {
            shell.preview_focus.is_focused(window)
        });
        assert!(focused, "the preview has the focus again");
    }

    #[gpui::test]
    fn a_retime_while_presenting_never_starts_playback(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let (shell, cx) = presenting_shell(&temp, cx);
        // Held at the first stop.
        cx.simulate_keystrokes("right right");
        shell.read_with(cx, |shell, _| {
            assert!(!shell.transport.playing());
            assert_eq!(shell.transport.position(), 30);
        });
        // A trim shortens the video; the stops move as the engine resolves them again.
        shell.update(cx, |shell, cx| {
            let playing = shell.transport.playing();
            shell.transport.reinstall(30, 80, 30, 1.).unwrap();
            shell.after_retime(playing, cx);
            shell.presentation.slides = Some(slides(&[25, 50, 79]));
            cx.notify();
        });
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert!(
                !shell.transport.playing(),
                "a retime while held started playback"
            );
            assert_eq!(
                shell.transport.position(),
                25,
                "holds at the nearest new stop"
            );
            assert_eq!(
                shell.presenting.as_ref().unwrap().deck.holds(),
                &[0, 25, 50, 79]
            );
        });
        // A retime while a slide plays stops it too.
        cx.simulate_keystrokes("right");
        shell.update(cx, |shell, cx| {
            let playing = shell.transport.playing();
            assert!(playing);
            shell.transport.reinstall(30, 80, 25, 2.).unwrap();
            shell.after_retime(playing, cx);
        });
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert!(!shell.transport.playing());
            assert_eq!(shell.transport.position(), 25);
        });
    }

    #[test]
    fn a_toggle_waits_for_the_window_and_a_later_wish_waits_behind_it() {
        let mut fullscreen = Fullscreen::default();
        // Entering: toggle once, the window is still windowed at the next renders.
        assert!(fullscreen.want(false, true));
        assert!(!fullscreen.observe(false));
        // Leaving before the window got there: no second toggle yet.
        assert!(!fullscreen.want(false, false));
        assert!(!fullscreen.settled());
        // The window arrives in full screen: now leave it.
        assert!(fullscreen.observe(true));
        assert!(!fullscreen.observe(true));
        assert!(!fullscreen.observe(false));
        assert!(fullscreen.settled());
        // Nothing to do when the window already is as wanted.
        assert!(!fullscreen.want(false, false));
        assert!(fullscreen.settled());
    }
}
