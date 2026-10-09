//! The Studio demo, drawn vertically: canvas on top, timeline under it and the agent
//! chat filling the rest, the way a phone-sized window would stack them. One
//! continuous take: select the title, type the prompt, watch the agent retrieve the
//! code and edit it on the drop, preview, export.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use super::create::{self, spinner};
use crate::beat::*;
use crate::scenes::install::check;
use crate::shaders::SHADERS;
use crate::ui::*;

short_scene!(StudioScene, Some(super::STUDIO_AT), super::FAST_AT);

// Local beats (0 = start of the scene).
// 0..17 is the first run (`create.rs`), then the edit by selection.
const SELECT: f32 = 18.5;
pub const TYPE_FROM: f32 = 20.5;
const TYPE_TO: f32 = 26.5;
/// The prompt is sent.
pub const SEND: f32 = 28.5;
/// The soundtrack drops here: the agent's edit lands (global beat 48).
const EDIT: f32 = super::DROP - super::STUDIO_AT;
const PLAY_FROM: f32 = EDIT + 1.5;
const PLAY_TO: f32 = EDIT + 7.0;
const EXPORT: f32 = EDIT + 6.5;

pub const BLIP_BEATS: &[f32] = &[
    SELECT,
    SEND,
    SEND + 1.6,
    SEND + 3.6,
    EDIT,
    EDIT + 3.0,
    EDIT + 5.0,
];

const CODE_OPEN: &str = "  <text x=\"48\" y=\"200\" font-family=\"Archivo Black\"";
const CODE_TITLE: &str = "  font-size=\"64\" fill=\"#ece8e1\">Career Tech Day</text>";
const CODE_ADDED_1: &str = "+ <text x=\"48\" y=\"290\" font-size=\"56\" fill=\"#2f80ff\">";
const CODE_ADDED_2: &str = "+   Future of Tech</text>";

const SLOGAN: &str = "Future of Tech";
const PROMPT: &str = "Add our slogan: Future of Tech";

// The window, in canvas coordinates of the 1080x1920 video.
const WX: f32 = 40.0;
const WY: f32 = 330.0;
const WW: f32 = 1000.0;
const WH: f32 = 1320.0;
// The project frame inside the canvas area (16:9).
const FX: f32 = 180.0;
const FY: f32 = 408.0;
const FW: f32 = 720.0;
const FH: f32 = 405.0;
// Where the title sits, baseline left.
const TX: f32 = FX + 48.0;
const TY: f32 = FY + 200.0;

/// (start beat, tag, headline)
const CAPTIONS: &[(f32, &str, &str)] = &[
    (0.0, "01 / DESCRIBE", "Describe the video."),
    (create::GENERATE + 0.4, "02 / GENERATE", "Code, then video."),
    (17.3, "03 / SELECT", "Click any element."),
    (TYPE_FROM - 0.5, "04 / PROMPT", "Say what changes."),
    (SEND + 1.6, "05 / RETRIEVE", "It finds the code."),
    (EDIT, "06 / EDIT", "A real Rust diff."),
    (PLAY_FROM + 1.5, "07 / PREVIEW", "Rebuilt. Playing."),
    (EXPORT - 0.5, "08 / EXPORT", "Export an MP4."),
];

fn captions(lb: f32) -> Svgr<'static> {
    let items: Vec<Svgr> = CAPTIONS
        .iter()
        .enumerate()
        .filter_map(|(i, (start, tag, line))| {
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
                    <text x="80" y="188" font-family={MONO} font-weight="600" font-size="24" letter-spacing="4" fill={ACCENT}>{*tag}</text>
                    <text x="80" y="276" font-family={DISPLAY} font-size="66" letter-spacing="-2" fill={BONE}>{*line}</text>
                </g>
            ))
        })
        .collect();
    fframes::svgr!(<g>{items}</g>)
}

