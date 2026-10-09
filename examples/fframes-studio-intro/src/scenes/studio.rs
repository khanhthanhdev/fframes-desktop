//! The product demo: one continuous Studio window that a camera moves
//! around. Select the title on the canvas, prompt the agent, watch it
//! retrieve the code behind the selection and edit it, preview the rebuilt
//! video and export it. All state is derived from the beat, so the scene
//! scrubs in both directions.

use std::fmt::Write;

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use super::first_run;
use crate::beat::*;
use crate::shaders::SHADERS;
use crate::ui::*;

beat_scene!(StudioScene, Some(16.0), Some(80.0));

/// Scene beat the prompt starts to be typed and the beat it is sent.
const TYPE_AT: f32 = 14.0;
const SEND: f32 = 31.0;
/// Scene beat of the first frame of the rebuilt video.
const RESULT: f32 = 52.0;
/// The same beats on the global grid, for the soundtrack.
pub const TYPE_START: f32 = 16.0 + TYPE_AT;
pub const SEND_AT: f32 = 16.0 + SEND;
pub const RESULT_AT: f32 = 16.0 + RESULT;
/// Clicks, tool calls and the accept button, on the global grid.
pub const BLIP_BEATS: &[f32] = &[26.0, 30.0, 49.5, 52.0, 61.0, 63.5, 68.0, 72.8, 76.3];

/// When things happen before the prompt is sent. The 9:16 cut starts on the
/// workspace; the 16:9 cut starts on the first run of the app (`first_run.rs`) and
/// selects later, but sends the prompt on the same beat.
pub(crate) struct Plan {
    select: f32,
    scope: f32,
    focus: f32,
    type_at: f32,
    type_end: f32,
    hint_at: f32,
    canvas_at: f32,
    play_at: f32,
    cursor: &'static [(f32, f32, f32)],
    clicks: &'static [f32],
    first_run: bool,
}

pub(crate) const CLASSIC: Plan = Plan {
    select: 10.0,
    scope: 10.8,
    focus: 13.8,
    type_at: TYPE_AT,
    type_end: 30.0,
    hint_at: 1.5,
    canvas_at: 1.5,
    play_at: 1.5,
    cursor: CURSOR,
    clicks: &CLICKS,
    first_run: false,
};

pub(crate) const FIRST_RUN: Plan = Plan {
    select: 17.5,
    scope: 18.3,
    focus: 20.6,
    type_at: RUN_TYPE_AT,
    type_end: 29.8,
    hint_at: first_run::CHAT_END,
    canvas_at: 15.9,
    play_at: 16.2,
    cursor: CURSOR_RUN,
    clicks: &CLICKS_RUN,
    first_run: true,
};

/// Scene beat the prompt starts to be typed in the first-run cut.
const RUN_TYPE_AT: f32 = 20.8;
pub const RUN_TYPE_START: f32 = 16.0 + RUN_TYPE_AT;
/// Sounds of the first-run cut on the global grid: the first run, the selection,
/// the focus in the chat, then the tool calls of the edit.
pub const RUN_BLIP_BEATS: &[f32] = &[
    26.0, 27.6, 29.6, 30.8, 32.2, 33.5, 36.6, 49.5, 52.0, 61.0, 63.5, 68.0, 72.8, 76.3,
];

// ---------------------------------------------------------------------------
// layout of the mock window, in window coordinates

const WIN_W: f32 = 1600.0;
const WIN_H: f32 = 740.0;
const PANEL: &str = "#131211";
const LINE: &str = DIM;
const GREEN: &str = "#6fcf8a";
const RED: &str = "#e5594f";
const WARM: &str = "#0f1b33";

const CANVAS_X: f32 = 295.0;
const CANVAS_Y: f32 = 62.0;
const CANVAS_W: f32 = 760.0;
const CANVAS_H: f32 = 427.0;
const TITLE_X: f32 = 44.0;
const TITLE_Y: f32 = 232.0;
const TITLE: &str = "Career Tech Day";
const SLOGAN: &str = "Future of Tech";

const CHAT_X: f32 = 1100.0;
/// Characters on the first line of the prompt in the chat.
const SPLIT: usize = 29;
const PROMPT: &str = "add the slogan Future of Tech and spring it in from below";

// ---------------------------------------------------------------------------
// camera: (beat, center x, center y, scale) in window coordinates

const CAMERA: &[(f32, f32, f32, f32)] = &[
    (0.0, 800.0, 370.0, 0.95),
    (8.0, 800.0, 370.0, 1.0),
    (15.5, 800.0, 370.0, 1.0),
    (17.0, 640.0, 300.0, 1.22),
    (19.0, 640.0, 300.0, 1.22),
    (20.5, 1180.0, 470.0, 1.5),
    (31.0, 1180.0, 470.0, 1.5),
    (33.0, 600.0, 340.0, 1.2),
    (49.0, 600.0, 340.0, 1.2),
    (52.0, 900.0, 330.0, 1.15),
    (59.0, 900.0, 330.0, 1.15),
    (60.5, 800.0, 370.0, 1.0),
    (64.0, 800.0, 370.0, 1.0),
];

fn camera(lb: f32) -> (f32, f32, f32) {
    camera_in(CAMERA, lb)
}

