//! The shared component kit, ported from Cap's desktop app.
//!
//! Every surface of the studio builds its controls from here: buttons,
//! toggles, selects, sliders, menus, fields, cards and the text input. A view
//! resolves a [`studio_theme::Theme`] once per render and passes it to each
//! component's constructor.
//!
//! Every component takes the theme in its one constructor and has one look: it reads the
//! colour roles and `METRICS` itself, so call sites never pass colours or sizes.
//!
//! ## Event handlers
//!
//! Handlers are `impl Fn(&ClickEvent, &mut Window, &mut App)`, which is exactly
//! what `cx.listener(..)` produces for any view type. Components are
//! therefore view-agnostic without a generic parameter.

mod badge;
mod button;
mod collapsible;
mod color_picker;
mod icon;
mod interact;
mod kbd;
mod menu;
mod notice;
mod number_field;
mod progress;
mod radio_cards;
mod segmented;
mod select;
mod selectable_text;
mod settings_frame;
mod slider;
mod surface;
mod tab_rail;
#[cfg(test)]
pub(crate) mod test_support;
mod text;
pub mod text_input;
mod toggle;
mod tooltip;
mod windows_caption;

pub use badge::{Badge, StatusDot, Tone, tone_color, tone_surface};
pub use button::{Button, ButtonSize, ClickHandler, Emphasis, IconButton, IconButtonSize};
pub use collapsible::{Collapsible, CollapsibleSection, CollapsibleState, Disclosure};
pub use color_picker::{
    ColorPicker, ColorPickerSnapshot, color_swatch, hsv_to_rgb, hue_from_point, preview_ring,
    rgb_to_hsv, sv_from_point,
};
pub use icon::{Icon, icon};
pub use interact::{
    BoundsHandler, DISABLED_OPACITY, FOCUS_RING_PX, Handler, Nav, States, activates, bounds_probe,
    dim_disabled, focus_ring, focusable, nav_key, on_activate, reveal_actions, step_enabled,
    with_tooltip,
};
pub use kbd::{KbdChip, kbd_symbol};
pub use menu::{Menu, MenuItem, MenuKey, MenuState};
pub use notice::{DETAILS_MAX_H, Notice, notice_surface, tone_icon};
pub use number_field::{
    NumberChange, NumberField, NumberFieldState, NumberLimits, format_number, parse_number,
};
pub use progress::CircularProgress;
pub use radio_cards::{RadioCard, RadioCards};
pub use segmented::{SegmentOption, SegmentedControl, option_at, segment_target};
pub use select::Select;
pub use selectable_text::{
    CopySelection, SelectableText, bind_selectable_text_keys, selectable, selected_text,
};
pub use settings_frame::{SETTINGS_NAV_PAD, SETTINGS_NAV_W, SettingsFrame, SettingsNavItem};
pub use slider::{
    Slider, SliderDrag, SliderTrack, fraction_from_x, slider_key, snap_to_step,
    value_at as slider_value_at, value_from_fraction,
};
pub use surface::{Card, Popover, divider};
pub use tab_rail::{TabRail, TabRailItem, tab_target};
pub use text::{heading, label, muted};
pub use text_input::{
    TextInput, TextInputEvent, TextInputState, bind_keys as bind_text_input_keys,
    text_input_has_focus,
};
pub use toggle::{Toggle, toggled};
pub use tooltip::{TOOLTIP_SHOW_DELAY, Tooltip};
pub use windows_caption::{CAPTION_BUTTON_W, windows_caption_controls};

// Phase 3 additions (plans/261027-2045-studio-design-system/phase-03-kit-additions.md).
// `PanelSection` and `FormField` are the plan's `Section` and `Field`, named apart from
// `surface::Section` and `field::Field` until phase 7 retires those.
mod chip;
mod divider;
mod empty_state;
mod form_field;
mod list_row;
mod panel_section;
mod progress_bar;
mod resize_grip;
mod spinner;
mod toast;

pub use chip::Chip;
pub use divider::Divider;
pub use empty_state::{EmptyState, EmptyStateSize};
pub use form_field::FormField;
pub use list_row::{List, ListRow};
pub use panel_section::PanelSection;
pub use progress_bar::Progress;
pub use resize_grip::ResizeGrip;
pub use spinner::{Spinner, SpinnerKind};
pub use toast::{HostedToast, TOAST_DURATION, Toast, ToastHost, toast_remaining};