/// The mouse: (beat, x, y) waypoints, eased between.
const CURSOR: &[(f32, f32, f32)] = &[
    (0.4, 1010.0, 1300.0),
    (1.4, 420.0, 678.0),
    (3.7, 420.0, 678.0),
    (4.1, 520.0, 880.0),
    (9.0, 520.0, 880.0),
    (9.8, 540.0, 1122.0),
    (11.0, 540.0, 1122.0),
    (11.8, 1010.0, 1250.0),
    (17.0, 1010.0, 1100.0),
    (SELECT - 0.1, 470.0, 585.0),
    (SELECT + 0.7, 470.0, 585.0),
    (TYPE_FROM - 0.2, 640.0, 1608.0),
    (SEND - 1.5, 640.0, 1608.0),
    (SEND - 0.1, 968.0, 1600.0),
    (SEND + 2.0, 968.0, 1600.0),
];

fn cursor_at(lb: f32) -> (f32, f32) {
    let keys = CURSOR;
    if lb <= keys[0].0 {
        return (keys[0].1, keys[0].2);
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if lb <= b.0 {
            let t = cubic_in_out(prog(lb, a.0, b.0));
            return (lerp(a.1, b.1, t), lerp(a.2, b.2, t));
        }
    }
    let last = keys[keys.len() - 1];
    (last.1, last.2)
}

fn cursor(lb: f32) -> Svgr<'static> {
    let first = prog(lb, 0.3, 0.6) * (1.0 - prog(lb, 11.2, 11.7));
    let second = prog(lb, 16.9, 17.3) * (1.0 - prog(lb, SEND + 1.5, SEND + 2.1));
    let visible = first.max(second);
    if visible <= 0.0 {
        return Svgr::empty();
    }
    let (x, y) = cursor_at(lb);
    // a press dips the pointer, a ring spreads from the click
    let ring = |at: f32| {
        let t = (lb - at) / 1.2;
        if (0.0..1.0).contains(&t) {
            Some((expo_out(t) * 46.0, (1.0 - t) * 0.8))
        } else {
            None
        }
    };
    let rings: Vec<Svgr> = [
        create::TITLE_CLICK,
        create::DESC_CLICK,
        create::GENERATE,
        SELECT,
        SEND,
    ]
    .iter()
    .filter_map(|c| ring(*c))
    .map(|(r, o)| {
        fframes::svgr!(<circle cx={x} cy={y} r={r} fill="none" stroke={ACCENT} stroke-width="3" opacity={o} />)
    })
    .collect();
    fframes::svgr!(
        <g opacity={visible}>
            {rings}
            <path transform={format!("translate({x} {y})")} d="M0 0 L0 30 L8 23 L14 36 L19 33 L13 21 L24 21 Z"
                  fill={BONE} stroke={INK} stroke-width="2" stroke-linejoin="round" />
        </g>
    )
}

struct Step {
    name: &'static str,
    detail: &'static str,
    from: f32,
    to: f32,
}

const STEPS: [Step; 5] = [
    Step {
        name: "selection_context",
        detail: "Text 'Career Tech Day'",
        from: SEND + 0.3,
        to: SEND + 1.6,
    },
    Step {
        name: "source_lookup",
        detail: "src/title.rs:12",
        from: SEND + 1.6,
        to: SEND + 3.6,
    },
    Step {
        name: "edit",
        detail: "+2  -0 lines",
        from: SEND + 3.6,
        to: EDIT + 0.5,
    },
    Step {
        name: "build",
        detail: "1.2 s",
        from: EDIT + 0.5,
        to: EDIT + 3.0,
    },
    Step {
        name: "inspect",
        detail: "0 issues",
        from: EDIT + 3.0,
        to: EDIT + 5.0,
    },
];

