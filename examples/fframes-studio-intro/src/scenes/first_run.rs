//! The first run of the desktop app, in window coordinates: a "New video" form with a
//! title and a description, one click, and the agent writes the code, builds it and
//! renders the first preview into the empty canvas. Drawn over the Studio window.

use fframes::Svgr;

use super::install::check;
use crate::beat::*;
use crate::ui::*;

pub const TITLE: &str = "VinUni Career Tech Day 2026";
const DESC: [&str; 2] = [
    "A career fair for VinUni students.",
    "Dark navy, big title, modern look.",
];

// Scene beats.
pub const TITLE_CLICK: f32 = 1.6;
const TITLE_FROM: f32 = 1.8;
const TITLE_TO: f32 = 3.9;
pub const DESC_CLICK: f32 = 4.2;
pub const DESC_FROM: f32 = 4.5;
const DESC_TO: f32 = 9.0;
/// The Generate button is pressed.
pub const GENERATE: f32 = 10.0;
/// The agent is done and the canvas fills in.
pub const REVEAL_FROM: f32 = 15.4;
pub const REVEAL_TO: f32 = 16.2;
/// The agent's first-run log leaves the chat.
pub const CHAT_END: f32 = 17.0;

/// Clicks, steps and the render, on the scene's beats.
pub const BLIP_BEATS: &[f32] = &[GENERATE, 11.6, 13.6, 14.8, 16.2];

// The form, in window coordinates.
const MX: f32 = 330.0;
const MY: f32 = 72.0;
const MW: f32 = 700.0;
const MH: f32 = 444.0;
const FX: f32 = 366.0;
const FW: f32 = 628.0;

const STEPS: [(&str, &str, f32, f32); 4] = [
    ("plan_scene", "Career Tech Day", GENERATE + 0.4, 11.6),
    ("write_code", "intro.rs", 11.6, 13.6),
    ("build", "1.4 s", 13.6, 14.8),
    ("render_preview", "6.0 s", 14.8, 16.2),
];

const CODE: [&str; 5] = [
    "<text x=\"88\" y=\"464\"",
    "  font-family=\"Archivo Black\"",
    "  font-size=\"64\" fill=\"#ece8e1\">",
    "  Career Tech Day",
    "</text>",
];

fn spinner(cx: f32, cy: f32, lb: f32) -> Svgr<'static> {
    let deg = lb * 220.0;
    fframes::svgr!(
        <g transform={format!("rotate({deg} {cx} {cy})")}>
            <circle cx={cx} cy={cy} r="9" fill="none" stroke={DIM} stroke-width="3" />
            <circle cx={cx} cy={cy} r="9" fill="none" stroke={ACCENT} stroke-width="3" stroke-dasharray="18 60" stroke-linecap="round" />
        </g>
    )
}

fn blink(lb: f32) -> bool {
    (lb * 2.0).fract() < 0.6
}

/// The "New video" form with a title, a description and a Generate button.
pub fn modal(lb: f32) -> Svgr<'static> {
    let a = prog(lb, 0.0, 0.4) * (1.0 - prog(lb, GENERATE + 0.1, GENERATE + 0.7));
    if a <= 0.0 {
        return Svgr::empty();
    }
    let scale = 0.95 + 0.05 * snap(lb) - 0.03 * prog(lb, GENERATE + 0.1, GENERATE + 0.7);

    let n_title = (prog(lb, TITLE_FROM, TITLE_TO) * TITLE.len() as f32) as usize;
    let title: String = TITLE.chars().take(n_title).collect();
    let n_desc = (prog(lb, DESC_FROM, DESC_TO) * (DESC[0].len() + DESC[1].len()) as f32) as usize;
    let first = DESC[0].len();
    let line_a: String = DESC[0].chars().take(n_desc).collect();
    let line_b: String = DESC[1].chars().take(n_desc.saturating_sub(first)).collect();

    let title_active = (TITLE_CLICK..DESC_CLICK - 0.2).contains(&lb);
    let desc_active = (DESC_CLICK..GENERATE).contains(&lb);
    let stroke = |on: bool| if on { ACCENT } else { "#3d3a35" };
    let title_hint = if title.is_empty() && !title_active {
        1.0
    } else {
        0.0
    };
    let desc_hint = if n_desc == 0 && !desc_active {
        1.0
    } else {
        0.0
    };

    let caret_title = title_active && blink(lb);
    let caret_title_x = FX + 22.0 + n_title as f32 * 15.6;
    let caret_desc = desc_active && blink(lb);
    let (caret_desc_x, caret_desc_y) = if n_desc <= first {
        (FX + 22.0 + n_desc as f32 * 13.2, 312.0)
    } else {
        (FX + 22.0 + (n_desc - first) as f32 * 13.2, 348.0)
    };
    let press = if (GENERATE..GENERATE + 0.35).contains(&lb) {
        0.97
    } else {
        1.0
    };
    let cy = MY + MH / 2.0;
    fframes::svgr!(
        <g opacity={a}>
            <rect x="1" y="49" width="1598" height="690" fill={BG} fill-opacity="0.78" />
            <g transform={format!("translate(680 {cy}) scale({scale}) translate(-680 -{cy})")}>
                <rect x={MX} y={MY} width={MW} height={MH} rx="16" fill="#181715" stroke="#3d3a35" stroke-width="2" />
                <text x={FX} y="106" font-family={MONO} font-weight="600" font-size="15" letter-spacing="4" fill={ACCENT}>"FIRST VIDEO"</text>
                <text x={FX} y="150" font-family={DISPLAY} font-size="38" letter-spacing="-1" fill={BONE}>"New video"</text>

                <text x={FX} y="190" font-family={MONO} font-weight="600" font-size="14" letter-spacing="4" fill={GREY}>"TITLE"</text>
                <rect x={FX} y="200" width={FW} height="52" rx="10" fill="#0f0e0d" stroke={stroke(title_active)} stroke-width="2" />
                <text x={FX + 22.0} y="236" font-family={MONO} font-weight="500" font-size="26" fill={BONE}>{title}</text>
                <text x={FX + 22.0} y="236" font-family={MONO} font-weight="500" font-size="26" fill={GREY} opacity={title_hint}>"Name your video"</text>
                <rect x={caret_title_x} y="212" width="3" height="28" fill={ACCENT} opacity={if caret_title { 1.0 } else { 0.0 }} />

                <text x={FX} y="286" font-family={MONO} font-weight="600" font-size="14" letter-spacing="4" fill={GREY}>"DESCRIPTION"</text>
                <rect x={FX} y="296" width={FW} height="104" rx="10" fill="#0f0e0d" stroke={stroke(desc_active)} stroke-width="2" />
                <text x={FX + 22.0} y="334" font-family={MONO} font-weight="500" font-size="22" fill={BONE}>{line_a}</text>
                <text x={FX + 22.0} y="370" font-family={MONO} font-weight="500" font-size="22" fill={BONE}>{line_b}</text>
                <text x={FX + 22.0} y="334" font-family={MONO} font-weight="500" font-size="22" fill={GREY} opacity={desc_hint}>"What should it look like?"</text>
                <rect x={caret_desc_x} y={caret_desc_y - 6.0} width="3" height="28" fill={ACCENT} opacity={if caret_desc { 1.0 } else { 0.0 }} />

                <text x={FX} y="430" font-family={MONO} font-weight="500" font-size="16" letter-spacing="2" fill={GREY}>"Your agent writes the Rust. You get a video."</text>
                <g transform={format!("translate(680 470) scale({press}) translate(-680 -470)")}>
                    <rect x={FX} y="442" width={FW} height="56" rx="12" fill={ACCENT} />
                    <text x="680" y="479" text-anchor="middle" font-family={DISPLAY} font-size="26" letter-spacing="-1" fill={INK}>"Generate video"</text>
                </g>
            </g>
        </g>
    )
}

