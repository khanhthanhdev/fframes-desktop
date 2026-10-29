//! `Collapsible` -- Kobalte's, with the height animation the Solid app defines
//! in `packages/ui-solid/src/main.css`:
//!
//! ```css
//! @keyframes collapsible-down { from { height: 0 } to { height: var(--kb-collapsible-content-height) } }
//! ```
//!
//! `--kb-collapsible-content-height` is set at runtime by Kobalte, from the
//! content's *measured* natural height. That measurement is the load-bearing
//! half: animating a canned duration against a guessed height reflows the
//! surrounding page wrongly for one frame, every frame.
//!
//! gpui's equivalent of "measure the content" is a [`gpui::canvas`] laid over
//! it, whose prepaint bounds land in a cell -- the same trick the sliders use
//! for their tracks. The content is rendered at its natural size inside a
//! clipped container whose height is animated; until the first measurement
//! lands the container is unconstrained, so the very first expand is a single
//! instant frame rather than a collapse-to-zero flash.
//!
//! The `filter: blur(5px) -> blur(0)` cross-fade in the same keyframe is not
//! reproduced: this gpui rev has no per-element blur (the same gap the
//! teleprompter's vignette and the recording overlay's backdrop have).

use std::{cell::Cell, rc::Rc, time::Instant};

use gpui::{
    AnyElement, App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, Role, StatefulInteractiveElement, Styled, Transformation, Window, canvas, div,
    prelude::FluentBuilder, px, radians, svg,
};

use studio_theme::Theme;

use super::interact::focusable;

/// `0.2s ease-out`, from the two keyframes.
pub const COLLAPSIBLE_DURATION_SECS: f32 = 0.2;

/// The measured height and the in-flight animation for one collapsible.
///
/// Windows own one per collapsible and drive `tick` from their own frame loop
/// (or simply flip `open` and let the instant path run -- see
/// [`CollapsibleState::height_for`]).
#[derive(Debug, Clone)]
pub struct CollapsibleState {
    /// A `Cell` for the same reason `transition` is one: the windows that own
    /// these render from `&self`, and the editor's sidebar toggles several of
    /// them (the colour-grade reveal, the 3D panel's sections) from inside a
    /// render-built click handler that only has the state by shared reference.
    open: Cell<bool>,
    /// The content's natural height, written from prepaint.
    measured: Rc<Cell<Option<Pixels>>>,
    /// When the current open/close transition started, and the height it
    /// started from. `None` once it has settled.
    ///
    /// A `Cell` so `height_for` can retire a finished transition from `&self`:
    /// the windows that own these call it from a `&self` render helper.
    transition: Cell<Option<(Instant, f32)>>,
}

impl Default for CollapsibleState {
    fn default() -> Self {
        Self::new(false)
    }
}

