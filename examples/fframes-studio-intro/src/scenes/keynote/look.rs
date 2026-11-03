//! The look of the 16:9 cut: the Editorial Engineering system of
//! `docs/design/README.md`, on the roles of `design-system/tokens.css` in a blue
//! variant: the action colour and the inverse surface are blue and navy instead
//! of green and forest, and the neutrals are cool. A quiet paper canvas, Geist
//! for claims and explanations, Geist Mono only for commands, identifiers, stage
//! numbers and metadata, 4 px corners and thin rules. Colours are roles, never
//! decoration: the user's target is pink, work under consideration slate blue,
//! an accepted result green, and every state also has a label or a shape.

use fframes::{FFramesContext, FFramesSyncedVideoFrame, Frame, Svgr, SyncVideoFrameInput};

use crate::beat::*;

pub const SANS: &str = "Geist";
pub const MONO: &str = "Geist Mono";
/// Weight of display and section headings.
pub const DISPLAY_WEIGHT: u16 = 450;

// surfaces
pub const BG: &str = "#fafbfd";
pub const PANEL: &str = "#fafbfd";
/// Inset areas inside a panel: tracks, wells, empty bars.
pub const EMPHASIS: &str = "#e6ebf3";
/// Code, terminals and other featured technical artifacts.
pub const INVERSE: &str = "#14243f";

// text
pub const TEXT: &str = "#1f2633";
pub const SECONDARY: &str = "#5f6878";
pub const INVERSE_TEXT: &str = "#f3f6fb";
pub const INVERSE_SECONDARY: &str = "#b4c2d8";
/// The prompt and highlights of a command on the inverse surface.
pub const COMMAND: &str = "#9cc7ff";

// lines
/// Decorative separators and panel edges.
pub const RULE: &str = "#dde2ea";
/// Boundaries that need to read as such: an empty node, a control.
pub const CONTROL: &str = "#7d8796";

// action and states
/// A deliberate next step: buttons, the act numbers, progress.
pub const ACTION: &str = "#2350b5";
pub const ON_ACTION: &str = "#ffffff";
/// The user's target or supplied context.
pub const SELECTION: &str = "#a85070";
pub const SELECTION_SURFACE: &str = "#f9e1e9";
/// Work under consideration: a draft, a render in progress, the GPU lane.
pub const CANDIDATE: &str = "#4a6f9c";
pub const CANDIDATE_SURFACE: &str = "#e3eefb";
/// An accepted state or a named check that passed.
pub const ACCEPTED: &str = "#245e45";
pub const ACCEPTED_SURFACE: &str = "#e4eddf";
/// What goes wrong the old way. The system has no error role (selection pink is
/// not one), so this muted brick is the cut's one documented exception, always
/// paired with a label.
pub const FAILURE: &str = "#a3402e";

/// Corner radius of panels and controls.
pub const RADIUS: f32 = 4.0;
/// Width of rules and panel edges at 1080p.
pub const RULE_WIDTH: f32 = 1.5;

/// Opacity and vertical offset of something that rises into place at beat `at`.
pub fn rise(lb: f32, at: f32, distance: f32) -> (f32, f32) {
    (prog(lb, at, at + 0.75), (1.0 - snap(lb - at)) * distance)
}

/// 1 until beat `at`, then fades to 0 within half a beat.
pub fn leave(lb: f32, at: f32) -> f32 {
    1.0 - prog(lb, at, at + 0.5)
}

/// `translate(x y)`.
pub fn at(x: f32, y: f32) -> String {
    format!("translate({x} {y})")
}

/// Scale around a point.
pub fn zoom(cx: f32, cy: f32, s: f32) -> String {
    format!("translate({cx} {cy}) scale({s}) translate({} {})", -cx, -cy)
}

