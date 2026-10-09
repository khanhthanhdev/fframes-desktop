//! After the demo: how Studio works, the race against Remotion and the
//! reasons behind the speed. One implementation draws both the 16:9 and the
//! 9:16 cut; `p` selects the layout.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use crate::beat::*;
use crate::facts::*;
use crate::shaders::SHADERS;
use crate::ui::*;

beat_scene!(HowScene, Some(112.0), Some(144.0));
beat_scene!(FastScene, Some(144.0), Some(176.0));
beat_scene!(WhyScene, Some(176.0), Some(198.0));

/// The reasons were laid out for 32 beats; the wall of squares takes the last ones,
/// so the 16:9 cut plays them faster.
const WHY_SPEED: f32 = 32.0 / 22.0;
/// Beats (global grid) of the reason changes of the 16:9 cut, for the soundtrack.
pub const WHY_BLIP_BEATS: &[f32] = &[178.1, 182.9, 187.7, 192.5];

/// Beats (global grid) of the stage and reason changes, for the soundtrack.
pub const BLIP_BEATS: &[f32] = &[
    114.0, 120.0, 126.0, 132.0, 138.0, 179.0, 186.0, 193.0, 200.0,
];

/// The grid floor behind a story scene.
fn grid_bg(frame: &Frame, p: bool, bright: f32) -> Svgr<'static> {
    let g = SHADERS.grid.draw(
        frame,
        ShaderUniforms::new()
            .float("uSpeed", 1.0)
            .float("uHorizon", if p { 0.4 } else { 0.55 })
            .float("uBright", bright)
            .color("uInk", Color::hex("#6f6a63"))
            .color("uHot", Color::hex(ACCENT)),
    );
    let (w, h) = size(p);
    fframes::svgr!(<image href={g.href()} x="0" y="0" width={w} height={h} />)
}

fn size(p: bool) -> (f32, f32) {
    if p {
        (1080.0, 1920.0)
    } else {
        (1920.0, 1080.0)
    }
}

impl Scene for HowScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }
    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let bg = grid_bg(&frame, false, 0.2);
        fframes::svgr!(<g>{bg}{how(lb, false)}</g>)
    }
}

impl Scene for FastScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }
    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        fast_scene(&frame, Self::lb(&frame), false)
    }
}

impl Scene for WhyScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }
    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let bg = grid_bg(&frame, false, 0.28);
        fframes::svgr!(<g>{bg}{why(lb * WHY_SPEED, false)}</g>)
    }
}

/// The race scene of the 9:16 cut and the 16:9 cut: result and table are
/// full-screen, the race sits on the grid.
pub fn fast_scene<'a>(frame: &Frame, lb: f32, p: bool) -> Svgr<'a> {
    if (RESULT_AT..TABLE_AT).contains(&lb) {
        return result(lb - RESULT_AT, p);
    }
    let bg = grid_bg(frame, p, 0.35);
    let body = if lb < RESULT_AT {
        race(lb, p)
    } else {
        table(lb - TABLE_AT, p)
    };
    fframes::svgr!(<g>{bg}{body}</g>)
}

// ---------------------------------------------------------------------------
// how it works

const STAGES: [(&str, &str, &str, [&str; 2]); 5] = [
    (
        "SELECT",
        "CANVAS",
        "Click the canvas",
        [
            "The element's id and source",
            "anchor travel with your prompt.",
        ],
    ),
    (
        "GROUND",
        "TASK PACKET",
        "Freeze the context",
        [
            "Scope, frame time and the exact",
            "source lines become a task.",
        ],
    ),
    (
        "EDIT",
        "DRAFT",
        "The agent edits Rust",
        [
            "Codex or Claude Code writes the",
            "change to a draft of the project.",
        ],
    ),
    (
        "BUILD",
        "CARGO",
        "Cargo builds a worker",
        [
            "An immutable binary with your",
            "video, fframes and a bridge.",
        ],
    ),
    (
        "PREVIEW",
        "WORKER",
        "Frames stream back",
        [
            "The last good build stays on screen",
            "until the new one is ready.",
        ],
    ),
];

/// Beat the first stage starts and how long each lasts.
const STAGE_AT: f32 = 2.0;
const STAGE_LEN: f32 = 6.0;