/// Position of a camera path at a beat, eased between its keys.
pub(crate) fn camera_in(keys: &[(f32, f32, f32, f32)], lb: f32) -> (f32, f32, f32) {
    for w in keys.windows(2) {
        if lb < w[1].0 {
            let t = cubic_in_out(prog(lb, w[0].0, w[1].0));
            return (
                lerp(w[0].1, w[1].1, t),
                lerp(w[0].2, w[1].2, t),
                lerp(w[0].3, w[1].3, t),
            );
        }
    }
    let last = keys[keys.len() - 1];
    (last.1, last.2, last.3)
}

/// (start beat, tag, headline): the step shown above the window.
const CAPTIONS: &[(f32, &str, &str)] = &[
    (0.0, "01 / DESCRIBE", "Start with a title and a sentence."),
    (10.4, "02 / GENERATE", "It writes the code. Then renders."),
    (17.0, "03 / SELECT", "Click anything on the canvas."),
    (20.0, "04 / PROMPT", "Say what should change."),
    (32.0, "05 / RETRIEVE", "The agent finds the exact code."),
    (44.0, "06 / EDIT", "A real diff in your Rust."),
    (52.0, "07 / PREVIEW", "Rebuilt, checked, playing."),
    (60.0, "08 / EXPORT", "Happy? Export an MP4."),
];

fn captions(lb: f32) -> Svgr<'static> {
    let items: Vec<Svgr> = CAPTIONS
        .iter()
        .enumerate()
        .filter_map(|(i, (start, tag, head))| {
            let next = CAPTIONS.get(i + 1).map(|c| c.0);
            let appear = prog(lb, *start, *start + 0.5);
            let leave = next.map_or(0.0, |n| prog(lb, n - 0.4, n));
            let o = appear * (1.0 - leave);
            if o <= 0.0 {
                return None;
            }
            let dy = (1.0 - snap(lb - start)) * 40.0 - expo_in(leave) * 30.0;
            Some(fframes::svgr!(
                <g opacity={o} transform={format!("translate(0 {dy})")}>
                    <text x="160" y="128" font-family={MONO} font-weight="600" font-size="22" letter-spacing="4" fill={ACCENT}>{*tag}</text>
                    <text x="160" y="204" font-family={DISPLAY} font-size="68" letter-spacing="-2" fill={BONE}>{*head}</text>
                </g>
            ))
        })
        .collect();
    fframes::svgr!(<g>{items}</g>)
}

impl Scene for StudioScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, mut frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let (cx, cy, s) = camera(lb);
        let bg = SHADERS.contour.draw(
            &frame,
            ShaderUniforms::new()
                .float2("uWell", 0.5, 0.85)
                .float("uDepth", 0.5)
                .float("uDensity", 14.0)
                .float("uBright", 0.14 + (-(lb - 32.0).abs() * 0.8).exp() * 0.12)
                .color("uInk", Color::hex("#8a857c"))
                .color("uHot", Color::hex(ACCENT)),
        );
        let win = window_with(&mut frame, ctx, lb, &FIRST_RUN);
        let enter = prog(lb, 0.0, 0.8);
        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1920" height="1080" />
                {captions(lb)}
                <clipPath id="viewport"><rect x="100" y="236" width="1720" height="772" /></clipPath>
                <g clip-path="url(#viewport)" opacity={enter}>
                    <g transform={format!("translate(960 610) scale({s}) translate({} {})", -cx, -cy)}>{win}</g>
                </g>
            </g>
        )
    }
}

// ---------------------------------------------------------------------------
// the window

fn section_in(lb: f32, at: f32) -> (f32, f32) {
    (prog(lb, at, at + 0.6), (1.0 - snap(lb - at)) * 26.0)
}

pub(crate) fn window(frame: &mut Frame, ctx: &FFramesContext, lb: f32) -> Svgr<'static> {
    window_with(frame, ctx, lb, &CLASSIC)
}

pub(crate) fn window_with(
    frame: &mut Frame,
    ctx: &FFramesContext,
    lb: f32,
    pl: &Plan,
) -> Svgr<'static> {
    let selected = (pl.select..RESULT).contains(&lb);
    let rv = if pl.first_run {
        prog(lb, first_run::REVEAL_FROM, first_run::REVEAL_TO)
    } else {
        1.0
    };
    let (overlay, log) = if pl.first_run {
        (first_run::modal(lb), first_run::log(lb))
    } else {
        (Svgr::empty(), Svgr::empty())
    };
    let (o1, d1) = section_in(lb, 0.4);
    let (o2, d2) = section_in(lb, 0.9);
    let (o3, d3) = section_in(lb, 1.4);
    let (o4, d4) = section_in(lb, 1.9);
    let focus = prog(lb, 32.0, 32.6) * (1.0 - prog(lb, 49.0, 50.0));
    let canvas = canvas(frame, ctx, lb, pl, rv);
    fframes::svgr!(
        <g>
            <rect x="0" y="0" width={WIN_W} height={WIN_H} rx="14" fill="#0e0d0c" stroke="#3a3731" stroke-width="2" />
            <g opacity={o1} transform={format!("translate(0 {})", -d1)}>{titlebar(lb, pl)}</g>
            <g opacity={o2 * rv} transform={format!("translate({} 0)", -d2)}>{left_panel(selected)}</g>
            <g opacity={o3}>{canvas}</g>
            <g opacity={o3 * rv} transform={format!("translate(0 {d3})")}>{transport(lb, pl)}</g>
            <g opacity={o4 * rv} transform={format!("translate(0 {d4})")}>{timeline(lb, selected, pl)}</g>
            <g opacity={o2} transform={format!("translate({d2} 0)")}>{chat(lb, pl)}</g>
            {overlay}
            {log}
            <rect x="0" y="0" width={WIN_W} height={WIN_H} rx="14" fill="#000000" opacity={focus * 0.55} />
            {drawer(lb)}
            {export_overlay(lb)}
            {cursor(lb, pl)}
        </g>
    )
}