/// Spaced-out mono caps: an eyebrow, a stage number or a piece of metadata.
pub fn eyebrow(x: f32, y: f32, text: impl Into<String>, color: &'static str) -> Svgr<'static> {
    let text = text.into();
    fframes::svgr!(
        <text x={x} y={y} font-family={MONO} font-weight="500" font-size="22" letter-spacing="3" fill={color}>{text}</text>
    )
}

/// A HUD label: small mono caps.
pub fn label(x: f32, y: f32, text: impl Into<String>, anchor: &'static str) -> Svgr<'static> {
    let text = text.into();
    fframes::svgr!(
        <text x={x} y={y} font-family={MONO} font-weight="500" font-size="18" letter-spacing="2.5"
              fill={SECONDARY} text-anchor={anchor}>{text}</text>
    )
}

/// A headline in the display weight with tight, relative tracking.
pub fn headline(
    x: f32,
    y: f32,
    text: impl Into<String>,
    size: f32,
    fill: &'static str,
) -> Svgr<'static> {
    let text = text.into();
    fframes::svgr!(
        <text x={x} y={y} font-family={SANS} font-weight={DISPLAY_WEIGHT} font-size={size} letter-spacing={-size * 0.04} fill={fill}>{text}</text>
    )
}

/// The act number and name top left, the way every act opens.
pub fn act_title(
    lb: f32,
    number: &'static str,
    name: &'static str,
    title: &'static str,
) -> Svgr<'static> {
    let (o1, d1) = rise(lb, 0.0, 24.0);
    let (o2, d2) = rise(lb, 0.5, 40.0);
    fframes::svgr!(
        <g>
            <g opacity={o1} transform={at(0.0, d1)}>{act_eyebrow(number, name)}</g>
            <g opacity={o2} transform={at(0.0, d2)}>{headline(116.0, 252.0, title, 72.0, TEXT)}</g>
        </g>
    )
}

/// `01 / THE PROBLEM`: the act number in the action colour, its name secondary.
pub fn act_eyebrow(number: &'static str, name: &'static str) -> Svgr<'static> {
    fframes::svgr!(
        <text x="120" y="168" font-family={MONO} font-weight="500" font-size="22" letter-spacing="3" fill={ACTION}>{number}<tspan fill={SECONDARY}>" / "{name}</tspan></text>
    )
}

/// The plain-English caption under the picture: a lead in the colour of its
/// role and the explanation in primary text, centred at the bottom of the frame.
pub fn caption(
    lb: f32,
    from: f32,
    until: f32,
    lead: &'static str,
    rest: &'static str,
    color: &'static str,
) -> Svgr<'static> {
    if lb < from || lb > until + 0.5 {
        return Svgr::empty();
    }
    let (o, d) = rise(lb, from, 18.0);
    let o = o * leave(lb, until);
    fframes::svgr!(
        <g opacity={o} transform={at(0.0, d)}>
            <text x="960" y="974" text-anchor="middle" font-family={SANS} font-weight="400" font-size="32" letter-spacing="-0.3" fill={TEXT}><tspan font-weight="500" fill={color}>{lead}</tspan>" "{rest}</text>
        </g>
    )
}

/// A surface with a thin rule for an edge.
pub fn panel(x: f32, y: f32, w: f32, h: f32) -> Svgr<'static> {
    fframes::svgr!(
        <rect x={x} y={y} width={w} height={h} rx={RADIUS} fill={PANEL} stroke={RULE} stroke-width={RULE_WIDTH} />
    )
}

/// The inverse surface of a terminal or a code listing.
pub fn terminal(x: f32, y: f32, w: f32, h: f32) -> Svgr<'static> {
    fframes::svgr!(<rect x={x} y={y} width={w} height={h} rx={RADIUS} fill={INVERSE} />)
}

