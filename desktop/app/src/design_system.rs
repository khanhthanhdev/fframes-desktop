//! Studio's application-owned visual language.
//!
//! Video style presets belong to project content; this module owns the Studio
//! chrome. Keep the active appearance, palette, density and component metrics
//! here so screens do not create their own competing collections of values.

use gpui::{Div, InteractiveElement, Stateful, Styled, px, rgb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemePreset {
    /// Neutral graphite surfaces with a calm blue action color.
    Graphite,
    /// A brighter neutral appearance, available for future appearance settings.
    Pearl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DensityPreset {
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesignSystemConfig {
    pub theme: ThemePreset,
    pub density: DensityPreset,
}

/// Product-wide default. Theme and density choices resolve through this config
/// before any screen composes its colors or measurements.
pub const CONFIG: DesignSystemConfig = DesignSystemConfig {
    theme: ThemePreset::Graphite,
    density: DensityPreset::Comfortable,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub window: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub surface_raised: u32,
    pub surface_hover: u32,
    pub canvas: u32,
    pub border: u32,
    pub border_subtle: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_tint: u32,
    pub selection_overlay: u32,
    pub on_accent: u32,
    pub success: u32,
    pub success_surface: u32,
    pub warning: u32,
    pub warning_surface: u32,
    pub danger: u32,
    pub danger_surface: u32,
    pub user_bubble: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub space_1: f32,
    pub space_2: f32,
    pub space_3: f32,
    pub space_4: f32,
    pub space_6: f32,
    pub radius_sm: f32,
    pub radius_md: f32,
    pub radius_lg: f32,
    pub control_pad_x: f32,
    pub control_pad_y: f32,
    pub sidebar_width: f32,
    pub agent_panel_width: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DesignTokens {
    pub palette: Palette,
    pub metrics: Metrics,
}

const METRICS: Metrics = Metrics {
    space_1: 4.,
    space_2: 8.,
    space_3: 12.,
    space_4: 16.,
    space_6: 24.,
    radius_sm: 6.,
    radius_md: 8.,
    radius_lg: 12.,
    control_pad_x: 12.,
    control_pad_y: 7.,
    sidebar_width: 248.,
    agent_panel_width: 400.,
};

const GRAPHITE: DesignTokens = DesignTokens {
    palette: Palette {
        window: 0x111316,
        sidebar: 0x171a1e,
        surface: 0x1c2025,
        surface_raised: 0x22272d,
        surface_hover: 0x292f36,
        canvas: 0x0c0e11,
        border: 0x30363e,
        border_subtle: 0x252a30,
        text: 0xe6e9ed,
        muted: 0x969faa,
        accent: 0x79b5ff,
        accent_hover: 0x9ac8ff,
        accent_tint: 0x20354a,
        selection_overlay: 0x79b5ff40,
        on_accent: 0x111316,
        success: 0x76c99a,
        success_surface: 0x1b3026,
        warning: 0xe5bf72,
        warning_surface: 0x332c1d,
        danger: 0xe68a7d,
        danger_surface: 0x3a2624,
        user_bubble: 0x223448,
    },
    metrics: METRICS,
};

const PEARL: DesignTokens = DesignTokens {
    palette: Palette {
        window: 0xf4f5f6,
        sidebar: 0xebedef,
        surface: 0xffffff,
        surface_raised: 0xf8f9fa,
        surface_hover: 0xe9edf1,
        canvas: 0xdfe3e8,
        border: 0xd4d9df,
        border_subtle: 0xe4e7eb,
        text: 0x20252b,
        muted: 0x68727d,
        accent: 0x2468ad,
        accent_hover: 0x18578f,
        accent_tint: 0xdceaf7,
        selection_overlay: 0x2468ad40,
        on_accent: 0xffffff,
        success: 0x247a4a,
        success_surface: 0xe2f2e8,
        warning: 0x966600,
        warning_surface: 0xf7efd9,
        danger: 0xb3443a,
        danger_surface: 0xf8e7e4,
        user_bubble: 0xdceaf7,
    },
    metrics: METRICS,
};

const ACTIVE_PALETTE: Palette = match CONFIG.theme {
    ThemePreset::Graphite => GRAPHITE.palette,
    ThemePreset::Pearl => PEARL.palette,
};

/// Named compatibility exports for the current UI surfaces. New code should
/// prefer the semantic fields on [`tokens()`].
pub(crate) mod colors {
    use super::ACTIVE_PALETTE;

    pub(crate) const BACKGROUND: u32 = ACTIVE_PALETTE.window;
    pub(crate) const PANEL: u32 = ACTIVE_PALETTE.surface_raised;
    pub(crate) const BORDER: u32 = ACTIVE_PALETTE.border;
    pub(crate) const TEXT: u32 = ACTIVE_PALETTE.text;
    pub(crate) const MUTED: u32 = ACTIVE_PALETTE.muted;
    pub(crate) const ACCENT: u32 = ACTIVE_PALETTE.accent;
    pub(crate) const WARNING: u32 = ACTIVE_PALETTE.warning_surface;
    pub(crate) const WARNING_TEXT: u32 = ACTIVE_PALETTE.warning;
    pub(crate) const SUCCESS: u32 = ACTIVE_PALETTE.success_surface;
    pub(crate) const SUCCESS_TEXT: u32 = ACTIVE_PALETTE.success;
    pub(crate) const DANGER: u32 = ACTIVE_PALETTE.danger_surface;
    pub(crate) const DANGER_TEXT: u32 = ACTIVE_PALETTE.danger;
    pub(crate) const BUBBLE: u32 = ACTIVE_PALETTE.user_bubble;
    pub(crate) const CARD: u32 = ACTIVE_PALETTE.surface;
    pub(crate) const CANVAS: u32 = ACTIVE_PALETTE.canvas;
}

pub const fn metrics_for(density: DensityPreset) -> Metrics {
    match density {
        DensityPreset::Comfortable => METRICS,
        DensityPreset::Compact => Metrics {
            space_1: 3.,
            space_2: 6.,
            space_3: 10.,
            space_4: 12.,
            space_6: 20.,
            radius_sm: 5.,
            radius_md: 7.,
            radius_lg: 10.,
            control_pad_x: 10.,
            control_pad_y: 5.,
            sidebar_width: 232.,
            agent_panel_width: 376.,
        },
    }
}

const GRAPHITE_COMPACT: DesignTokens = DesignTokens {
    palette: GRAPHITE.palette,
    metrics: metrics_for(DensityPreset::Compact),
};

const PEARL_COMPACT: DesignTokens = DesignTokens {
    palette: PEARL.palette,
    metrics: metrics_for(DensityPreset::Compact),
};

pub const fn tokens() -> &'static DesignTokens {
    match (CONFIG.theme, CONFIG.density) {
        (ThemePreset::Graphite, DensityPreset::Comfortable) => &GRAPHITE,
        (ThemePreset::Graphite, DensityPreset::Compact) => &GRAPHITE_COMPACT,
        (ThemePreset::Pearl, DensityPreset::Comfortable) => &PEARL,
        (ThemePreset::Pearl, DensityPreset::Compact) => &PEARL_COMPACT,
    }
}

pub const fn tokens_for(preset: ThemePreset) -> &'static DesignTokens {
    match (preset, CONFIG.density) {
        (ThemePreset::Graphite, DensityPreset::Comfortable) => &GRAPHITE,
        (ThemePreset::Graphite, DensityPreset::Compact) => &GRAPHITE_COMPACT,
        (ThemePreset::Pearl, DensityPreset::Comfortable) => &PEARL,
        (ThemePreset::Pearl, DensityPreset::Compact) => &PEARL_COMPACT,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    Secondary,
    Primary,
    Selected,
    Destructive,
}

/// Shared baseline for native command buttons. Callers remain responsible for
/// labels, actions, keyboard activation and accessibility names.
pub fn button(element: Stateful<Div>, style: ButtonStyle, enabled: bool) -> Stateful<Div> {
    let tokens = tokens();
    let colors = tokens.palette;
    let (background, border, foreground) = match style {
        ButtonStyle::Secondary => (colors.surface_raised, colors.border, colors.text),
        ButtonStyle::Primary => (colors.accent, colors.accent, colors.on_accent),
        ButtonStyle::Selected => (colors.accent_tint, colors.accent, colors.accent_hover),
        ButtonStyle::Destructive => (colors.danger_surface, colors.danger, colors.danger),
    };
    element
        .px(px(tokens.metrics.control_pad_x))
        .py(px(tokens.metrics.control_pad_y))
        .rounded(px(tokens.metrics.radius_md))
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(background))
        .text_color(rgb(if enabled { foreground } else { colors.muted }))
        .opacity(if enabled { 1. } else { 0.48 })
        .focus_visible(|style| style.border_color(rgb(colors.accent_hover)))
}

/// A consistent inset surface for preview, status and content regions.
pub fn surface(element: Div) -> Div {
    let tokens = tokens();
    element
        .rounded(px(tokens.metrics.radius_lg))
        .border_1()
        .border_color(rgb(tokens.palette.border))
        .bg(rgb(tokens.palette.surface))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_theme_and_density_resolve_to_complete_token_sets() {
        let active = tokens();
        let graphite = tokens_for(ThemePreset::Graphite);
        let pearl = tokens_for(ThemePreset::Pearl);
        assert_eq!(active, graphite);
        assert_ne!(graphite.palette.window, pearl.palette.window);
        assert_ne!(graphite.palette.text, pearl.palette.text);
        assert_eq!(metrics_for(DensityPreset::Comfortable), METRICS);
        assert!(metrics_for(DensityPreset::Compact).sidebar_width < METRICS.sidebar_width);
    }

    #[test]
    fn semantic_status_surfaces_are_distinct_from_the_neutral_surface() {
        let colors = tokens().palette;
        assert_ne!(colors.success_surface, colors.surface);
        assert_ne!(colors.warning_surface, colors.surface);
        assert_ne!(colors.danger_surface, colors.surface);
    }
}