pub fn how(lb: f32, p: bool) -> Svgr<'static> {
    let active = (((lb - STAGE_AT) / STAGE_LEN).floor().max(0.0) as usize).min(4);
    let title = if p {
        fframes::svgr!(
            <g>
                {Slam::new(80.0, 300.0, "HOW IT", DISPLAY, 130.0, BONE).draw(lb, -140.0, 0.0)}
                {Slam::new(80.0, 440.0, "WORKS.", DISPLAY, 130.0, ACCENT).draw(lb - 0.5, -140.0, 0.0)}
            </g>
        )
    } else {
        fframes::svgr!(
            <g>
                {Slam::new(150.0, 250.0, "HOW IT", DISPLAY, 120.0, BONE).draw(lb, -140.0, 0.0)}
                {Slam::new(650.0, 250.0, "WORKS.", DISPLAY, 120.0, ACCENT).draw(lb - 0.5, -140.0, 0.0)}
            </g>
        )
    };

    let nodes: Vec<Svgr> = STAGES
        .iter()
        .enumerate()
        .map(|(i, (name, sub, _, _))| {
            let at = 1.0 + i as f32 * 0.3;
            let s = snap(lb - at);
            let lit = i <= active && lb >= STAGE_AT;
            let hot = i == active && lb >= STAGE_AT;
            let (x, y, w, h) = if p {
                (80.0, 540.0 + i as f32 * 118.0, 920.0, 102.0)
            } else {
                (150.0 + i as f32 * 334.0, 400.0, 290.0, 120.0)
            };
            let (nx, ny) = if p { (x + 36.0, y + 62.0) } else { (x + w / 2.0, y + 58.0) };
            let anchor = if p { "start" } else { "middle" };
            let (sx, sy) = if p { (x + w - 36.0, y + 60.0) } else { (x + w / 2.0, y + 92.0) };
            let sub_anchor = if p { "end" } else { "middle" };
            let off = (1.0 - s) * 40.0;
            let tr = if p { format!("translate({off} 0)") } else { format!("translate(0 {off})") };
            fframes::svgr!(
                <g opacity={prog(lb, at, at + 0.1)} transform={tr}>
                    <rect x={x} y={y} width={w} height={h} fill={if hot { ACCENT } else { "#121110" }} stroke={if lit { ACCENT } else { "#3d3a35" }} stroke-width="2" />
                    <text x={nx} y={ny} text-anchor={anchor} font-family={DISPLAY} font-size="44" fill={if hot { INK } else if lit { BONE } else { GREY }}>{*name}</text>
                    <text x={sx} y={sy} text-anchor={sub_anchor} font-family={MONO} font-weight="500" font-size="17" letter-spacing="2" fill={if hot { INK } else { GREY }}>{*sub}</text>
                </g>
            )
        })
        .collect();

    // the detail of the current stage
    let details: Vec<Svgr> = STAGES
        .iter()
        .enumerate()
        .filter_map(|(i, (name, _, head, lines))| {
            let start = STAGE_AT + i as f32 * STAGE_LEN;
            let end = start + STAGE_LEN;
            let appear = prog(lb, start, start + 0.4);
            let leave = if i == 4 { 0.0 } else { prog(lb, end - 0.4, end) };
            let o = appear * (1.0 - leave);
            if o <= 0.0 {
                return None;
            }
            let dy = (1.0 - snap(lb - start)) * 40.0;
            let (x, y) = if p { (80.0, 1250.0) } else { (150.0, 640.0) };
            let (hs, ds) = if p { (60.0, 30.0) } else { (84.0, 32.0) };
            let num = format!("{:02} / {name}", i + 1);
            Some(fframes::svgr!(
                <g opacity={o} transform={format!("translate(0 {dy})")}>
                    <text x={x} y={y} font-family={MONO} font-weight="600" font-size="24" letter-spacing="4" fill={ACCENT}>{num}</text>
                    <text x={x} y={y + hs + 20.0} font-family={DISPLAY} font-size={hs} letter-spacing="-2" fill={BONE}>{*head}</text>
                    <text x={x} y={y + hs + 80.0} font-family={MONO} font-weight="500" font-size={ds} fill="#b9b4ab">{lines[0]}</text>
                    <text x={x} y={y + hs + 80.0 + ds * 1.45} font-family={MONO} font-weight="500" font-size={ds} fill="#b9b4ab">{lines[1]}</text>
                </g>
            ))
        })
        .collect();

    let (nx, ny) = if p { (80.0, 1760.0) } else { (150.0, 960.0) };
    let note_o = prog(lb, 3.0, 3.5);
    fframes::svgr!(
        <g>
            {title}
            {nodes}
            {details}
            <g opacity={note_o}>
                <text x={nx} y={ny} font-family={MONO} font-weight="600" font-size="20" letter-spacing="3" fill={GREY}>"THE RUST SOURCE STAYS THE AUTHORITY."</text>
                <text x={nx} y={ny + 32.0} font-family={MONO} font-weight="600" font-size="20" letter-spacing="3" fill={GREY}>"NO HIDDEN TEMPLATE STATE."</text>
            </g>
        </g>
    )
}

