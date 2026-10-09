//! The first run of the desktop app: a title and a description in a "New video"
//! form, one click, and the agent writes the code, builds it and renders the first
//! preview into the empty canvas. Drawn over the Studio window of `studio.rs`.

use fframes::Svgr;

use crate::beat::*;
use crate::scenes::install::check;
use crate::ui::*;

pub const TITLE: &str = "VinUni Career Tech Day 2026";
const DESC: [&str; 2] = [
    "A career fair for VinUni students.",
    "Dark navy, big title, modern look.",
];

// Local beats of the Studio scene.
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

pub const BLIP_BEATS: &[f32] = &[GENERATE, 11.6, 13.6, 14.8, 16.2];

// Field geometry, in video pixels.
const MX: f32 = 100.0;
const MY: f32 = 440.0;
const MW: f32 = 880.0;
const MH: f32 = 760.0;
const FIELD_X: f32 = 140.0;
const FIELD_W: f32 = 800.0;

const STEPS: [(&str, &str, f32, f32); 4] = [
    ("plan_scene", "Career Tech Day", GENERATE + 0.4, 11.6),
    ("write_code", "src/title.rs", 11.6, 13.6),
    ("build", "1.4 s", 13.6, 14.8),
    ("render_preview", "6.0 s  /  60 fps", 14.8, 16.2),
];

const CODE: [&str; 5] = [
    "<text x=\"48\" y=\"200\"",
    "  font-family=\"Archivo Black\"",
    "  font-size=\"64\" fill=\"#ece8e1\">",
    "  Career Tech Day",
    "</text>",
];

