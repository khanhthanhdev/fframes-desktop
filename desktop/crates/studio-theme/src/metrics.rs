//! Spacing, radii and sizes shared by every Studio surface.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub space_1: f32,
    pub space_2: f32,
    pub space_3: f32,
    pub space_4: f32,
    pub space_6: f32,
    pub radius_control: f32,
    pub radius_item: f32,
    pub radius_segment: f32,
    pub radius_card: f32,
    pub radius_popover: f32,
    pub radius_tab: f32,
    pub radius_frame: f32,
    pub control_h: f32,
    pub select_h: f32,
    pub input_h: f32,
    pub field_row_h: f32,
    pub icon_sm: f32,
    pub icon_md: f32,
    pub header_h: f32,
    pub tab_bar_h: f32,
    pub toolbar_h: f32,
    pub transport_h: f32,
    pub gutter: f32,
    /// The tabbed sidebar right of the player: its default width, and the range its grip
    /// keeps it in.
    pub sidebar_w: f32,
    pub sidebar_min_w: f32,
    pub sidebar_max_w: f32,
    /// The player card never gets narrower than this while it fits.
    pub editor_min_w: f32,
    pub status_bar_h: f32,
    pub timeline_default_h: f32,
    pub timeline_min_h: f32,
    pub player_min_h: f32,
    pub ruler_h: f32,
    pub track_h: f32,
    pub track_gap: f32,
    pub track_gutter_w: f32,
    /// The rounded icon tile in a track's gutter.
    pub track_tile: f32,
    /// The timeline card's padding: left and right, top, bottom.
    pub timeline_pad_x: f32,
    pub timeline_pad_top: f32,
    pub timeline_pad_bottom: f32,

    /// The settings page gap (was 28).
    pub space_8: f32,
    /// Corner radii, grouped. The flat `radius_*` fields above hold the same values until
    /// their callers move here.
    pub radius: Radius,
    /// Component sizes, grouped.
    pub size: Size,
    /// Shell, screen and timeline-painter dimensions.
    pub layout: Layout,
    /// Opacity of whole elements.
    pub opacity: Opacity,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radius {
    pub control: f32,
    pub item: f32,
    pub segment: f32,
    pub tab: f32,
    pub card: f32,
    /// Popovers and menus use the medium control radius.
    pub popover: f32,
    /// Fully round ends (chips, badges, toggles).
    pub pill: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub control_h: f32,
    pub control_sm_h: f32,
    pub select_h: f32,
    pub input_h: f32,
    pub field_row_h: f32,
    /// A one-line and a two-line list row.
    pub list_row_h: f32,
    pub list_row_2_h: f32,
    pub icon_sm: f32,
    pub icon_md: f32,
    pub icon_lg: f32,
    /// The square icon buttons: small, medium and large (`IconButtonSize`).
    pub icon_button_sm: f32,
    pub icon_button_md: f32,
    pub icon_button_lg: f32,
    pub chip_h: f32,
    /// A small media thumbnail's side.
    pub thumb_sm: f32,
    /// A colour swatch that opens a colour picker.
    pub swatch: f32,
    pub slider_track: f32,
    pub slider_thumb: f32,
    pub progress_h: f32,
    /// A resize grip's visible bar and the width that takes the pointer.
    pub grip_w: f32,
    pub grip_h: f32,
    pub grip_hit: f32,
    pub divider: f32,
    pub focus_ring: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub header_h: f32,
    pub tab_bar_h: f32,
    pub toolbar_h: f32,
    pub transport_h: f32,
    pub status_bar_h: f32,
    /// The space between the shell's panes: none, the player, sidebar and timeline meet at
    /// hairlines, after Oleafly's flat workspace.
    pub gutter: f32,
    pub sidebar_w: f32,
    pub sidebar_min_w: f32,
    pub sidebar_max_w: f32,
    pub editor_min_w: f32,
    /// The player's height below which the timeline gives way (`PLAYER_MIN_H`).
    pub player_min_h: f32,
    /// The least height of the player's stage, the frame's area between toolbar and
    /// transport.
    pub stage_min_h: f32,

    /// Header: room for the macOS traffic lights, the title's minimum, and the action
    /// cluster's width with a project open and without one (`shell/header.rs`).
    pub traffic_lights_w: f32,
    /// The traffic lights' inset from the window's top-left corner, on macOS.
    pub traffic_light_inset: f32,
    pub title_min_w: f32,
    pub actions_w: f32,
    pub actions_closed_w: f32,
    /// The app icon beside the header's title (Oleafly's `size-5` brand mark).
    pub header_logo: f32,
    /// The open project's colour dot in the header's breadcrumb (Oleafly's `size-2`).
    pub project_dot: f32,

    /// Transport: the tools' gap, the zoom slider's width, and the player widths below
    /// which the bar goes compact, narrow, and drops the total time (`shell/transport.rs`).
    pub transport_tools_gap: f32,
    pub zoom_slider_w: f32,
    pub transport_compact_below: f32,
    pub transport_narrow_below: f32,
    pub transport_total_hidden_below: f32,

    /// The settings window: size, minimum size, and the Windows title strip.
    pub settings_w: f32,
    pub settings_h: f32,
    pub settings_min_w: f32,
    pub settings_min_h: f32,
    pub windows_title_h: f32,

    /// The start screen card's maximum width; the slides notes field's height.
    pub start_max_w: f32,
    pub slides_notes_h: f32,
    /// The widest a Create or Export workflow column grows (Oleafly's dialog `max-w-2xl`).
    pub workflow_max_w: f32,
    /// The tallest the start screen's recent projects and the Project tab's library (recent
    /// projects under the files) grow before they scroll.
    pub start_recents_max_h: f32,
    pub library_max_h: f32,
    /// The slide number column of a Slides row; the CSS import report's scroll height.
    pub slide_number_w: f32,
    pub css_report_max_h: f32,

    /// Timeline card.
    pub timeline_default_h: f32,
    pub timeline_min_h: f32,
    pub timeline_pad_x: f32,
    pub timeline_pad_top: f32,
    pub timeline_pad_bottom: f32,
    pub ruler_h: f32,
    pub track_h: f32,
    pub track_gap: f32,
    pub track_gutter_w: f32,
    pub track_tile: f32,

    /// Timeline painters (`timeline/view.rs`): waveform top pad, trim handle width, and a
    /// slide stop's flag (top, width, height) and dash.
    pub waveform_pad: f32,
    pub trim_handle_w: f32,
    pub stop_flag_top: f32,
    pub stop_flag_w: f32,
    pub stop_flag_h: f32,
    pub stop_dash: f32,
    /// The rounded corners of a stop flag's pointing side.
    pub stop_flag_radius: f32,
    /// Segment label insets and accent bar (`timeline/segment.rs`).
    pub segment_label_pad_left: f32,
    pub segment_label_pad_right: f32,
    pub segment_accent_bar_w: f32,
    /// Ruler ticks and labels (`timeline/ruler.rs`).
    pub ruler_major_tick_h: f32,
    pub ruler_minor_tick_h: f32,
    pub ruler_label_w: f32,
    pub ruler_label_bottom: f32,
    /// The playhead's head and line (`timeline/playhead.rs`).
    pub playhead_head: f32,
    pub playhead_line: f32,
    /// The minimap (`timeline/minimap.rs`).
    pub minimap_max_w: f32,
    pub minimap_h: f32,
    pub minimap_bar_h: f32,
    pub minimap_min_chip_w: f32,
    pub minimap_handle_w: f32,
    /// The corners of the minimap's bar and chip.
    pub minimap_radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Opacity {
    pub disabled: f32,
    /// An element being dragged.
    pub dragging: f32,
    /// Media that is muted or excluded.
    pub muted_media: f32,
    /// A slide stop's line through the timeline lanes while it is neither selected nor
    /// dragged.
    pub stop_default: f32,
}

pub const METRICS: Metrics = Metrics {
    space_1: 4.,
    space_2: 8.,
    space_3: 12.,
    space_4: 16.,
    space_6: 24.,
    radius_control: 8.,
    radius_item: 6.,
    radius_segment: 8.,
    radius_card: 10.,
    radius_popover: 8.,
    radius_tab: 6.,
    radius_frame: 6.,
    control_h: 32.,
    select_h: 36.,
    input_h: 36.,
    field_row_h: 40.,
    icon_sm: 14.,
    icon_md: 16.,
    header_h: 52.,
    tab_bar_h: 46.,
    toolbar_h: 44.,
    transport_h: 48.,
    gutter: 0.,
    sidebar_w: 416.,
    sidebar_min_w: 320.,
    sidebar_max_w: 520.,
    editor_min_w: 480.,
    status_bar_h: 28.,
    timeline_default_h: 260.,
    timeline_min_h: 240.,
    player_min_h: 320.,
    ruler_h: 26.,
    track_h: 44.,
    track_gap: 6.,
    track_gutter_w: 104.,
    track_tile: 22.,
    timeline_pad_x: 11.,
    timeline_pad_top: 9.,
    timeline_pad_bottom: 11.,

    space_8: 32.,
    radius: Radius {
        control: 8.,
        item: 6.,
        segment: 8.,
        tab: 6.,
        card: 10.,
        popover: 8.,
        pill: 999.,
    },
    size: Size {
        control_h: 32.,
        control_sm_h: 24.,
        select_h: 36.,
        input_h: 36.,
        field_row_h: 40.,
        list_row_h: 32.,
        list_row_2_h: 44.,
        icon_sm: 14.,
        icon_md: 16.,
        icon_lg: 20.,
        icon_button_sm: 24.,
        icon_button_md: 28.,
        icon_button_lg: 32.,
        chip_h: 28.,
        thumb_sm: 56.,
        swatch: 20.,
        slider_track: 3.,
        slider_thumb: 12.,
        progress_h: 4.,
        grip_w: 4.,
        grip_h: 36.,
        grip_hit: 14.,
        divider: 0.5,
        focus_ring: 2.,
    },
    layout: Layout {
        header_h: 52.,
        tab_bar_h: 46.,
        toolbar_h: 44.,
        transport_h: 48.,
        status_bar_h: 28.,
        gutter: 0.,
        sidebar_w: 416.,
        sidebar_min_w: 320.,
        sidebar_max_w: 520.,
        editor_min_w: 480.,
        player_min_h: 320.,
        stage_min_h: 120.,

        traffic_lights_w: 82.,
        traffic_light_inset: 20.,
        title_min_w: 270.,
        actions_w: 440.,
        actions_closed_w: 236.,
        header_logo: 20.,
        project_dot: 8.,

        transport_tools_gap: 6.,
        zoom_slider_w: 72.,
        transport_compact_below: 1270.,
        transport_narrow_below: 830.,
        transport_total_hidden_below: 520.,

        settings_w: 782.,
        settings_h: 775.,
        settings_min_w: 780.,
        settings_min_h: 560.,
        windows_title_h: 36.,

        start_max_w: 520.,
        slides_notes_h: 96.,
        workflow_max_w: 672.,
        start_recents_max_h: 260.,
        library_max_h: 360.,
        slide_number_w: 20.,
        css_report_max_h: 200.,

        timeline_default_h: 260.,
        timeline_min_h: 240.,
        timeline_pad_x: 11.,
        timeline_pad_top: 9.,
        timeline_pad_bottom: 11.,
        ruler_h: 26.,
        track_h: 44.,
        track_gap: 6.,
        track_gutter_w: 104.,
        track_tile: 22.,

        waveform_pad: 6.,
        trim_handle_w: 6.,
        stop_flag_top: 16.,
        stop_flag_w: 7.,
        stop_flag_h: 8.,
        stop_dash: 3.,
        stop_flag_radius: 2.,
        segment_label_pad_left: 13.,
        segment_label_pad_right: 10.,
        segment_accent_bar_w: 3.,
        ruler_major_tick_h: 8.,
        ruler_minor_tick_h: 5.,
        ruler_label_w: 52.,
        ruler_label_bottom: 10.,
        playhead_head: 10.,
        playhead_line: 1.5,
        minimap_max_w: 78.,
        minimap_h: 10.,
        minimap_bar_h: 4.,
        minimap_min_chip_w: 20.,
        minimap_handle_w: 8.,
        minimap_radius: 2.,
    },
    opacity: Opacity {
        disabled: 0.5,
        dragging: 0.7,
        muted_media: 0.55,
        stop_default: 0.85,
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_match_studio_spec() {
        let m = METRICS;
        assert_eq!(
            [m.space_1, m.space_2, m.space_3, m.space_4, m.space_6],
            [4., 8., 12., 16., 24.]
        );
        assert_eq!(
            [
                m.radius_control,
                m.radius_item,
                m.radius_segment,
                m.radius_card,
                m.radius_popover,
                m.radius_tab,
                m.radius_frame
            ],
            [8., 6., 8., 10., 8., 6., 6.]
        );
        assert_eq!(
            [
                m.control_h,
                m.select_h,
                m.input_h,
                m.field_row_h,
                m.icon_sm,
                m.icon_md
            ],
            [32., 36., 36., 40., 14., 16.]
        );
        assert_eq!(
            [
                m.header_h,
                m.tab_bar_h,
                m.toolbar_h,
                m.transport_h,
                m.gutter,
                m.timeline_default_h,
                m.timeline_min_h,
                m.player_min_h
            ],
            [52., 46., 44., 48., 0., 260., 240., 320.]
        );
        assert_eq!(
            [
                m.sidebar_w,
                m.sidebar_min_w,
                m.sidebar_max_w,
                m.editor_min_w,
                m.status_bar_h
            ],
            [416., 320., 520., 480., 28.]
        );
        assert_eq!(
            [
                m.ruler_h,
                m.track_h,
                m.track_gap,
                m.track_gutter_w,
                m.track_tile
            ],
            [26., 44., 6., 104., 22.]
        );
        assert_eq!(
            [m.timeline_pad_x, m.timeline_pad_top, m.timeline_pad_bottom],
            [11., 9., 11.]
        );
    }

    #[test]
    fn grouped_metrics_match_the_flat_fields_they_replace() {
        let m = METRICS;
        let (r, s, l) = (m.radius, m.size, m.layout);
        assert_eq!(
            [r.control, r.item, r.segment, r.tab, r.card, r.popover],
            [
                m.radius_control,
                m.radius_item,
                m.radius_segment,
                m.radius_tab,
                m.radius_card,
                m.radius_popover
            ]
        );
        assert_eq!(r.item, m.radius_frame, "frame merges into item");
        assert_eq!(
            [
                s.control_h,
                s.select_h,
                s.input_h,
                s.field_row_h,
                s.icon_sm,
                s.icon_md
            ],
            [
                m.control_h,
                m.select_h,
                m.input_h,
                m.field_row_h,
                m.icon_sm,
                m.icon_md
            ]
        );
        assert_eq!(
            [
                l.header_h,
                l.tab_bar_h,
                l.toolbar_h,
                l.transport_h,
                l.status_bar_h,
                l.gutter,
                l.sidebar_w,
                l.sidebar_min_w,
                l.sidebar_max_w,
                l.editor_min_w,
                l.player_min_h,
            ],
            [
                m.header_h,
                m.tab_bar_h,
                m.toolbar_h,
                m.transport_h,
                m.status_bar_h,
                m.gutter,
                m.sidebar_w,
                m.sidebar_min_w,
                m.sidebar_max_w,
                m.editor_min_w,
                m.player_min_h,
            ]
        );
        assert_eq!(
            [
                l.timeline_default_h,
                l.timeline_min_h,
                l.timeline_pad_x,
                l.timeline_pad_top,
                l.timeline_pad_bottom,
                l.ruler_h,
                l.track_h,
                l.track_gap,
                l.track_gutter_w,
                l.track_tile,
            ],
            [
                m.timeline_default_h,
                m.timeline_min_h,
                m.timeline_pad_x,
                m.timeline_pad_top,
                m.timeline_pad_bottom,
                m.ruler_h,
                m.track_h,
                m.track_gap,
                m.track_gutter_w,
                m.track_tile,
            ]
        );
    }

    #[test]
    fn new_metrics_match_the_phase_1_table() {
        let m = METRICS;
        let (s, l, o) = (m.size, m.layout, m.opacity);
        assert_eq!(m.space_8, 32.);
        assert_eq!(m.radius.pill, 999.);
        assert_eq!(
            [
                s.control_sm_h,
                s.list_row_h,
                s.list_row_2_h,
                s.icon_lg,
                s.icon_button_sm,
                s.icon_button_md,
                s.icon_button_lg,
                s.chip_h,
                s.thumb_sm,
            ],
            [24., 32., 44., 20., 24., 28., 32., 28., 56.]
        );
        assert_eq!(
            [
                s.slider_track,
                s.slider_thumb,
                s.progress_h,
                s.grip_w,
                s.grip_h,
                s.grip_hit,
                s.divider,
                s.focus_ring,
            ],
            [3., 12., 4., 4., 36., 14., 0.5, 2.]
        );
        assert_eq!([o.disabled, o.dragging, o.muted_media], [0.5, 0.7, 0.55]);
        assert_eq!(
            [
                l.traffic_lights_w,
                l.title_min_w,
                l.actions_w,
                l.actions_closed_w
            ],
            [82., 270., 440., 236.]
        );
        assert_eq!(
            [
                l.transport_compact_below,
                l.transport_narrow_below,
                l.transport_total_hidden_below,
                l.zoom_slider_w,
            ],
            [1270., 830., 520., 72.]
        );
        assert_eq!(
            [
                l.settings_w,
                l.settings_h,
                l.settings_min_w,
                l.settings_min_h,
                l.windows_title_h
            ],
            [782., 775., 780., 560., 36.]
        );
        assert_eq!(
            [l.start_max_w, l.slides_notes_h, l.workflow_max_w],
            [520., 96., 672.]
        );
        // The breakpoints only make sense in this order.
        assert!(l.transport_compact_below > l.transport_narrow_below);
        assert!(l.transport_narrow_below > l.transport_total_hidden_below);
        assert!(l.settings_min_w <= l.settings_w && l.settings_min_h <= l.settings_h);
    }
}