// ---------------------------------------------------------------------------
// the race, the result and the table (as in examples/fframes-intro)

const RACE_START: f32 = 2.0;
/// The slower renderer crosses the line after this many beats.
const RACE_BEATS: f32 = 12.0;
const RESULT_AT: f32 = 16.0;
const TABLE_AT: f32 = 22.0;

fn slowest() -> f32 {
    BENCH_REMOTION_S.max(BENCH_FFRAMES_S).max(0.001)
}

fn race(lb: f32, p: bool) -> Svgr<'static> {
    let exit = expo_in(prog(lb, 15.6, 16.0));
    let real = prog(lb, RACE_START, RACE_START + RACE_BEATS) * slowest();
    let speedup = slowest() / (RACE_BEATS * BEAT);
    let lanes = [
        (
            BENCH_REMOTION_LABEL,
            "CHROME + REACT → H.264 MP4",
            BENCH_REMOTION_S,
            GREY,
        ),
        (
            BENCH_FFRAMES_LABEL,
            "RUST + SKIA → H.264 MP4",
            BENCH_FFRAMES_S,
            ACCENT,
        ),
    ];
    let (x0, bw) = if p { (80.0, 920.0) } else { (150.0, 1620.0) };
    let rows: Vec<Svgr> = lanes
        .iter()
        .enumerate()
        .map(|(i, (name, sub, secs, color))| {
            let y = if p { 800.0 + i as f32 * 290.0 } else { 470.0 + i as f32 * 230.0 };
            let (name_sz, time_sz) = if p { (40.0, 44.0) } else { (58.0, 52.0) };
            let appear = snap(lb - 0.6 - i as f32 * 0.3);
            let pr = (real / secs.max(0.001)).min(1.0);
            let done = pr >= 1.0;
            let shown = real.min(*secs);
            let bar_w = (bw * pr).max(0.5);
            let done_at = RACE_START + RACE_BEATS * secs / slowest();
            let stamp = snap(lb - done_at);
            let flash = if done { (-(lb - done_at) * 6.0).exp() } else { 0.0 };
            let frames_done = (300.0 * pr).floor() as u32;
            fframes::svgr!(
                <g opacity={prog(lb, 0.6 + i as f32 * 0.3, 0.7 + i as f32 * 0.3)} transform={format!("translate({} 0)", (1.0 - appear) * -120.0)}>
                    <text x={x0} y={y} font-family={DISPLAY} font-size={name_sz} letter-spacing="-2" fill={if i == 1 { ACCENT } else { BONE }}>{*name}</text>
                    <text x={x0 + 4.0} y={y + 34.0} font-family={MONO} font-weight="500" font-size="18" letter-spacing="2.5" fill={GREY}>{*sub}</text>
                    <text x={x0 + bw} y={y} text-anchor="end" font-family={MONO} font-weight="600" font-size={time_sz} fill={if done { *color } else { BONE }}>{format!("{shown:.3}s")}</text>
                    <rect x={x0} y={y + 56.0} width={bw} height="64" fill="#141312" stroke="#2f2c29" stroke-width="1.5" />
                    <rect x={x0} y={y + 56.0} width={bar_w} height="64" fill={*color} />
                    <rect x={x0} y={y + 56.0} width={bw} height="64" fill={BONE} opacity={flash * 0.6} />
                    <text x={x0 + 20.0} y={y + 100.0} font-family={MONO} font-weight="600" font-size="22" letter-spacing="2" fill={INK} opacity={if bar_w > 280.0 { 1.0 } else { 0.0 }}>{format!("{frames_done} / 300 FRAMES")}</text>
                    <g opacity={if done { 1.0 } else { 0.0 }} transform={format!("translate(0 {})", (1.0 - stamp) * 20.0)}>
                        <text x={x0 + bw - 20.0} y={y + 100.0} text-anchor="end" font-family={MONO} font-weight="700" font-size="24" letter-spacing="4" fill={INK}>"DONE"</text>
                    </g>
                </g>
            )
        })
        .collect();
    let head = if p {
        fframes::svgr!(
            <g>
                {Slam::new(80.0, 290.0, "100,000", DISPLAY, 130.0, BONE).draw(lb, 0.0, 140.0)}
                {Slam::new(80.0, 430.0, "TEXT NODES", DISPLAY, 130.0, BONE).draw(lb - 0.3, 0.0, 140.0)}
                {label(84.0, 500.0, "300 FRAMES · 3840×2160 · H.264 MP4".to_owned(), GREY, 20.0, "start")}
                {label(84.0, 536.0, "SAME LAYOUT, SAME FONT".to_owned(), GREY, 20.0, "start")}
            </g>
        )
    } else {
        fframes::svgr!(
            <g>
                {Slam::new(150.0, 250.0, "100,000 TEXT NODES", DISPLAY, 120.0, BONE).draw(lb, 0.0, 140.0)}
                {label(154.0, 310.0, "300 FRAMES · 3840×2160 · H.264 MP4 · SAME LAYOUT, SAME FONT".to_owned(), GREY, 20.0, "start")}
            </g>
        )
    };
    let (lx, ly, la) = if p {
        (84.0, 1190.0 + 290.0, "start")
    } else {
        (1770.0, 310.0, "end")
    };
    fframes::svgr!(
        <g opacity={1.0 - exit}>
            {head}
            {rows}
            <g opacity={prog(lb, RACE_START, RACE_START + 0.2)}>
                {label(lx, ly, format!("TIME-LAPSE ×{speedup:.1} · MEASURED WALL CLOCK"), ACCENT, 20.0, la)}
            </g>
        </g>
    )
}