fn titlebar(lb: f32, pl: &Plan) -> Svgr<'static> {
    let rev = if lb >= RESULT + 4.0 { "rev 4" } else { "rev 3" };
    let fresh = pl.first_run && lb < first_run::REVEAL_TO;
    let name = if pl.first_run && lb < first_run::GENERATE {
        "untitled"
    } else {
        "vinuni-tech-day"
    };
    let status = if fresh {
        "new project".to_owned()
    } else {
        format!("{rev}  ·  built")
    };
    let press = prog(lb, 60.1, 60.4) * (1.0 - prog(lb, 60.6, 61.0));
    fframes::svgr!(
        <g>
            <rect x="0" y="0" width={WIN_W} height="48" rx="14" fill={PANEL} />
            <rect x="0" y="30" width={WIN_W} height="18" fill={PANEL} />
            <rect x="0" y="47" width={WIN_W} height="1" fill={LINE} />
            <text x="28" y="32" font-family={MONO} font-weight="600" font-size="20" fill={BONE}>{name}</text>
            <circle cx="262" cy="24" r="5" fill={GREEN} />
            <text x="278" y="31" font-family={MONO} font-weight="500" font-size="17" fill={GREY}>{status}</text>
            <rect x="1290" y="9" width="150" height="30" rx="15" fill="none" stroke="#4a4640" stroke-width="1.5" />
            <circle cx="1311" cy="24" r="5" fill={ACCENT} />
            <text x="1328" y="30" font-family={MONO} font-weight="600" font-size="16" letter-spacing="2" fill={BONE}>"CODEX"</text>
            <g transform={format!("translate(1528 24) scale({}) translate(-1528 -24)", 1.0 - press * 0.06)}>
                <rect x="1466" y="8" width="108" height="32" rx="6" fill={ACCENT} />
                <text x="1520" y="30" text-anchor="middle" font-family={MONO} font-weight="600" font-size="16" letter-spacing="2" fill={INK}>"EXPORT"</text>
            </g>
        </g>
    )
}

fn left_panel(selected: bool) -> Svgr<'static> {
    let scenes = [("Intro", "4.0s"), ("Features", "2.8s"), ("Outro", "2.5s")];
    let rows: Vec<Svgr> = scenes
        .iter()
        .enumerate()
        .map(|(i, (name, dur))| {
            let y = 100.0 + i as f32 * 52.0;
            let hot = i == 0 && selected;
            fframes::svgr!(
                <g>
                    <rect x="12" y={y} width="226" height="40" rx="6" fill={if hot { WARM } else { "none" }} stroke={if hot { ACCENT } else { "none" }} stroke-width="1.5" />
                    <text x="28" y={y + 27.0} font-family={MONO} font-weight="500" font-size="20" fill={BONE}>{*name}</text>
                    <text x="224" y={y + 26.0} text-anchor="end" font-family={MONO} font-weight="500" font-size="16" fill={GREY}>{*dur}</text>
                </g>
            )
        })
        .collect();
    fframes::svgr!(
        <g>
            <rect x="0" y="48" width="250" height="492" fill={PANEL} />
            <rect x="250" y="48" width="1" height="492" fill={LINE} />
            {label(20.0, 84.0, "SCENES".to_owned(), GREY, 15.0, "start")}
            {rows}
            {label(20.0, 300.0, "STYLE".to_owned(), GREY, 15.0, "start")}
            <rect x="12" y="316" width="226" height="72" rx="6" fill="none" stroke="#3a3731" stroke-width="1.5" />
            <rect x="28" y="332" width="20" height="20" fill="#0b0b0b" stroke="#4a4640" />
            <rect x="54" y="332" width="20" height="20" fill={ACCENT} />
            <rect x="80" y="332" width="20" height="20" fill={BONE} />
            <rect x="106" y="332" width="20" height="20" fill={EMBER} />
            <text x="28" y="376" font-family={MONO} font-weight="500" font-size="17" fill={BONE}>"Ember"</text>
        </g>
    )
}

// ---------------------------------------------------------------------------
// the video inside the canvas

