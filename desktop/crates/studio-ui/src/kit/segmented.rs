//! `SegmentedControl`: exclusive options on a `ctl` trough, the selected one a raised
//! pill. Options carry a label, an icon, or both.
//!
//! The indicator does not slide. Kobalte positions it from the selected
//! trigger's measured rect and animates a transform; this gpui rev has no
//! transform, so the selected item paints its own fill.
use std::rc::Rc;

use gpui::{
    App, Bounds, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, svg,
};

use studio_theme::{METRICS, Shadow, TextStyle, Theme, shadow_for, text_style};

use super::interact::{Nav, States, bounds_probe, dim_disabled, focusable, nav_key, step_enabled};

/// One segment. `selected` is computed by the caller so the control never has
/// to know what the value type is.
#[derive(Debug, Clone)]
pub struct SegmentOption {
    pub label: Option<SharedString>,
    pub icon: Option<SharedString>,
    /// The accessible name of an icon-only segment.
    pub name: Option<SharedString>,
    pub selected: bool,
    pub disabled: bool,
    /// The option's element id; `{id}-{index}` when not set.
    pub id: Option<SharedString>,
}

impl SegmentOption {
    pub fn new(label: impl Into<SharedString>, selected: bool) -> Self {
        Self {
            label: Some(label.into()),
            icon: None,
            name: None,
            selected,
            disabled: false,
            id: None,
        }
    }

    pub fn icon(icon: impl Into<SharedString>, selected: bool) -> Self {
        Self {
            label: None,
            icon: Some(icon.into()),
            name: None,
            selected,
            disabled: false,
            id: None,
        }
    }