fn result(l: f32, p: bool) -> Svgr<'static> {
    let ratio = BENCH_REMOTION_S / BENCH_FFRAMES_S.max(0.001);
    let n = 1.0 + (ratio - 1.0) * expo_out(prog(l, 0.0, 1.2));
    let s = 1.0 + (1.0 - expo_out(l / 0.35)) * 0.15;
    let word = snap(l - 1.0);
    let exit = expo_in(prog(l, 5.7, 6.0));
    let (w, h) = size(p);
    let body = if p {
        fframes::svgr!(
            <g>
                <g transform={format!("translate(70 940) scale({s}) translate(-70 -940)")}>
                    <text x="60" y="940" font-family={DISPLAY} font-size="230" letter-spacing="-14" fill={INK}>{format!("{n:.2}×")}</text>
                </g>
                <g opacity={prog(l, 1.0, 1.08)} transform={format!("translate(0 {})", (1.0 - word) * 60.0)}>
                    <text x="80" y="1130" font-family={DISPLAY} font-size="112" letter-spacing="-4" fill={INK}>"FASTER THAN"</text>
                    <text x="80" y="1250" font-family={DISPLAY} font-size="112" letter-spacing="-4" fill={INK}>"REMOTION"</text>
                </g>
                <g font-family={MONO} font-weight="600" font-size="22" letter-spacing="4" fill={INK}>
                    <text x="80" y="230">"SAME 100,000 TEXT NODES"</text>
                    <text x="80" y="270">"COMPLETE MP4 · MEDIAN WALL CLOCK"</text>
                </g>
                <rect x="80" y="296" width="920" height="3" fill={INK} />
            </g>
        )
    } else {
        fframes::svgr!(
            <g>
                <g transform={format!("translate(150 760) scale({s}) translate(-150 -760)")}>
                    <text x="130" y="760" font-family={DISPLAY} font-size="480" letter-spacing="-30" fill={INK}>{format!("{n:.2}×")}</text>
                </g>
                <text x="150" y={900.0 + (1.0 - word) * 60.0} font-family={DISPLAY} font-size="120" letter-spacing="-4" fill={INK} opacity={prog(l, 1.0, 1.08)}>"FASTER THAN REMOTION"</text>
                <g font-family={MONO} font-weight="600" font-size="22" letter-spacing="4" fill={INK}>
                    <text x="150" y="170">"SAME 100,000 TEXT NODES"</text>
                    <text x="1770" y="170" text-anchor="end">"COMPLETE MP4 · MEDIAN WALL CLOCK"</text>
                </g>
                <rect x="150" y="196" width="1620" height="3" fill={INK} />
            </g>
        )
    };
    fframes::svgr!(
        <g opacity={1.0 - exit}>
            <rect width={w} height={h} fill={ACCENT} />
            {body}
        </g>
    )
}