/// An application window: a panel with a title bar.
pub fn window(x: f32, y: f32, w: f32, h: f32, title: impl Into<String>) -> Svgr<'static> {
    let title = title.into();
    fframes::svgr!(
        <g>
            {panel(x, y, w, h)}
            <path d={format!("M{} {} H{}", x, y + 44.0, x + w)} stroke={RULE} stroke-width={RULE_WIDTH} />
            <circle cx={x + 22.0} cy={y + 22.0} r="5" fill="none" stroke={CONTROL} stroke-width="1.5" />
            <circle cx={x + 40.0} cy={y + 22.0} r="5" fill="none" stroke={CONTROL} stroke-width="1.5" />
            <circle cx={x + 58.0} cy={y + 22.0} r="5" fill="none" stroke={CONTROL} stroke-width="1.5" />
            <text x={x + w / 2.0} y={y + 28.0} text-anchor="middle" font-family={MONO} font-weight="400" font-size="16" fill={SECONDARY}>{title}</text>
        </g>
    )
}

/// A check mark of `size` px whose left end sits at (`x`, `y`).
pub fn check(x: f32, y: f32, size: f32, color: &'static str) -> Svgr<'static> {
    let d = format!(
        "M{x} {y} L{} {} L{} {}",
        x + size * 0.36,
        y + size * 0.34,
        x + size,
        y - size * 0.42
    );
    fframes::svgr!(
        <path d={d} fill="none" stroke={color} stroke-width={(size * 0.14).max(2.0)} stroke-linecap="round" stroke-linejoin="round" />
    )
}

/// A horizontal arrow from `x1` to `x2` on the line `y`.
pub fn arrow(x1: f32, x2: f32, y: f32, color: &'static str) -> Svgr<'static> {
    let d = format!(
        "M{x1} {y} H{x2} M{} {} L{x2} {y} L{} {}",
        x2 - 6.0,
        y - 5.0,
        x2 - 6.0,
        y + 5.0
    );
    fframes::svgr!(
        <path d={d} fill="none" stroke={color} stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
    )
}

/// A mouse pointer with its tip at (`x`, `y`).
pub fn pointer(x: f32, y: f32) -> Svgr<'static> {
    fframes::svgr!(
        <path transform={at(x, y)} d="M0 0 L0 30 L8 23 L14 36 L20 33 L14 21 L24 21 Z"
              fill={TEXT} stroke={BG} stroke-width="2" stroke-linejoin="round" />
    )
}

/// A frame of the clip `name` filling the box (cropped to it), playing from
/// `start_from` seconds of the scene. `poster` stands in before that and where
/// frames cannot be decoded (the web editor).
pub fn clip(
    frame: &Frame,
    ctx: &FFramesContext<'_, '_>,
    name: &str,
    poster: &str,
    start_from: f32,
    (x, y, w, h): (f32, f32, f32, f32),
) -> Svgr<'static> {
    let input = SyncVideoFrameInput {
        start_from,
        looping: true,
        editor_fallback_image: ctx.get_image(poster),
        ..Default::default()
    };
    let Some(image) = frame
        .get_synced_video_frame(ctx, name, &input)
        .map(|f| f.into_image())
    else {
        return still(ctx, poster, (x, y, w, h));
    };
    fframes::svgr!(
        <image href={image.href()} x={x} y={y} width={w} height={h} preserveAspectRatio="xMidYMid slice" />
    )
}

/// The image `name` filling the box (cropped to it).
pub fn still(
    ctx: &FFramesContext<'_, '_>,
    name: &str,
    (x, y, w, h): (f32, f32, f32, f32),
) -> Svgr<'static> {
    let Some(image) = ctx.get_image(name) else {
        return fframes::svgr!(<rect x={x} y={y} width={w} height={h} fill={EMPHASIS} />);
    };
    fframes::svgr!(
        <image href={image.href()} x={x} y={y} width={w} height={h} preserveAspectRatio="xMidYMid slice" />
    )
}

/// Clips `body` to a rounded box.
pub fn rounded(
    id: &str,
    (x, y, w, h): (f32, f32, f32, f32),
    r: f32,
    body: Svgr<'static>,
) -> Svgr<'static> {
    let url = format!("url(#{id})");
    fframes::svgr!(
        <g>
            <clipPath id={id.to_owned()}><rect x={x} y={y} width={w} height={h} rx={r} /></clipPath>
            <g clip-path={url}>{body}</g>
        </g>
    )
}