fn canvas(frame: &mut Frame, ctx: &FFramesContext, lb: f32, pl: &Plan, rv: f32) -> Svgr<'static> {
    let after = lb >= RESULT;
    let size = 64;
    // the title comes in with the canvas
    let t = (lb - pl.canvas_at).max(0.0);
    let (dy, o) = ((1.0 - expo_out(t / 1.5)) * 36.0, prog(t, 0.0, 0.5));
    // the slogan the agent adds springs in from below
    let ts = (lb - RESULT).max(0.0);
    let (slogan_dy, slogan_o) = (
        (1.0 - soft(ts - 0.3)) * 150.0,
        if after { prog(ts, 0.2, 0.5) } else { 0.0 },
    );
    let flash = if after {
        (-(lb - RESULT) * 6.0).exp() * 0.5
    } else {
        0.0
    };
    let sub_y = TITLE_Y + size as f32 * 0.62;
    let sub_o = prog(t, 0.4, 0.9);
    let slogan_y = TITLE_Y + 100.0;
    let pulse_r = 120.0 + pulse(lb, 6.0) * 4.0;

    let width = measure(frame, ctx, DISPLAY, size, 400, false, TITLE);
    let sel = (pl.select..RESULT).contains(&lb);
    let sel_in = snap(lb - pl.select);
    let bx = TITLE_X - 10.0;
    let by = TITLE_Y - size as f32 * 0.80 - 8.0;
    let bw = width + 20.0;
    let bh = size as f32 * 1.06 + 16.0;
    let handles: Vec<Svgr> = [(bx, by), (bx + bw, by), (bx, by + bh), (bx + bw, by + bh)]
        .iter()
        .map(|(hx, hy)| {
            fframes::svgr!(<rect x={hx - 6.0} y={hy - 6.0} width="12" height="12" fill="#0b0b0b" stroke={ACCENT} stroke-width="2" />)
        })
        .collect();
    let selection = if sel {
        fframes::svgr!(
            <g opacity={sel_in.min(1.0)} transform={format!("translate({} {}) scale({}) translate({} {})", bx + bw / 2.0, by + bh / 2.0, 1.0 + (1.0 - sel_in) * 0.08, -(bx + bw / 2.0), -(by + bh / 2.0))}>
                <rect x={bx} y={by} width={bw} height={bh} fill="none" stroke={ACCENT} stroke-width="2" />
                {handles}
                <rect x={bx} y={by - 26.0} width="132" height="26" fill={ACCENT} />
                <text x={bx + 10.0} y={by - 8.0} font-family={MONO} font-weight="600" font-size="15" letter-spacing="2" fill={INK}>"TITLE"</text>
            </g>
        )
    } else {
        Svgr::empty()
    };

    let empty = if pl.first_run {
        fframes::svgr!(
            <g opacity={1.0 - rv}>
                <rect x={CANVAS_X} y={CANVAS_Y} width={CANVAS_W} height={CANVAS_H} fill="none" stroke="#3a3731" stroke-width="2" stroke-dasharray="14 10" />
                <text x={CANVAS_X + CANVAS_W / 2.0} y={CANVAS_Y + CANVAS_H / 2.0 + 8.0} text-anchor="middle" font-family={MONO} font-weight="600" font-size="20" letter-spacing="5" fill={GREY}>"NO VIDEO YET"</text>
            </g>
        )
    } else {
        Svgr::empty()
    };

    fframes::svgr!(
        <g>
            <clipPath id="canvas-clip"><rect x={CANVAS_X} y={CANVAS_Y} width={CANVAS_W} height={CANVAS_H} /></clipPath>
            <rect x={CANVAS_X - 2.0} y={CANVAS_Y - 2.0} width={CANVAS_W + 4.0} height={CANVAS_H + 4.0} fill="none" stroke="#3a3731" stroke-width="2" />
            {empty}
            <g clip-path="url(#canvas-clip)" opacity={rv}>
                <g transform={format!("translate({CANVAS_X} {CANVAS_Y})")}>
                    <rect x="0" y="0" width={CANVAS_W} height={CANVAS_H} fill="#10141c" />
                    <circle cx="640" cy="110" r={pulse_r + 50.0} fill={EMBER} opacity="0.55" />
                    <circle cx="640" cy="110" r={pulse_r} fill={ACCENT} />
                    <rect x="44" y="360" width="220" height="4" fill={ACCENT} />
                    <g opacity={o} transform={format!("translate(0 {dy})")}>
                        <text x={TITLE_X} y={TITLE_Y} font-family={DISPLAY} font-size={size} letter-spacing={-(size as f32) * 0.03} fill={BONE}>{TITLE}</text>
                    </g>
                    <g opacity={sub_o}>
                        <text x={TITLE_X + 2.0} y={sub_y + dy * 0.2} font-family={MONO} font-weight="500" font-size="18" letter-spacing="3" fill={BONE}>"VINUNI  ·  2026"</text>
                    </g>
                    <g opacity={slogan_o} transform={format!("translate(0 {slogan_dy})")}>
                        <text x={TITLE_X} y={slogan_y} font-family={DISPLAY} font-size="48" letter-spacing="-1" fill={ACCENT}>{SLOGAN}</text>
                    </g>
                    {selection}
                    <rect x="0" y="0" width={CANVAS_W} height={CANVAS_H} fill={BONE} opacity={flash} />
                </g>
            </g>
        </g>
    )
}

fn playhead(lb: f32, pl: &Plan) -> f32 {
    if lb < RESULT {
        0.30 * expo_out(prog(lb, pl.play_at, pl.play_at + 3.5))
    } else {
        0.05 + 0.25 * cubic_in_out(prog(lb, RESULT + 0.3, RESULT + 4.0))
    }
}