fn table(l: f32, p: bool) -> Svgr<'static> {
    let rows = [
        (BENCH_REMOTION_LABEL, BENCH_REMOTION_S, GREY),
        ("FFRAMES · CPU", BENCH_FFRAMES_CPU_S, BONE),
        (BENCH_FFRAMES_LABEL, BENCH_FFRAMES_S, ACCENT),
    ];
    let (x0, rw) = if p { (80.0, 920.0) } else { (150.0, 1620.0) };
    let items: Vec<Svgr> = rows
        .iter()
        .enumerate()
        .map(|(i, (name, secs, color))| {
            let at = 0.3 + i as f32 * 0.5;
            let s = snap(l - at);
            let (y, rh) = if p { (800.0 + i as f32 * 190.0, 166.0) } else { (520.0 + i as f32 * 130.0, 110.0) };
            let time = format!("{secs:.3} s");
            let body = if p {
                fframes::svgr!(
                    <g>
                        <text x={x0 + 40.0} y={y - 18.0} font-family={DISPLAY} font-size="34" letter-spacing="-1" fill={BONE}>{*name}</text>
                        <text x={x0 + 40.0} y={y + 52.0} font-family={MONO} font-weight="600" font-size="56" fill={*color}>{time}</text>
                    </g>
                )
            } else {
                fframes::svgr!(
                    <g>
                        <text x="190" y={y} font-family={DISPLAY} font-size="48" letter-spacing="-1" fill={BONE}>{*name}</text>
                        <text x="1740" y={y} text-anchor="end" font-family={MONO} font-weight="600" font-size="48" fill={*color}>{time}</text>
                    </g>
                )
            };
            let top = y - 70.0;
            fframes::svgr!(
                <g opacity={prog(l, at, at + 0.08)} transform={format!("translate({} 0)", (1.0 - s) * 80.0)}>
                    <rect x={x0} y={top} width={rw} height={rh} fill="#0f0e0d" fill-opacity="0.85" stroke="#2f2c29" stroke-width="1.5" />
                    <rect x={x0} y={top} width="6" height={rh} fill={*color} />
                    {body}
                </g>
            )
        })
        .collect();
    let exit = expo_in(prog(l, 9.6, 10.0));
    let (hx, hy, hs) = if p {
        (80.0, 330.0, 100.0)
    } else {
        (150.0, 300.0, 110.0)
    };
    let (fx, fy) = if p { (84.0, 1420.0) } else { (154.0, 860.0) };
    let (wx, wy, wa) = if p {
        (1000.0, 740.0, "end")
    } else {
        (1740.0, 400.0, "end")
    };
    fframes::svgr!(
        <g opacity={1.0 - exit}>
            {Slam::new(hx, hy, "THE NUMBERS", DISPLAY, hs, BONE).draw(l, 0.0, 120.0)}
            {label(wx, wy, "WALL CLOCK".to_owned(), GREY, 18.0, wa)}
            {items}
            {label(fx, fy, "RENDER + ENCODE + MP4".to_owned(), BONE, 18.0, "start")}
            {label(fx, fy + 40.0, BENCH_NOTE.to_owned(), GREY, 18.0, "start")}
            {label(fx, fy + 80.0, "REPRODUCE: RENDER-BENCH/VS-REMOTION".to_owned(), ACCENT, 18.0, "start")}
        </g>
    )
}

// ---------------------------------------------------------------------------
// why it is fast

const REASONS: [(&str, &str, [&str; 2]); 4] = [
    (
        "01",
        "Skia on the GPU",
        ["Metal or Vulkan. No headless", "browser, no screenshots."],
    ),
    (
        "02",
        "Static is cached",
        [
            "Markup without {} is hashed at",
            "compile time and drawn once.",
        ],
    ),
    (
        "03",
        "Every core works",
        [
            "render_frame runs per frame on",
            "many threads, no I/O inside.",
        ],
    ),
    (
        "04",
        "No readback",
        ["Frames go from the GPU texture", "straight to the encoder."],
    ),
];

const REASON_AT: [f32; 4] = [3.0, 10.0, 17.0, 24.0];