pub fn spinner(cx: f32, cy: f32, lb: f32) -> Svgr<'static> {
    let deg = lb * 220.0;
    fframes::svgr!(
        <g transform={format!("rotate({deg} {cx} {cy})")}>
            <circle cx={cx} cy={cy} r="12" fill="none" stroke={DIM} stroke-width="4" />
            <circle cx={cx} cy={cy} r="12" fill="none" stroke={ACCENT} stroke-width="4" stroke-dasharray="26 80" stroke-linecap="round" />
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
    let caret_title_x = FIELD_X + 38.0 + n_title as f32 * 19.2;
    let caret_desc = desc_active && blink(lb);
    let (caret_desc_x, caret_desc_y) = if n_desc <= first {
        (FIELD_X + 38.0 + n_desc as f32 * 16.8, 818.0)
    } else {
        (FIELD_X + 38.0 + (n_desc - first) as f32 * 16.8, 862.0)
    };

    let press = if (GENERATE..GENERATE + 0.35).contains(&lb) {
        0.96
    } else {
        1.0
    };
    let cy = MY + MH / 2.0;
    fframes::svgr!(
        <g opacity={a}>
            <rect x="41" y="381" width="998" height="1268" fill={BG} fill-opacity="0.78" />
            <g transform={format!("translate(540 {cy}) scale({scale}) translate(-540 -{cy})")}>
                <rect x={MX} y={MY} width={MW} height={MH} rx="20" fill="#181715" stroke="#3d3a35" stroke-width="2" />
                <text x={FIELD_X} y="496" font-family={MONO} font-weight="600" font-size="18" letter-spacing="4" fill={ACCENT}>"FIRST VIDEO"</text>
                <text x={FIELD_X} y="566" font-family={DISPLAY} font-size="48" letter-spacing="-1" fill={BONE}>"New video"</text>

                <text x={FIELD_X} y="626" font-family={MONO} font-weight="600" font-size="17" letter-spacing="4" fill={GREY}>"TITLE"</text>
                <rect x={FIELD_X} y="640" width={FIELD_W} height="76" rx="12" fill="#0f0e0d" stroke={stroke(title_active)} stroke-width="2" />
                <text x={FIELD_X + 38.0} y="690" font-family={MONO} font-weight="500" font-size="32" fill={BONE}>{title}</text>
                <text x={FIELD_X + 38.0} y="690" font-family={MONO} font-weight="500" font-size="32" fill={GREY} opacity={title_hint}>"Name your video"</text>
                <rect x={caret_title_x} y="658" width="3" height="40" fill={ACCENT} opacity={if caret_title { 1.0 } else { 0.0 }} />

                <text x={FIELD_X} y="772" font-family={MONO} font-weight="600" font-size="17" letter-spacing="4" fill={GREY}>"DESCRIPTION"</text>
                <rect x={FIELD_X} y="786" width={FIELD_W} height="220" rx="12" fill="#0f0e0d" stroke={stroke(desc_active)} stroke-width="2" />
                <text x={FIELD_X + 38.0} y="840" font-family={MONO} font-weight="500" font-size="28" fill={BONE}>{line_a}</text>
                <text x={FIELD_X + 38.0} y="884" font-family={MONO} font-weight="500" font-size="28" fill={BONE}>{line_b}</text>
                <text x={FIELD_X + 38.0} y="840" font-family={MONO} font-weight="500" font-size="28" fill={GREY} opacity={desc_hint}>"What should it look like?"</text>
                <rect x={caret_desc_x} y={caret_desc_y - 8.0} width="3" height="36" fill={ACCENT} opacity={if caret_desc { 1.0 } else { 0.0 }} />

                <text x={FIELD_X} y="1050" font-family={MONO} font-weight="500" font-size="19" letter-spacing="2" fill={GREY}>"Your agent writes the Rust. You get a video."</text>
                <g transform={format!("translate(540 1122) scale({press}) translate(-540 -1122)")}>
                    <rect x={FIELD_X} y="1080" width={FIELD_W} height="84" rx="14" fill={ACCENT} />
                    <text x="540" y="1136" text-anchor="middle" font-family={DISPLAY} font-size="36" letter-spacing="-1" fill={INK}>"Generate video"</text>
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
            let y = 1066.0 + i as f32 * 50.0;
            let done = lb >= *b;
            let s = snap(lb - a);
            let icon = if done {
                check(84.0, y - 12.0, prog(lb, *b, *b + 0.3))
            } else {
                spinner(98.0, y - 5.0, lb)
            };
            Some(fframes::svgr!(
                <g opacity={appear} transform={format!("translate({} 0)", (1.0 - s) * 40.0)}>
                    {icon}
                    <text x="130" y={y} font-family={MONO} font-weight="600" font-size="26" fill={BONE}>{*name}</text>
                    <text x="1000" y={y} text-anchor="end" font-family={MONO} font-weight="500" font-size="22" fill={if done { "#6fcf8a" } else { GREY }}>{*detail}</text>
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
        "src/title.rs   WRITTEN"
    } else {
        "src/title.rs   WRITING"
    };
    let card = prog(lb, 11.5, 11.9);
    let y = 1290.0;
    fframes::svgr!(
        <g opacity={o}>
            {rows}
            <g opacity={card}>
                <rect x="70" y={y} width="940" height="250" rx="10" fill="#0b0b0b" stroke="#3d3a35" stroke-width="2" />
                <rect x="70" y={y} width="6" height="250" fill={edge} />
                <text x="100" y={y + 36.0} font-family={MONO} font-weight="600" font-size="19" letter-spacing="2" fill={edge}>{header}</text>
                <text x="100" y={y + 80.0} font-family={MONO} font-weight="500" font-size="22" fill="#b9b4ab">{lines[0].clone()}</text>
                <text x="100" y={y + 114.0} font-family={MONO} font-weight="500" font-size="22" fill="#b9b4ab">{lines[1].clone()}</text>
                <text x="100" y={y + 148.0} font-family={MONO} font-weight="500" font-size="22" fill="#b9b4ab">{lines[2].clone()}</text>
                <text x="100" y={y + 182.0} font-family={MONO} font-weight="500" font-size="22" fill="#b9b4ab">{lines[3].clone()}</text>
                <text x="100" y={y + 216.0} font-family={MONO} font-weight="500" font-size="22" fill="#b9b4ab">{lines[4].clone()}</text>
            </g>
        </g>
    )
}