fn transport(lb: f32, pl: &Plan) -> Svgr<'static> {
    let ph = playhead(lb, pl);
    let time = format!("00:{:04.1} / 00:27.0", ph * 27.0);
    fframes::svgr!(
        <g>
            <path d="M306 502 L306 530 L328 516 Z" fill={BONE} />
            <text x="346" y="523" font-family={MONO} font-weight="500" font-size="17" fill={GREY}>{time}</text>
            <rect x="560" y="514" width="495" height="4" fill={DIM} />
            <rect x="560" y="514" width={(495.0 * ph / 0.4).clamp(0.5, 495.0)} height="4" fill={ACCENT} />
        </g>
    )
}

fn timeline(lb: f32, selected: bool, pl: &Plan) -> Svgr<'static> {
    let ph = playhead(lb, pl);
    let hx = 20.0 + ph * 1060.0;
    let blocks = [
        ("Intro", 20.0, 500.0),
        ("Features", 524.0, 276.0),
        ("Outro", 804.0, 276.0),
    ];
    let scenes: Vec<Svgr> = blocks
        .iter()
        .enumerate()
        .map(|(i, (name, x, w))| {
            let hot = i == 0 && selected;
            fframes::svgr!(
                <g>
                    <rect x={*x} y="592" width={*w} height="48" rx="4" fill={if hot { WARM } else { "#1b1a18" }} stroke={if hot { ACCENT } else { LINE }} stroke-width="1.5" />
                    <text x={x + 14.0} y="623" font-family={MONO} font-weight="500" font-size="18" fill={if hot { BONE } else { GREY }}>{*name}</text>
                </g>
            )
        })
        .collect();
    let mut bars = String::new();
    for i in 0..106 {
        let x = 20.0 + i as f32 * 10.0;
        let h = 6.0 + hash(i as f32) * 38.0 * (0.5 + 0.5 * ((i as f32) * 0.11).sin().abs());
        let _ = write!(bars, "M{x} {} V{} ", 684.0 - h / 2.0, 684.0 + h / 2.0);
    }
    fframes::svgr!(
        <g>
            <rect x="0" y="540" width="1100" height="200" fill={PANEL} />
            <rect x="0" y="540" width="1100" height="1" fill={LINE} />
            <rect x="1100" y="540" width="1" height="200" fill={LINE} />
            {label(20.0, 574.0, "TIMELINE".to_owned(), GREY, 15.0, "start")}
            {scenes}
            <path d={bars} stroke="#4a4640" stroke-width="5" fill="none" />
            <rect x={hx - 1.0} y="584" width="2" height="140" fill={ACCENT} />
            <path d={format!("M{} 580 h12 l-6 10 Z", hx - 6.0)} fill={ACCENT} />
        </g>
    )
}

// ---------------------------------------------------------------------------
// the agent chat

/// (scene beat, text) of the agent's tool calls.
const TOOL_CALLS: &[(f32, &str)] = &[
    (33.5, "Resolved selection: Title"),
    (36.0, "Read src/scenes/intro.rs:38-61"),
    (45.0, "Edited intro.rs  +3 -0"),
    (47.5, "cargo build"),
    (49.5, "inspect: 0 problems"),
];