pub fn why(lb: f32, p: bool) -> Svgr<'static> {
    let head = if p {
        fframes::svgr!(
            <g>
                {Slam::new(80.0, 300.0, "WHY IT'S", DISPLAY, 130.0, BONE).draw(lb, -140.0, 0.0)}
                {Slam::new(80.0, 440.0, "FAST.", DISPLAY, 130.0, ACCENT).draw(lb - 0.5, -140.0, 0.0)}
            </g>
        )
    } else {
        fframes::svgr!(
            <g>
                {Slam::new(150.0, 250.0, "WHY IT'S", DISPLAY, 110.0, BONE).draw(lb, -140.0, 0.0)}
                {Slam::new(150.0, 370.0, "FAST.", DISPLAY, 110.0, ACCENT).draw(lb - 0.5, -140.0, 0.0)}
            </g>
        )
    };
    // the headline number
    let stat_o = prog(lb, 1.0, 1.4);
    let stat_s = snap(lb - 1.0);
    let stat = if p {
        fframes::svgr!(
            <g opacity={stat_o} transform={format!("translate(0 {})", (1.0 - stat_s) * 40.0)}>
                <text x="80" y="640" font-family={DISPLAY} font-size="150" letter-spacing="-6" fill={ACCENT}>"10×"</text>
                <text x="470" y="590" font-family={MONO} font-weight="600" font-size="22" letter-spacing="3" fill={BONE}>"SKIA BACKEND VS THE"</text>
                <text x="470" y="626" font-family={MONO} font-weight="600" font-size="22" letter-spacing="3" fill={BONE}>"CPU BACKEND, 1080P"</text>
            </g>
        )
    } else {
        fframes::svgr!(
            <g opacity={stat_o} transform={format!("translate(0 {})", (1.0 - stat_s) * 40.0)}>
                <text x="1770" y="330" text-anchor="end" font-family={DISPLAY} font-size="220" letter-spacing="-10" fill={ACCENT}>"10×"</text>
                <text x="1770" y="376" text-anchor="end" font-family={MONO} font-weight="600" font-size="22" letter-spacing="3" fill={BONE}>"SKIA BACKEND VS THE CPU BACKEND, 1080P"</text>
            </g>
        )
    };
    let rows: Vec<Svgr> = REASONS
        .iter()
        .enumerate()
        .map(|(i, (num, title, lines))| {
            let at = REASON_AT[i];
            let s = snap(lb - at);
            let current = lb >= at && (i == 3 || lb < REASON_AT[i + 1]);
            let (x, y, w, h) = if p {
                (80.0, 740.0 + i as f32 * 262.0, 920.0, 244.0)
            } else {
                (150.0, 420.0 + i as f32 * 140.0, 1620.0, 124.0)
            };
            let stroke = if current { ACCENT } else { "#3d3a35" };
            let off = (1.0 - s) * 80.0;
            let body = if p {
                fframes::svgr!(
                    <g>
                        <text x={x + 36.0} y={y + 56.0} font-family={MONO} font-weight="600" font-size="24" letter-spacing="3" fill={ACCENT}>{*num}</text>
                        <text x={x + 36.0} y={y + 128.0} font-family={DISPLAY} font-size="52" letter-spacing="-1" fill={BONE}>{*title}</text>
                        <text x={x + 36.0} y={y + 178.0} font-family={MONO} font-weight="500" font-size="26" fill="#b9b4ab">{lines[0]}</text>
                        <text x={x + 36.0} y={y + 214.0} font-family={MONO} font-weight="500" font-size="26" fill="#b9b4ab">{lines[1]}</text>
                    </g>
                )
            } else {
                fframes::svgr!(
                    <g>
                        <text x={x + 36.0} y={y + 74.0} font-family={MONO} font-weight="600" font-size="26" letter-spacing="3" fill={ACCENT}>{*num}</text>
                        <text x={x + 110.0} y={y + 78.0} font-family={DISPLAY} font-size="54" letter-spacing="-1" fill={BONE}>{*title}</text>
                        <text x="1010" y={y + 54.0} font-family={MONO} font-weight="500" font-size="26" fill="#b9b4ab">{lines[0]}</text>
                        <text x="1010" y={y + 92.0} font-family={MONO} font-weight="500" font-size="26" fill="#b9b4ab">{lines[1]}</text>
                    </g>
                )
            };
            fframes::svgr!(
                <g opacity={prog(lb, at, at + 0.15)} transform={format!("translate({off} 0)")}>
                    <rect x={x} y={y} width={w} height={h} fill="#0f0e0d" fill-opacity="0.92" stroke={stroke} stroke-width="2" />
                    <rect x={x} y={y} width="6" height={h} fill={if current { ACCENT } else { "#3d3a35" }} />
                    {body}
                </g>
            )
        })
        .collect();
    fframes::svgr!(<g>{head}{stat}{rows}</g>)
}