/// The agent's log while it writes, builds and renders the first video.
pub fn log(lb: f32) -> Svgr<'static> {
    let from = GENERATE + 0.4;
    let o = prog(lb, from, from + 0.3) * (1.0 - prog(lb, CHAT_END - 0.4, CHAT_END));
    if o <= 0.0 {
        return Svgr::empty();
    }
    let rows: Vec<Svgr> = STEPS
        .iter()
        .enumerate()
        .filter_map(|(i, (name, detail, a, b))| {
            let appear = prog(lb, *a, *a + 0.3);
            if appear <= 0.0 {
                return None;
            }
            let y = 140.0 + i as f32 * 44.0;
            let done = lb >= *b;
            let s = snap(lb - a);
            let icon = if done {
                check(1134.0, y - 14.0, prog(lb, *b, *b + 0.3))
            } else {
                spinner(1142.0, y - 7.0, lb)
            };
            Some(fframes::svgr!(
                <g opacity={appear} transform={format!("translate({} 0)", (1.0 - s) * 24.0)}>
                    {icon}
                    <text x="1164" y={y} font-family={MONO} font-weight="600" font-size="19" fill={BONE}>{*name}</text>
                    <text x="1580" y={y} text-anchor="end" font-family={MONO} font-weight="500" font-size="17" fill={if done { "#6fcf8a" } else { GREY }}>{*detail}</text>
                </g>
            ))
        })
        .collect();

    // the generated code, typed out
    let total: usize = CODE.iter().map(|l| l.len()).sum();
    let mut left = (prog(lb, 11.6, 13.4) * total as f32) as usize;
    let lines: Vec<String> = CODE
        .iter()
        .map(|l| {
            let n = left.min(l.len());
            left -= n;
            l.chars().take(n).collect()
        })
        .collect();
    let written = lb >= 13.6;
    let edge = if written { "#6fcf8a" } else { ACCENT };
    let header = if written {
        "src/scenes/intro.rs  WRITTEN"
    } else {
        "src/scenes/intro.rs  WRITING"
    };
    let card = prog(lb, 11.5, 11.9);
    let y = 336.0;
    fframes::svgr!(
        <g opacity={o}>
            {rows}
            <g opacity={card}>
                <rect x="1120" y={y} width="460" height="196" rx="8" fill="#0b0b0b" stroke="#3d3a35" stroke-width="2" />
                <rect x="1120" y={y} width="5" height="196" fill={edge} />
                <text x="1144" y={y + 30.0} font-family={MONO} font-weight="600" font-size="14" letter-spacing="2" fill={edge}>{header}</text>
                <text x="1144" y={y + 64.0} font-family={MONO} font-weight="500" font-size="18" fill="#b9b4ab">{lines[0].clone()}</text>
                <text x="1144" y={y + 94.0} font-family={MONO} font-weight="500" font-size="18" fill="#b9b4ab">{lines[1].clone()}</text>
                <text x="1144" y={y + 124.0} font-family={MONO} font-weight="500" font-size="18" fill="#b9b4ab">{lines[2].clone()}</text>
                <text x="1144" y={y + 154.0} font-family={MONO} font-weight="500" font-size="18" fill="#b9b4ab">{lines[3].clone()}</text>
                <text x="1144" y={y + 184.0} font-family={MONO} font-weight="500" font-size="18" fill="#b9b4ab">{lines[4].clone()}</text>
            </g>
        </g>
    )
}