fn chat(lb: f32, pl: &Plan) -> Svgr<'static> {
    let typed = if lb < SEND {
        ((prog(lb, pl.type_at, pl.type_end) * PROMPT.len() as f32) as usize).min(PROMPT.len())
    } else {
        0
    };
    let shown = &PROMPT[..typed];
    let split = SPLIT.min(typed);
    let line1 = shown[..split].to_owned();
    let line2 = shown[split..].trim_start().to_owned();
    let focus = (pl.focus..SEND).contains(&lb);
    let caret_line2 = typed > SPLIT;
    let caret_chars = if caret_line2 {
        line2.chars().count()
    } else {
        line1.chars().count()
    };
    let caret_x = 1138.0 + caret_chars as f32 * 13.2;
    let caret_y = if caret_line2 { 680.0 } else { 650.0 };
    let caret_on = focus && (lb * 2.0).fract() < 0.6;

    let hint = (1.0 - prog(lb, SEND - 0.3, SEND)).min(prog(lb, pl.hint_at, pl.hint_at + 0.7));
    let sent = prog(lb, SEND, SEND + 0.2);
    let sent_y = (1.0 - snap(lb - SEND)) * 420.0;
    let scope = prog(lb, pl.scope, pl.scope + 0.3) * (1.0 - prog(lb, SEND - 0.2, SEND));
    let scope_dy = (1.0 - snap(lb - pl.scope)) * 16.0;

    let calls: Vec<Svgr> = TOOL_CALLS
        .iter()
        .enumerate()
        .map(|(i, (at, text))| {
            let y = 250.0 + i as f32 * 44.0;
            let done = prog(lb, at + 1.2, at + 1.7);
            fframes::svgr!(
                <g opacity={prog(lb, *at, at + 0.3)} transform={format!("translate({} 0)", (1.0 - snap(lb - at)) * 24.0)}>
                    <circle cx="1134" cy={y - 7.0} r="6" fill={ACCENT} />
                    <text x="1154" y={y} font-family={MONO} font-weight="500" font-size="19" fill="#b9b4ab">{*text}</text>
                    {check(1546.0, y - 12.0, done)}
                </g>
            )
        })
        .collect();

    let rev = prog(lb, RESULT - 0.5, RESULT);
    let accepted = prog(lb, 56.8, 57.1);
    fframes::svgr!(
        <g>
            <rect x={CHAT_X} y="48" width="500" height="692" fill={PANEL} />
            <rect x={CHAT_X} y="48" width="1" height="692" fill={LINE} />
            {label(1120.0, 84.0, "AGENT".to_owned(), GREY, 15.0, "start")}
            <g opacity={hint}>
                <text x="1120" y="140" font-family={MONO} font-weight="500" font-size="20" fill={GREY}>"Select an element on the canvas,"</text>
                <text x="1120" y="170" font-family={MONO} font-weight="500" font-size="20" fill={GREY}>"or describe a change below."</text>
            </g>
            <g opacity={sent} transform={format!("translate(0 {sent_y})")}>
                <rect x="1120" y="104" width="460" height="92" rx="10" fill="#1d1b19" />
                <text x="1140" y="140" font-family={MONO} font-weight="500" font-size="20" fill={BONE}>"add the slogan Future of Tech"</text>
                <text x="1140" y="170" font-family={MONO} font-weight="500" font-size="20" fill={BONE}>"and spring it in from below"</text>
            </g>
            {calls}
            <g opacity={rev} transform={format!("translate(0 {})", (1.0 - snap(lb - RESULT + 0.5)) * 20.0)}>
                <rect x="1120" y="478" width="460" height="108" rx="10" fill="#0f0e0d" stroke={if accepted > 0.5 { GREEN } else { ACCENT }} stroke-width="1.5" />
                <text x="1140" y="512" font-family={MONO} font-weight="600" font-size="21" fill={BONE}>"Revision 4 ready"</text>
                <text x="1140" y="540" font-family={MONO} font-weight="500" font-size="16" fill={GREY}>"slogan added, springs in"</text>
                <rect x="1140" y="552" width="70" height="26" rx="4" fill="none" stroke="#4a4640" stroke-width="1.5" />
                <text x="1175" y="571" text-anchor="middle" font-family={MONO} font-weight="600" font-size="14" letter-spacing="2" fill={GREY}>"UNDO"</text>
                <rect x="1226" y="552" width="110" height="26" rx="4" fill={if accepted > 0.5 { GREEN } else { ACCENT }} />
                <text x="1281" y="571" text-anchor="middle" font-family={MONO} font-weight="600" font-size="14" letter-spacing="2" fill={INK}>{if accepted > 0.5 { "ACCEPTED" } else { "ACCEPT" }}</text>
            </g>
            <g opacity={scope} transform={format!("translate(0 {scope_dy})")}>
                <rect x="1120" y="586" width="336" height="34" rx="17" fill={WARM} stroke={ACCENT} stroke-width="1.5" />
                <text x="1140" y="609" font-family={MONO} font-weight="600" font-size="17" fill={ACCENT}>"Title  ·  Intro  ·  1.2s"</text>
                <text x="1436" y="609" text-anchor="middle" font-family={MONO} font-weight="600" font-size="18" fill={ACCENT}>"×"</text>
            </g>
            <rect x="1120" y="632" width="460" height="88" rx="12" fill="#0b0b0b" stroke={if focus { ACCENT } else { "#3a3731" }} stroke-width="2" />
            <text x="1138" y="656" font-family={MONO} font-weight="500" font-size="22" fill={GREY} opacity={if typed == 0 && !focus { 1.0 } else { 0.0 }}>"Describe a change..."</text>
            <text x="1138" y="656" font-family={MONO} font-weight="500" font-size="22" fill={BONE}>{line1}</text>
            <text x="1138" y="686" font-family={MONO} font-weight="500" font-size="22" fill={BONE}>{line2}</text>
            <rect x={caret_x} y={caret_y - 8.0} width="3" height="26" fill={ACCENT} opacity={if caret_on { 1.0 } else { 0.0 }} />
        </g>
    )
}

fn check(x: f32, y: f32, p: f32) -> Svgr<'static> {
    if p <= 0.0 {
        return Svgr::empty();
    }
    fframes::svgr!(
        <path d={format!("M{x} {y} l5 5 l10 -11")} fill="none" stroke={GREEN} stroke-width="2.5"
              stroke-linecap="round" stroke-linejoin="round" stroke-dasharray={format!("{} 30", 30.0 * p.min(1.0))} />
    )
}

// ---------------------------------------------------------------------------
// the code drawer: retrieval, then the diff

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Context,
    /// Context until the edit lands, then a removed line.
    Old,
    Add,
    Gap,
}

/// (line number, text, kind, beat it appears on)
const CODE: &[(&str, &str, Kind, f32)] = &[
    (
        "52",
        "<text x=\"88\" y=\"464\" font-family=\"Archivo Black\"",
        Kind::Context,
        36.6,
    ),
    (
        "53",
        "      font-size=\"64\" fill=\"#ece8e1\">",
        Kind::Context,
        36.9,
    ),
    ("54", "  Career Tech Day</text>", Kind::Context, 37.2),
    ("..", "", Kind::Gap, 37.5),
    (
        "55",
        "<text x=\"88\" y=\"560\" font-family=\"Archivo Black\"",
        Kind::Add,
        45.4,
    ),
    (
        "56",
        "      font-size=\"48\" fill=\"#2f80ff\">",
        Kind::Add,
        45.8,
    ),
    ("57", "  Future of Tech</text>", Kind::Add, 46.2),
];