    /// Gives the option a stable element id of its own instead of `{id}-{index}`, for
    /// options that already had one.
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The name a screen reader announces for an icon-only segment.
    pub fn named(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// The option a Left or Right key moves to: one step, no wrap, skipping
/// disabled options. `None` when the key is not Left or Right or nothing
/// enabled lies that way.
pub fn segment_target(from: usize, key: Nav, options: &[SegmentOption]) -> Option<usize> {
    let delta = match key {
        Nav::Left => -1,
        Nav::Right => 1,
        _ => return None,
    };
    step_enabled(from, delta, options.len(), |i| options[i].disabled)
}

/// Maps a click on segment `index` back onto a value from a fixed list, which
/// is the one piece of the old per-window `segmented::<T>` helper worth
/// keeping: every call site had the same `T::ALL.get(index)` guard.
pub fn option_at<T: Copy>(values: &[T], index: usize) -> Option<T> {
    values.get(index).copied()
}

/// The index arrives by reference so `cx.listener(|this, index: &usize, ..)`
/// builds one of these directly -- gpui's listeners take exactly one event
/// argument, and the index *is* the event here.
type SegmentHandler = Box<dyn Fn(&usize, &mut Window, &mut App) + 'static>;

/// The trough's inset around its items.
const TROUGH_INSET: f32 = 2.;
/// An item's horizontal padding.
const ITEM_PAD_X: f32 = 10.;

/// Exclusive options on a `ctl` trough; the selected one is a raised `segment_selected`
/// pill. Each option is `{id}-{index}`.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    theme: Theme,
    options: Vec<SegmentOption>,
    stretch: bool,
    tabs: bool,
    on_select: Option<SegmentHandler>,
    on_option_bounds: Option<OptionBoundsHandler>,
}

/// Receives an option's index and its bounds on every prepaint.
type OptionBoundsHandler = Rc<dyn Fn(usize, Bounds<Pixels>, &mut App)>;

impl SegmentedControl {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, options: Vec<SegmentOption>) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            options,
            stretch: false,
            tabs: false,
            on_select: None,
            on_option_bounds: None,
        }
    }

    /// A pill tab bar, Oleafly's `TabsList` ("Providers and keys | CLI agents | …"): the
    /// same trough and raised pill with 28 px items in `Body` text, announced as tabs. For
    /// switching the sub-pages of a page.
    pub fn tabs(mut self) -> Self {
        self.tabs = true;
        self
    }

    /// The options share the control's full width.
    pub fn stretch(mut self) -> Self {
        self.stretch = true;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }

    /// Reports each option's index and bounds, for controls that are measured.
    pub fn on_option_bounds(
        mut self,
        report: impl Fn(usize, Bounds<Pixels>, &mut App) + 'static,
    ) -> Self {
        self.on_option_bounds = Some(Rc::new(report));
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let SegmentedControl {
            id,
            theme,
            options,
            stretch,
            tabs,
            on_select,
            on_option_bounds,
        } = self;
        let (group_role, item_role, item_h, item_pad, item_text) = if tabs {
            (
                Role::TabList,
                Role::Tab,
                METRICS.size.control_h - 2. * TROUGH_INSET,
                METRICS.space_3,
                TextStyle::Body,
            )
        } else {
            (
                Role::RadioGroup,
                Role::RadioButton,
                METRICS.size.control_sm_h,
                ITEM_PAD_X,
                TextStyle::Label,
            )
        };
        let colors = theme.colors;
        let states = States::of(&theme);
        let selected_bg = Hsla::from(colors.segment_selected);
        let selected_text = Hsla::from(colors.text_1);
        let idle_text = Hsla::from(colors.text_2);
        let selected_shadow = shadow_for(theme.appearance, Shadow::RaisedPill).into_vec();

        let handler: Option<Rc<SegmentHandler>> = on_select.map(Rc::new);
        let keyed = Rc::new(options.clone());
        let prefix: SharedString = match &id {
            ElementId::Name(name) => name.clone(),
            other => SharedString::from(format!("{other:?}")),
        };

        div()
            .id(id)
            .role(group_role)
            .flex()
            .flex_row()
            .when(tabs, |this| this.flex_none().self_start())
            .p(px(TROUGH_INSET))
            .rounded(px(METRICS.radius.segment))
            .bg(states.rest)
            .children(options.into_iter().enumerate().map(|(index, option)| {
                let handler = handler.clone();
                let keyed = keyed.clone();
                let selected = option.selected;
                let disabled = option.disabled;
                let name = option.label.clone().or(option.name.clone());

                let item_id = option
                    .id
                    .clone()
                    .unwrap_or_else(|| SharedString::from(format!("{prefix}-{index}")));
                let item = focusable(div().id(item_id), &theme, disabled)
                    .role(item_role)
                    .aria_selected(selected)
                    .relative()
                    .children(on_option_bounds.clone().map(|report| {
                        bounds_probe(Rc::new(move |bounds, cx: &mut App| {
                            report(index, bounds, cx)
                        }))
                    }))
                    .when_some(name, |this, name| this.aria_label(name))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(METRICS.space_1))
                    .px(px(item_pad))
                    .h(px(item_h))
                    .rounded(px(METRICS.radius.item))
                    .when(stretch, |this| this.flex_1());
                dim_disabled(text_style(item, item_text), disabled)
                    .map(|this| {
                        if selected {
                            this.text_color(selected_text)
                                .bg(selected_bg)
                                .shadow(selected_shadow.clone())
                        } else {
                            let this = this.text_color(idle_text);
                            if disabled {
                                this
                            } else {
                                this.hover(move |style| style.bg(states.hover))
                            }
                        }
                    })
                    .children(option.icon.map(|icon| {
                        svg()
                            .path(icon)
                            .size(px(METRICS.size.icon_sm))
                            .flex_shrink_0()
                            .text_color(if selected { selected_text } else { idle_text })
                    }))
                    .children(option.label)
                    .when_some(handler.filter(|_| !disabled), |this, handler| {
                        let keys = handler.clone();
                        this.on_click(move |_, window, cx| handler(&index, window, cx))
                            .on_key_down(move |event, window, cx| {
                                let Some(key @ (Nav::Left | Nav::Right)) =
                                    nav_key(event.keystroke.key.as_str())
                                else {
                                    return;
                                };
                                // Every enabled item is a tab stop, so the key belongs to the
                                // focused one, which need not be the selected one.
                                if let Some(target) = segment_target(index, key, &keyed) {
                                    keys(&target, window, cx);
                                    if key == Nav::Left {
                                        window.focus_prev(cx);
                                    } else {
                                        window.focus_next(cx);
                                    }
                                    cx.stop_propagation();
                                }
                            })
                    })
            }))
    }
}
#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;
    use crate::kit::interact::Nav;
    use crate::kit::test_support::{Log, log, press, tab};

    fn options(disabled: &[bool]) -> Vec<SegmentOption> {
        disabled
            .iter()
            .enumerate()
            .map(|(i, d)| SegmentOption::new(format!("o{i}"), i == 0).disabled(*d))
            .collect()
    }

    #[test]
    fn arrows_move_one_step_without_wrapping() {
        let o = options(&[false, false, false]);
        assert_eq!(segment_target(1, Nav::Right, &o), Some(2));
        assert_eq!(segment_target(1, Nav::Left, &o), Some(0));
        assert_eq!(segment_target(2, Nav::Right, &o), None);
        assert_eq!(segment_target(0, Nav::Left, &o), None);
        assert_eq!(segment_target(0, Nav::Up, &o), None);
    }

    #[test]
    fn disabled_options_are_skipped() {
        let o = options(&[false, true, false]);
        assert_eq!(segment_target(0, Nav::Right, &o), Some(2));
        assert_eq!(segment_target(2, Nav::Left, &o), Some(0));
        let trailing = options(&[false, true]);
        assert_eq!(segment_target(0, Nav::Right, &trailing), None);
    }

    struct Picker {
        selected: usize,
        picks: Log<usize>,
    }

    impl Render for Picker {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let picks = self.picks.clone();
            let options = ["Fit", "Half", "Full"]
                .into_iter()
                .enumerate()
                .map(|(i, label)| SegmentOption::new(label, i == self.selected).disabled(i == 1))
                .collect();
            SegmentedControl::new(&theme, "zoom", options).on_select(cx.listener(
                move |this, index: &usize, _, cx| {
                    picks.borrow_mut().push(*index);
                    this.selected = *index;
                    cx.notify();
                },
            ))
        }
    }

    #[gpui::test]
    fn arrows_step_from_the_focused_segment_not_the_selected_one(cx: &mut TestAppContext) {
        let picks = log();
        let shared = picks.clone();
        let (_, cx) = cx.add_window_view(|_, _| Picker {
            selected: 2,
            picks: shared,
        });
        tab(cx);
        press(cx, "right");
        assert_eq!(
            *picks.borrow(),
            [2],
            "focus is on Fit, so Right reaches Full"
        );
        press(cx, "left");
        assert_eq!(
            *picks.borrow(),
            [2, 0],
            "focus followed the selection to Full"
        );
    }

    #[gpui::test]
    fn right_arrow_selects_the_next_enabled_segment(cx: &mut TestAppContext) {
        let picks = log();
        let shared = picks.clone();
        let (_, cx) = cx.add_window_view(|_, _| Picker {
            selected: 0,
            picks: shared,
        });
        tab(cx);
        press(cx, "right");
        assert_eq!(*picks.borrow(), [2]);
        press(cx, "right");
        assert_eq!(*picks.borrow(), [2], "no wrap past the last segment");
        press(cx, "left");
        assert_eq!(*picks.borrow(), [2, 0]);
        press(cx, "enter");
        assert_eq!(*picks.borrow(), [2, 0, 0]);
    }

    struct Measured(Log<(usize, f32)>);

    impl Render for Measured {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let seen = self.0.clone();
            let options = vec![
                SegmentOption::new("Short", true),
                SegmentOption::new("A much longer label", false),
            ];
            SegmentedControl::new(&theme, "measured", options).on_option_bounds(
                move |index, bounds, _| {
                    seen.borrow_mut()
                        .push((index, f32::from(bounds.size.width)));
                },
            )
        }
    }

    #[gpui::test]
    fn each_option_reports_its_own_bounds(cx: &mut TestAppContext) {
        let seen = log();
        let shared = seen.clone();
        let (_, cx) = cx.add_window_view(|_, _| Measured(shared));
        cx.run_until_parked();
        let seen = seen.borrow();
        let width = |index| {
            seen.iter()
                .rev()
                .find(|(i, _)| *i == index)
                .map(|(_, width)| *width)
                .unwrap()
        };
        assert!(width(0) > 0. && width(1) > width(0), "{seen:?}");
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Quality {
        Low,
        Medium,
        High,
    }

    #[test]
    fn a_click_index_maps_back_onto_its_value() {
        let all = [Quality::Low, Quality::Medium, Quality::High];
        assert_eq!(option_at(&all, 0), Some(Quality::Low));
        assert_eq!(option_at(&all, 2), Some(Quality::High));
    }

    /// The old per-window helper guarded this with `if let Some(value) =
    /// T::ALL.get(index)`; a stale index from a re-rendered control must be a
    /// no-op, never a panic.
    #[test]
    fn an_index_past_the_end_selects_nothing() {
        let all = [Quality::Low, Quality::Medium, Quality::High];
        assert_eq!(option_at(&all, 3), None);
        assert_eq!(option_at::<Quality>(&[], 0), None);
    }

    #[test]
    fn exactly_one_option_is_marked_selected_for_a_value() {
        let all = [Quality::Low, Quality::Medium, Quality::High];
        let current = Quality::Medium;
        let options: Vec<SegmentOption> = all
            .iter()
            .map(|value| SegmentOption::new(format!("{value:?}"), *value == current))
            .collect();
        assert_eq!(options.iter().filter(|option| option.selected).count(), 1);
        assert!(options[1].selected);
    }
}