fn steps(lb: f32) -> Svgr<'static> {
    let rows: Vec<Svgr> = STEPS
        .iter()
        .enumerate()
        .filter_map(|(i, step)| {
            let a = prog(lb, step.from, step.from + 0.3);
            if a <= 0.0 {
                return None;
            }
            let y = 1148.0 + i as f32 * 50.0;
            let done = lb >= step.to;
            let s = snap(lb - step.from);
            let icon = if done {
                check(84.0, y - 12.0, prog(lb, step.to, step.to + 0.3))
            } else {
                spinner(98.0, y - 5.0, lb)
            };
            Some(fframes::svgr!(
                <g opacity={a} transform={format!("translate({} 0)", (1.0 - s) * 40.0)}>
                    {icon}
                    <text x="130" y={y} font-family={MONO} font-weight="600" font-size="26" fill={BONE}>{step.name}</text>
                    <text x="1000" y={y} text-anchor="end" font-family={MONO} font-weight="500" font-size="22" fill={if done { "#6fcf8a" } else { GREY }}>{step.detail}</text>
                </g>
            ))
        })
        .collect();
    fframes::svgr!(<g>{rows}</g>)
}

/// The retrieved code and then the diff the agent applies.
fn diff(lb: f32) -> Svgr<'static> {
    let a = prog(lb, SEND + 2.0, SEND + 2.5);
    if a <= 0.0 {
        return Svgr::empty();
    }
    let y = 1388.0;
    let edited = lb >= EDIT;
    let s = snap(lb - (SEND + 2.0));
    let header = if edited {
        "src/title.rs   DIFF  +2 -0"
    } else {
        "src/title.rs   RETRIEVED"
    };
    let edge = if edited { "#6fcf8a" } else { ACCENT };
    let add = prog(lb, EDIT, EDIT + 0.2);
    fframes::svgr!(
        <g opacity={a} transform={format!("translate(0 {})", (1.0 - s) * 40.0)}>
            <rect x="70" y={y} width="940" height="170" rx="10" fill="#0b0b0b" stroke="#3d3a35" stroke-width="2" />
            <rect x="70" y={y} width="6" height="170" fill={edge} />
            <text x="100" y={y + 32.0} font-family={MONO} font-weight="600" font-size="19" letter-spacing="2" fill={edge}>{header}</text>
            <text x="100" y={y + 68.0} font-family={MONO} font-weight="500" font-size="22" fill={GREY}>{CODE_OPEN}</text>
            <text x="100" y={y + 100.0} font-family={MONO} font-weight="500" font-size="22" fill="#b9b4ab">{CODE_TITLE}</text>
            <text x="100" y={y + 130.0} font-family={MONO} font-weight="500" font-size="22" fill="#6fcf8a" opacity={add}>{CODE_ADDED_1}</text>
            <text x="100" y={y + 158.0} font-family={MONO} font-weight="500" font-size="22" fill="#6fcf8a" opacity={add}>{CODE_ADDED_2}</text>
        </g>
    )
}