const DX: f32 = 130.0;
const DY: f32 = 96.0;
const DW: f32 = 900.0;
const DH: f32 = 520.0;

fn drawer(lb: f32) -> Svgr<'static> {
    let vis = prog(lb, 32.0, 32.5) * (1.0 - prog(lb, 49.0, 50.0));
    if vis <= 0.0 {
        return Svgr::empty();
    }
    let dy = (1.0 - snap(lb - 32.0)) * 90.0 + expo_in(prog(lb, 49.0, 50.0)) * 50.0;
    let edit = prog(lb, 44.5, 45.0);

    let nodes = ["SELECTION", "TITLE @ INTRO", "intro.rs", "L38-61"];
    let lit_at = [33.0, 34.4, 36.0, 37.6];
    let node_els: Vec<Svgr> = nodes
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let x = DX + 32.0 + i as f32 * 210.0;
            let lit = prog(lb, lit_at[i], lit_at[i] + 0.3);
            let arrow = if i < 3 {
                let a = prog(lb, lit_at[i] + 0.2, lit_at[i] + 0.8);
                fframes::svgr!(<path d={format!("M{} {} h{}", x + 186.0, DY + 94.0, 20.0 * a)} stroke={ACCENT} stroke-width="2" fill="none" />)
            } else {
                Svgr::empty()
            };
            fframes::svgr!(
                <g>
                    <rect x={x} y={DY + 70.0} width="184" height="48" rx="6" fill={if lit > 0.5 { WARM } else { "none" }} stroke={if lit > 0.5 { ACCENT } else { "#3a3731" }} stroke-width="1.5" />
                    <text x={x + 92.0} y={DY + 101.0} text-anchor="middle" font-family={MONO} font-weight="600" font-size="16" letter-spacing="1" fill={if lit > 0.5 { BONE } else { GREY }}>{*name}</text>
                    {arrow}
                </g>
            )
        })
        .collect();

    let mut row_y = DY + 176.0;
    let rows: Vec<Svgr> = CODE
        .iter()
        .map(|(num, text, kind, at)| {
            let grow = prog(lb, *at, *at + 0.4);
            let h = match kind {
                Kind::Add => 34.0 * grow,
                _ => 34.0 * grow.min(1.0),
            };
            let y = row_y;
            row_y += h;
            if grow <= 0.0 {
                return Svgr::empty();
            }
            let removed = *kind == Kind::Old && edit > 0.5;
            let added = *kind == Kind::Add;
            let tint = if removed { RED } else if added { GREEN } else { ACCENT };
            let tint_o = if removed { 0.16 } else if added { 0.14 } else { 0.0 };
            let marker = if removed { "-" } else if added { "+" } else { "" };
            let text_fill = if removed { "#d19a95" } else if added { "#bfe9cd" } else { "#c9c4ba" };
            // the retrieved lines get an accent bar as the scan reaches them
            let bar = if matches!(kind, Kind::Context | Kind::Old) { grow } else { 0.0 };
            let indent = (text.len() - text.trim_start().len()) as f32 * 12.0;
            let text = (*text).to_owned();
            let gap = *kind == Kind::Gap;
            fframes::svgr!(
                <g opacity={grow.min(1.0)}>
                    <rect x={DX + 12.0} y={y - 24.0} width={DW - 24.0} height="32" fill={tint} opacity={tint_o} />
                    <rect x={DX + 12.0} y={y - 24.0} width="4" height="32" fill={ACCENT} opacity={bar * 0.9} />
                    <text x={DX + 40.0} y={y} font-family={MONO} font-weight="500" font-size="20" fill="#5d5953">{*num}</text>
                    <text x={DX + 82.0} y={y} font-family={MONO} font-weight="600" font-size="20" fill={tint}>{marker}</text>
                    <text x={DX + 104.0 + indent} y={y} font-family={MONO} font-weight="500" font-size="20" fill={text_fill}>{if gap { "···".to_owned() } else { text.trim_start().to_owned() }}</text>
                </g>
            )
        })
        .collect();

    let badge = if lb < 45.0 {
        ("RETRIEVED  L38-61", prog(lb, 37.6, 38.0))
    } else {
        ("DIFF  +3 -0", prog(lb, 45.0, 45.3))
    };
    fframes::svgr!(
        <g opacity={vis} transform={format!("translate(0 {dy})")}>
            <rect x={DX + 10.0} y={DY + 14.0} width={DW} height={DH} rx="14" fill="#000000" opacity="0.5" />
            <rect x={DX} y={DY} width={DW} height={DH} rx="14" fill="#0a0a09" stroke={ACCENT} stroke-width="2" />
            <text x={DX + 32.0} y={DY + 44.0} font-family={MONO} font-weight="600" font-size="21" fill={BONE}>"src/scenes/intro.rs"</text>
            <text x={DX + DW - 32.0} y={DY + 44.0} text-anchor="end" font-family={MONO} font-weight="600" font-size="16" letter-spacing="2" fill={ACCENT} opacity={badge.1}>{badge.0}</text>
            {node_els}
            <rect x={DX} y={DY + 140.0} width={DW} height="1" fill={LINE} />
            {rows}
        </g>
    )
}