impl CollapsibleState {
    pub fn new(open: bool) -> Self {
        Self {
            open: Cell::new(open),
            measured: Rc::new(Cell::new(None)),
            transition: Cell::new(None),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// The cell the content's `canvas` writes its prepaint bounds into.
    pub fn measure_cell(&self) -> Rc<Cell<Option<Pixels>>> {
        self.measured.clone()
    }

    /// The natural height, once one frame has been laid out.
    pub fn measured_height(&self) -> Option<Pixels> {
        self.measured.get()
    }

    /// Flip the panel, starting a transition from wherever it is now.
    pub fn toggle(&self) {
        self.set_open(!self.open.get());
    }

    pub fn set_open(&self, open: bool) {
        if self.open.get() == open {
            return;
        }
        let from = self.current_height();
        self.open.set(open);
        // With no measurement yet there is nothing to animate towards, so the
        // first expand of a never-rendered panel is instant.
        self.transition
            .set(self.measured.get().map(|_| (Instant::now(), from)));
    }

    /// Whether a transition is still in flight, i.e. whether the panel has to
    /// stay mounted even though it is on its way out.
    pub fn is_animating(&self) -> bool {
        self.transition.get().is_some()
    }

    /// The height to give the container this frame, and whether another frame
    /// is needed. `None` means "unconstrained": either fully open with no
    /// measurement yet, or fully open with the animation finished, where
    /// pinning a height would stop the panel reflowing if its content changes.
    pub fn height_for(&self, now: Instant) -> (Option<Pixels>, bool) {
        let Some(target) = self.measured.get().map(f32::from) else {
            return (if self.open.get() { None } else { Some(px(0.)) }, false);
        };
        let target = if self.open.get() { target } else { 0. };

        let Some((started, from)) = self.transition.get() else {
            return (if self.open.get() { None } else { Some(px(0.)) }, false);
        };

        let elapsed = now.duration_since(started).as_secs_f32();
        let t = (elapsed / COLLAPSIBLE_DURATION_SECS).clamp(0., 1.);
        // `ease-out`, the same cubic the window resize uses.
        let eased = 1. - (1. - t).powi(3);
        let height = from + (target - from) * eased;

        if t >= 1. {
            self.transition.set(None);
            return (if self.open.get() { None } else { Some(px(0.)) }, false);
        }
        (Some(px(height)), true)
    }

    fn current_height(&self) -> f32 {
        match (self.transition.get(), self.measured.get()) {
            (Some((started, from)), Some(measured)) => {
                let target = if self.open.get() {
                    f32::from(measured)
                } else {
                    0.
                };
                let t = (started.elapsed().as_secs_f32() / COLLAPSIBLE_DURATION_SECS).clamp(0., 1.);
                let eased = 1. - (1. - t).powi(3);
                from + (target - from) * eased
            }
            (None, Some(measured)) if self.open.get() => f32::from(measured),
            (None, Some(_)) => 0.,
            _ => 0.,
        }
    }
}

/// The requested open state arrives by reference, so `cx.listener(..)` builds
/// one directly.
type ToggleHandler = Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

/// The focusable header that opens and closes a section: a chevron that turns
/// down when open, then the children. Enter, Space and a click all flip it.
#[derive(IntoElement)]
pub struct Disclosure {
    id: ElementId,
    open: bool,
    children: Vec<AnyElement>,
    on_toggle: Option<ToggleHandler>,
}

impl Disclosure {
    pub fn new(id: impl Into<ElementId>, open: bool) -> Self {
        Self {
            id: id.into(),
            open,
            children: Vec::new(),
            on_toggle: None,
        }
    }

    /// Called with the requested open state.
    pub fn on_toggle(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Disclosure {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Disclosure {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        let Disclosure {
            id,
            open,
            children,
            on_toggle,
        } = self;
        let turn = if open {
            std::f32::consts::FRAC_PI_2
        } else {
            0.
        };

        focusable(div().id(id), &theme, false)
            .role(Role::DisclosureTriangle)
            .aria_expanded(open)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .child(
                svg()
                    .path("icons/chevron-right.svg")
                    .size(px(12.))
                    .flex_shrink_0()
                    .text_color(Hsla::from(theme.colors.text_3))
                    .with_transformation(Transformation::rotate(radians(turn))),
            )
            .children(children)
            .when_some(on_toggle, |this, handler| {
                this.on_click(move |_, window, cx| handler(&!open, window, cx))
            })
    }
}

/// A section its owner opens and closes: a [`Disclosure`] header over content
/// that animates its height. The owner keeps `open` and flips it from
/// `on_toggle`; the measurement and the animation live in keyed state.
#[derive(IntoElement)]
pub struct CollapsibleSection {
    id: ElementId,
    open: bool,
    header: Vec<AnyElement>,
    content: Option<AnyElement>,
    on_toggle: Option<ToggleHandler>,
}

impl CollapsibleSection {
    pub fn new(id: impl Into<ElementId>, open: bool) -> Self {
        Self {
            id: id.into(),
            open,
            header: Vec::new(),
            content: None,
            on_toggle: None,
        }
    }

    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header.push(header.into_any_element());
        self
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }

    /// Called with the requested open state.
    pub fn on_toggle(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for CollapsibleSection {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let CollapsibleSection {
            id,
            open,
            header,
            content,
            on_toggle,
        } = self;
        // The entity, not a clone of its value: the open flag is a `Cell`.
        let state = window.use_keyed_state((id.clone(), "state".to_string()), cx, |_, _| {
            CollapsibleState::new(open)
        });
        let state = state.read(cx);
        state.set_open(open);
        let (height, animating) = state.height_for(Instant::now());
        if animating {
            window.request_animation_frame();
        }
        let measure = state.measure_cell();

        let mut disclosure = Disclosure::new((id.clone(), "header".to_string()), open);
        disclosure.extend(header);
        if let Some(handler) = on_toggle {
            disclosure = disclosure.on_toggle(handler);
        }
        div()
            .id(id)
            .flex()
            .flex_col()
            .child(disclosure)
            .children(content.map(|content| Collapsible::new(height, measure).content(content)))
    }
}

/// The clipped container. Give it the height [`CollapsibleState::height_for`]
/// returned and the content to measure.
#[derive(IntoElement)]
pub struct Collapsible {
    height: Option<Pixels>,
    measure: Rc<Cell<Option<Pixels>>>,
    content: Option<AnyElement>,
}

impl Collapsible {
    pub fn new(height: Option<Pixels>, measure: Rc<Cell<Option<Pixels>>>) -> Self {
        Self {
            height,
            measure,
            content: None,
        }
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }
}

impl RenderOnce for Collapsible {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Collapsible {
            height,
            measure,
            content,
        } = self;

        div()
            .overflow_hidden()
            .when_some(height, |this, height| this.h(height))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(
                        // The measurement. It is inside the natural-size stack
                        // rather than around it, so it reports the content's
                        // height and not the clipped container's.
                        canvas(
                            move |bounds, _window, _cx| measure.set(Some(bounds.size.height)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children(content),
            )
    }
}

#[cfg(test)]
mod section_in_a_window {
    use gpui::{Context, Render, TestAppContext, VisualTestContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    /// A controlled section over a 40px body, inside a box the test measures.
    struct Panel {
        open: bool,
        toggles: Log<bool>,
    }

    impl Render for Panel {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let toggles = self.toggles.clone();
            div().flex().flex_col().size_full().child(
                div().debug_selector(|| "panel".into()).child(
                    CollapsibleSection::new("sdk", self.open)
                        .header(div().child("SDK"))
                        .content(div().h(px(40.)).child("body"))
                        .on_toggle(cx.listener(move |this, open: &bool, _, cx| {
                            toggles.borrow_mut().push(*open);
                            this.open = *open;
                            cx.notify();
                        })),
                ),
            )
        }
    }

    fn panel(cx: &mut TestAppContext, open: bool) -> (Log<bool>, &mut VisualTestContext) {
        let toggles = log();
        let shared = toggles.clone();
        let (_, cx) = cx.add_window_view(|_, _| Panel {
            open,
            toggles: shared,
        });
        cx.run_until_parked();
        (toggles, cx)
    }

    fn height(cx: &mut VisualTestContext) -> f32 {
        f32::from(cx.debug_bounds("panel").expect("painted").size.height)
    }

    #[gpui::test]
    fn an_open_section_shows_its_content(cx: &mut TestAppContext) {
        let (_, cx) = panel(cx, true);
        assert!(height(cx) >= 40., "{}", height(cx));
    }

    #[gpui::test]
    fn a_closed_section_shows_only_its_header(cx: &mut TestAppContext) {
        let (_, cx) = panel(cx, false);
        assert!(height(cx) < 40., "{}", height(cx));
    }

    #[gpui::test]
    fn the_header_asks_the_owner_to_flip_it(cx: &mut TestAppContext) {
        let (toggles, cx) = panel(cx, false);
        tab(cx);
        press(cx, "enter");
        press(cx, "space");
        assert_eq!(*toggles.borrow(), [true, false]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn measured(state: &CollapsibleState, height: f32) {
        state.measure_cell().set(Some(px(height)));
    }

    #[test]
    fn a_closed_panel_is_zero_high() {
        let state = CollapsibleState::new(false);
        measured(&state, 120.);
        assert_eq!(state.height_for(Instant::now()), (Some(px(0.)), false));
    }

    /// Open and settled means *unconstrained*, not "pinned at the last
    /// measurement" -- otherwise content that grows after the animation ends
    /// would be clipped forever.
    #[test]
    fn a_settled_open_panel_takes_its_natural_height() {
        let state = CollapsibleState::new(true);
        measured(&state, 120.);
        assert_eq!(state.height_for(Instant::now()), (None, false));
    }

    #[test]
    fn the_first_expand_of_an_unmeasured_panel_is_instant() {
        let state = CollapsibleState::new(false);
        state.toggle();
        assert!(state.is_open());
        // No measurement yet, so no transition was started and nothing has to
        // be animated.
        assert_eq!(state.height_for(Instant::now()), (None, false));
    }

    #[test]
    fn expanding_animates_from_zero_to_the_measured_height() {
        let state = CollapsibleState::new(false);
        measured(&state, 200.);
        state.toggle();

        let start = Instant::now();
        let (height, more) = state.height_for(start);
        assert!(more, "an in-flight transition asks for another frame");
        let height = f32::from(height.expect("mid-transition height"));
        assert!((0. ..200.).contains(&height), "{height}");

        let (height, more) = state.height_for(start + Duration::from_millis(100));
        let height = f32::from(height.expect("mid-transition height"));
        assert!(more);
        // Ease-out is past halfway at the halfway point.
        assert!(height > 100. && height < 200., "{height}");

        let (height, more) = state.height_for(start + Duration::from_millis(220));
        assert!(!more, "the transition has finished");
        assert_eq!(height, None, "and hands the panel back its natural height");
    }

    #[test]
    fn collapsing_animates_back_down_to_zero() {
        let state = CollapsibleState::new(true);
        measured(&state, 200.);
        state.toggle();
        assert!(!state.is_open());

        let start = Instant::now();
        let (height, more) = state.height_for(start);
        assert!(more);
        assert!(f32::from(height.expect("height")) > 0.);

        let (height, more) = state.height_for(start + Duration::from_millis(220));
        assert!(!more);
        assert_eq!(height, Some(px(0.)));
    }

    #[test]
    fn re_opening_mid_collapse_starts_from_where_it_is() {
        let state = CollapsibleState::new(true);
        measured(&state, 200.);
        state.toggle();
        // Interrupt immediately: the panel is still near its full height, so
        // the re-open must not snap to zero first.
        state.set_open(true);
        let (height, _) = state.height_for(Instant::now());
        let height = f32::from(height.expect("height"));
        assert!(height > 150., "restarted from {height}, not from the top");
    }

    #[test]
    fn setting_the_state_it_already_has_does_nothing() {
        let state = CollapsibleState::new(true);
        measured(&state, 200.);
        state.set_open(true);
        assert_eq!(state.height_for(Instant::now()), (None, false));
    }
}

#[cfg(test)]
mod disclosure_tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    struct Section {
        open: bool,
        toggles: Log<bool>,
    }

    impl Render for Section {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let toggles = self.toggles.clone();
            Disclosure::new("advanced", self.open)
                .child("Advanced")
                .on_toggle(cx.listener(move |this, open: &bool, _, cx| {
                    toggles.borrow_mut().push(*open);
                    this.open = *open;
                    cx.notify();
                }))
        }
    }

    #[gpui::test]
    fn enter_and_space_flip_the_focused_header(cx: &mut TestAppContext) {
        let toggles = log();
        let shared = toggles.clone();
        let (_, cx) = cx.add_window_view(|_, _| Section {
            open: false,
            toggles: shared,
        });
        tab(cx);
        press(cx, "enter");
        press(cx, "space");
        assert_eq!(*toggles.borrow(), [true, false]);
    }
}