impl Scene for StudioScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, mut frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let bg = SHADERS.contour.draw(
            &frame,
            ShaderUniforms::new()
                .float2("uWell", 0.5, 0.85)
                .float("uDepth", 0.5)
                .float("uDensity", 12.0)
                .float("uBright", 0.14 + (-(lb - EDIT).abs() * 0.8).exp() * 0.14)
                .color("uInk", Color::hex("#8a857c"))
                .color("uHot", Color::hex(ACCENT)),
        );

        // ---- the window
        let enter = snap(lb);
        let win_o = prog(lb, 0.0, 0.5);

        // ---- the title on the canvas, before and after the edit
        let size: f32 = 64.0;
        // the slogan the agent adds springs in from below on the drop
        let since = (lb - EDIT).max(0.0);
        let slogan_o = if lb >= EDIT {
            prog(since, 0.0, 0.2)
        } else {
            0.0
        };
        let slogan_dy = (1.0 - soft(since - 0.1)) * 120.0;
        let tw = measure(
            &mut frame,
            ctx,
            DISPLAY,
            size.round() as usize,
            400,
            false,
            "Career Tech Day",
        );
        let sel = snap(lb - SELECT);
        let selection = if lb >= SELECT {
            let bx = TX - 16.0;
            let by = TY - size * 0.92;
            let bw = tw + 32.0;
            let bh = size * 1.22;
            let handles: Vec<Svgr> = [(bx, by), (bx + bw, by), (bx, by + bh), (bx + bw, by + bh)]
                .iter()
                .map(|(hx, hy)| {
                    fframes::svgr!(<rect x={hx - 7.0} y={hy - 7.0} width="14" height="14" fill={BG} stroke={ACCENT} stroke-width="3" />)
                })
                .collect();
            fframes::svgr!(
                <g opacity={sel.min(1.0)}>
                    <rect x={bx} y={by} width={bw} height={bh} fill={ACCENT} fill-opacity="0.12" stroke={ACCENT} stroke-width="3" />
                    {handles}
                    <rect x={bx} y={by - 38.0} width="236" height="30" fill={ACCENT} />
                    <text x={bx + 12.0} y={by - 16.0} font-family={MONO} font-weight="600" font-size="17" letter-spacing="1.5" fill={INK}>"TEXT  /  title"</text>
                </g>
            )
        } else {
            Svgr::empty()
        };

        // ---- the first run fills the canvas and the timeline
        let reveal = prog(lb, create::REVEAL_FROM, create::REVEAL_TO);
        let named = if lb >= create::GENERATE {
            "vinuni-tech-day  /  fframes Studio"
        } else {
            "untitled  /  fframes Studio"
        };

        // ---- timeline
        let playing = prog(lb, PLAY_FROM, PLAY_TO);
        let head_x = WX + 40.0 + (0.08 + playing * 0.78) * (WW - 80.0);
        let secs = if lb >= PLAY_FROM { playing * 6.0 } else { 0.0 };
        let timecode_text = format!(
            "00:0{}.{:02}",
            secs.floor() as u32,
            ((secs.fract()) * 100.0) as u32
        );
        let clips: Vec<Svgr> = [
            (0.0, 0.46, "#2b2926", 906.0),
            (0.1, 0.62, "#14357a", 930.0),
            (0.0, 0.94, "#2b2926", 954.0),
        ]
        .iter()
        .map(|(a, b, fill, y)| {
            fframes::svgr!(
                <rect x={WX + 40.0 + a * (WW - 80.0)} y={*y} width={((b - a) * (WW - 80.0) * reveal).max(0.1)} height="16" rx="3" fill={*fill} />
            )
        })
        .collect();

        // ---- the chat input
        let typed_n = if lb < TYPE_FROM {
            0
        } else {
            (prog(lb, TYPE_FROM, TYPE_TO) * PROMPT.len() as f32) as usize
        };
        let sent = lb >= SEND;
        let shown: String = if sent {
            String::new()
        } else {
            PROMPT.chars().take(typed_n).collect()
        };
        let placeholder = shown.is_empty();
        let caret_on = !sent && (lb >= TYPE_FROM - 1.0) && ((lb * 2.0).fract() < 0.6);
        let caret_x = 106.0 + typed_n as f32 * 17.5;
        let input_hint = if sent {
            "Ask for another change..."
        } else {
            "Describe the change..."
        };
        let send_press = if (SEND..SEND + 0.4).contains(&lb) {
            0.9
        } else {
            1.0
        };
        let input_stroke = if lb >= TYPE_FROM && !sent {
            ACCENT
        } else {
            "#3d3a35"
        };

        // the selection chip, until the prompt is sent
        let chip = if lb >= SELECT + 1.0 && !sent {
            let s = snap(lb - SELECT - 1.0);
            fframes::svgr!(
                <g opacity={s.min(1.0)}>
                    <rect x="70" y="1494" width="480" height="48" rx="24" fill="#14357a" fill-opacity="0.6" stroke={ACCENT} stroke-width="2" />
                    <rect x="94" y="1510" width="16" height="16" fill={ACCENT} />
                    <text x="124" y="1525" font-family={MONO} font-weight="600" font-size="20" fill={BONE}>"SELECTED  Text 'Career Tech Day'"</text>
                </g>
            )
        } else {
            Svgr::empty()
        };

        // the agent's greeting, until it starts working
        let hello = if (create::CHAT_END..SEND + 0.5).contains(&lb) {
            let s = snap(lb - create::CHAT_END);
            fframes::svgr!(
                <g opacity={prog(lb, create::CHAT_END, create::CHAT_END + 0.3)} transform={format!("translate(0 {})", (1.0 - s) * 30.0)}>
                    <rect x="70" y="1030" width="720" height="118" rx="22" fill="#1d1b19" />
                    <text x="102" y="1078" font-family={MONO} font-weight="500" font-size="26" fill="#b9b4ab">"Select anything on the canvas,"</text>
                    <text x="102" y="1118" font-family={MONO} font-weight="500" font-size="26" fill="#b9b4ab">"then tell me what to change."</text>
                </g>
            )
        } else {
            Svgr::empty()
        };

        // the user's bubble after sending
        let bubble = if sent {
            let s = snap(lb - SEND);
            fframes::svgr!(
                <g opacity={s.min(1.0)} transform={format!("translate(0 {})", (1.0 - s) * 40.0)}>
                    <rect x="400" y="1022" width="610" height="76" rx="22" fill={ACCENT} />
                    <text x="430" y="1070" font-family={MONO} font-weight="600" font-size="27" fill={INK}>{PROMPT}</text>
                </g>
            )
        } else {
            Svgr::empty()
        };

        // ---- export confirmation over the canvas
        let ex = prog(lb, EXPORT, EXPORT + 0.2);
        let export = if ex > 0.0 {
            let s = snap(lb - EXPORT);
            fframes::svgr!(
                <g opacity={ex} transform={format!("translate(540 {}) scale({}) translate(-540 -{})", 610.0, 0.85 + 0.15 * s, 610.0)}>
                    <rect x="230" y="548" width="620" height="124" rx="16" fill={BG} stroke={ACCENT} stroke-width="3" />
                    {check(262.0, 604.0, prog(lb, EXPORT + 0.2, EXPORT + 0.8))}
                    <text x="324" y="596" font-family={DISPLAY} font-size="34" fill={BONE}>"Exported"</text>
                    <text x="324" y="642" font-family={MONO} font-weight="500" font-size="24" fill={GREY}>"vinuni-tech-day.mp4  /  1080x1920"</text>
                </g>
            )
        } else {
            Svgr::empty()
        };

        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                {captions(lb)}
                <g opacity={win_o} transform={format!("translate(0 {})", (1.0 - enter) * 70.0)}>
                    <rect x={WX} y={WY} width={WW} height={WH} rx="20" fill="#121110" stroke="#3d3a35" stroke-width="2" />
                    // title bar
                    <circle cx="76" cy="355" r="7" fill="#3d3a35" />
                    <circle cx="100" cy="355" r="7" fill="#3d3a35" />
                    <circle cx="124" cy="355" r="7" fill="#3d3a35" />
                    <text x="540" y="362" text-anchor="middle" font-family={MONO} font-weight="500" font-size="20" letter-spacing="1" fill={GREY}>{named}</text>
                    <text x="1006" y="362" text-anchor="end" font-family={MONO} font-weight="600" font-size="17" letter-spacing="2" fill={ACCENT}>"CLAUDE CODE"</text>
                    <rect x={WX} y="380" width={WW} height="1" fill="#2b2926" />
                    // canvas
                    <rect x={WX + 1.0} y="381" width={WW - 2.0} height="460" fill="#0a0a0a" />
                    <clipPath id="frame-clip"><rect x={FX} y={FY} width={FW} height={FH} /></clipPath>
                    <rect x={FX} y={FY} width={FW} height={FH} fill="#0d1424" opacity={reveal} />
                    <g opacity={1.0 - reveal}>
                        <rect x={FX} y={FY} width={FW} height={FH} fill="none" stroke="#3d3a35" stroke-width="2" stroke-dasharray="14 10" />
                        <text x="540" y={FY + FH / 2.0 + 8.0} text-anchor="middle" font-family={MONO} font-weight="600" font-size="22" letter-spacing="5" fill={GREY}>"NO VIDEO YET"</text>
                    </g>
                    <g clip-path="url(#frame-clip)" opacity={reveal}>
                        <circle cx="850" cy="760" r="120" fill="none" stroke="#14357a" stroke-width="3" />
                        <circle cx="850" cy="760" r="70" fill="none" stroke="#14357a" stroke-width="3" />
                        <text x={FX + 48.0} y={FY + 66.0} font-family={MONO} font-weight="500" font-size="18" letter-spacing="4" fill={GREY}>"VINUNI  ·  2026"</text>
                        <text x={TX} y={TY} font-family={DISPLAY} font-size={size} letter-spacing="-1" fill={BONE}>"Career Tech Day"</text>
                        <g opacity={slogan_o} transform={format!("translate(0 {slogan_dy})")}>
                            <text x={TX} y={TY + 90.0} font-family={DISPLAY} font-size="56" letter-spacing="-1" fill={ACCENT}>{SLOGAN}</text>
                        </g>
                        <rect x={FX + 48.0} y={FY + 350.0} width="150" height="8" fill={ACCENT} />
                    </g>
                    <rect x={FX} y={FY} width={FW} height={FH} fill="none" stroke="#2b2926" stroke-width="2" opacity={reveal} />
                    {selection}
                    {export}
                    // timeline
                    <rect x={WX} y="841" width={WW} height="1" fill="#2b2926" />
                    <path d={format!("M{} 872 l0 28 l24 -14 Z", WX + 40.0)} fill={BONE} />
                    <text x="130" y="894" font-family={MONO} font-weight="600" font-size="22" fill={BONE}>{timecode_text}</text>
                    <text x="1006" y="894" text-anchor="end" font-family={MONO} font-weight="500" font-size="18" letter-spacing="2" fill={GREY}>"6.0 s  /  60 fps"</text>
                    {clips}
                    <rect x={head_x} y="896" width="3" height="80" fill={ACCENT} opacity={reveal} />
                    <rect x={WX} y="984" width={WW} height="1" fill="#2b2926" />
                    // agent chat
                    <text x="80" y="1018" font-family={MONO} font-weight="600" font-size="18" letter-spacing="4" fill={ACCENT}>"AGENT"</text>
                    {create::log(lb)}
                    {hello}
                    {bubble}
                    {steps(lb)}
                    {diff(lb)}
                    {chip}
                    <rect x="70" y="1572" width="940" height="72" rx="16" fill="#181715" stroke={input_stroke} stroke-width="2" />
                    <text x="106" y="1619" font-family={MONO} font-weight="500" font-size="29" fill={BONE} opacity={if placeholder { 0.0 } else { 1.0 }}>{shown}</text>
                    <text x="106" y="1619" font-family={MONO} font-weight="500" font-size="29" fill={GREY} opacity={if placeholder && !caret_on { 1.0 } else { 0.0 }}>{input_hint}</text>
                    <rect x={caret_x} y="1592" width="3" height="34" fill={ACCENT} opacity={if caret_on { 1.0 } else { 0.0 }} />
                    <g transform={format!("translate(968 1608) scale({send_press}) translate(-968 -1608)")}>
                        <circle cx="968" cy="1608" r="27" fill={ACCENT} />
                        <path d="M956 1608 H980 M971 1597 L982 1608 L971 1619" fill="none" stroke={INK} stroke-width="4" stroke-linecap="round" stroke-linejoin="round" />
                    </g>
                    {create::modal(lb)}
                </g>
                {cursor(lb)}
            </g>
        )
    }
}