// ---------------------------------------------------------------------------
// export

fn export_overlay(lb: f32) -> Svgr<'static> {
    let vis = prog(lb, 60.8, 61.2);
    if vis <= 0.0 {
        return Svgr::empty();
    }
    let p = prog(lb, 61.4, 63.4);
    let done = p >= 1.0;
    let status = if done {
        "Saved".to_owned()
    } else {
        format!("Rendering  {:>3}%", (p * 100.0) as u32)
    };
    fframes::svgr!(
        <g opacity={vis} transform={format!("translate(0 {})", (1.0 - snap(lb - 60.8)) * 30.0)}>
            <rect x="900" y="64" width="580" height="150" rx="12" fill="#0a0a09" stroke={if done { GREEN } else { ACCENT }} stroke-width="2" />
            <text x="932" y="110" font-family={MONO} font-weight="600" font-size="24" fill={BONE}>"vinuni-tech-day.mp4"</text>
            <text x="932" y="142" font-family={MONO} font-weight="500" font-size="17" fill={GREY}>"1080p  ·  60 fps  ·  H.264"</text>
            <rect x="932" y="166" width="516" height="8" fill={DIM} />
            <rect x="932" y="166" width={(516.0 * p).max(0.5)} height="8" fill={if done { GREEN } else { ACCENT }} />
            <text x="932" y="200" font-family={MONO} font-weight="600" font-size="17" fill={if done { GREEN } else { BONE }}>{status}</text>
        </g>
    )
}

// ---------------------------------------------------------------------------
// the pointer

/// (beat, x, y) in window coordinates.
const CURSOR: &[(f32, f32, f32)] = &[
    (5.5, 1000.0, 700.0),
    (9.6, 470.0, 272.0),
    (10.8, 470.0, 272.0),
    (13.8, 1290.0, 676.0),
    (52.4, 1290.0, 676.0),
    (56.6, 1281.0, 565.0),
    (58.0, 1281.0, 565.0),
    (60.1, 1520.0, 26.0),
    (64.0, 1520.0, 26.0),
];
const CLICKS: [f32; 4] = [10.0, 14.0, 56.8, 60.3];

/// The pointer of the first-run cut.
const CURSOR_RUN: &[(f32, f32, f32)] = &[
    (0.4, 1000.0, 700.0),
    (1.4, 520.0, 222.0),
    (3.7, 520.0, 222.0),
    (4.1, 640.0, 350.0),
    (9.0, 640.0, 350.0),
    (9.8, 680.0, 470.0),
    (11.0, 680.0, 470.0),
    (11.8, 1000.0, 700.0),
    (15.6, 1000.0, 700.0),
    (17.3, 470.0, 272.0),
    (18.5, 470.0, 272.0),
    (20.6, 1290.0, 676.0),
    (52.4, 1290.0, 676.0),
    (56.6, 1281.0, 565.0),
    (58.0, 1281.0, 565.0),
    (60.1, 1520.0, 26.0),
    (64.0, 1520.0, 26.0),
];
const CLICKS_RUN: [f32; 7] = [
    first_run::TITLE_CLICK,
    first_run::DESC_CLICK,
    first_run::GENERATE,
    17.5,
    20.6,
    56.8,
    60.3,
];

fn cursor(lb: f32, pl: &Plan) -> Svgr<'static> {
    let gate = (1.0 - prog(lb, 31.0, 31.4) + prog(lb, 52.0, 52.4)).clamp(0.0, 1.0);
    let vis = if pl.first_run {
        (prog(lb, 0.3, 0.6) * (1.0 - prog(lb, 11.2, 11.7))).max(prog(lb, 15.8, 16.3)) * gate
    } else {
        prog(lb, 5.5, 6.0) * gate
    };
    if vis <= 0.0 {
        return Svgr::empty();
    }
    let keys = pl.cursor;
    let mut pos = (keys[0].1, keys[0].2);
    for w in keys.windows(2) {
        if lb >= w[0].0 {
            let t = cubic_in_out(prog(lb, w[0].0, w[1].0));
            pos = (lerp(w[0].1, w[1].1, t), lerp(w[0].2, w[1].2, t));
        }
    }
    let rings: Vec<Svgr> = pl
        .clicks
        .iter()
        .filter_map(|c| {
            let t = prog(lb, *c, *c + 0.7);
            if t <= 0.0 || t >= 1.0 {
                return None;
            }
            Some(fframes::svgr!(
                <circle cx={pos.0} cy={pos.1} r={10.0 + expo_out(t) * 34.0} fill="none" stroke={ACCENT} stroke-width="3" opacity={1.0 - t} />
            ))
        })
        .collect();
    fframes::svgr!(
        <g opacity={vis}>
            {rings}
            <path d={format!("M{} {} l0 30 l8 -7 l6 14 l6 -3 l-6 -14 l11 -1 Z", pos.0, pos.1)} fill={BONE} stroke="#000000" stroke-width="2" stroke-linejoin="round" />
        </g>
    )
}
