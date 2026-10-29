//! Act 6: why it is fast. Three engines are named one at a time, each with a
//! small demonstration of what it saves, then how long this very video took to
//! render on a CPU.

use fframes::{Duration, FFramesContext, Frame, Scene, Svgr};

use super::look::*;
use crate::beat::*;

beat_scene!(SpeedScene, Some(208.0), Some(240.0));

/// Beats (inside the act) each engine is named on.
const ENGINES_AT: [f32; 3] = [2.0, 9.0, 16.0];
/// Beat the engines make way for the measurements.
const PROOF: f32 = 23.0;

/// Video beats the engines and the bars land on.
pub const BLIP_BEATS: [f32; 5] = [210.0, 217.0, 224.0, 231.8, 232.6];

struct Engine {
    title: &'static str,
    lines: [&'static str; 2],
}

const ENGINES: [Engine; 3] = [
    Engine {
        title: "Drawn once",
        lines: [
            "Parts that never change are",
            "cached; only motion is redrawn.",
        ],
    },
    Engine {
        title: "Straight to the GPU",
        lines: [
            "Skia draws the svgr! tree, about",
            "10x the CPU backend at 1080p.",
        ],
    },
    Engine {
        title: "Straight into the encoder",
        lines: ["Frames go to libav in memory,", "no PNG files on the disk."],
    },
];

/// The demonstration panel on the right.
const DEMO: (f32, f32, f32, f32) = (820.0, 330.0, 980.0, 540.0);

impl Scene for SpeedScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let active = ENGINES_AT.iter().rposition(|b| lb >= *b);
        let away = leave(lb, PROOF - 0.5);

        let list: Vec<Svgr> = ENGINES
            .iter()
            .enumerate()
            .map(|(i, e)| engine_row(lb, i, e, active == Some(i)))
            .collect();
        let demo = match active {
            Some(i) if lb < PROOF => {
                let (o, d) = rise(lb, ENGINES_AT[i], 30.0);
                let o = o * leave(lb, ENGINES_AT.get(i + 1).copied().unwrap_or(PROOF) - 0.3);
                let body = match i {
                    0 => cached(lb),
                    1 => lanes(lb - ENGINES_AT[1]),
                    _ => encoder(lb - ENGINES_AT[2]),
                };
                let (x, y, w, h) = DEMO;
                fframes::svgr!(
                    <g opacity={o} transform={at(0.0, d)}>
                        {panel(x, y, w, h)}
                        {body}
                    </g>
                )
            }
            _ => Svgr::empty(),
        };

        let caption = caption(
            lb,
            ENGINES_AT[0] + 0.5,
            PROOF - 0.6,
            "Under the hood:",
            "less work per frame, and no detours.",
            ACTION,
        );
        fframes::svgr!(
            <g opacity={leave(lb, 31.4)}>
                <g opacity={away}>
                    {act_title(lb, "06", "WHY IT'S FAST", "Fast by design.")}
                    {list}
                    {demo}
                    {caption}
                </g>
                {proof(lb)}
            </g>
        )
    }
}

fn engine_row(lb: f32, i: usize, e: &Engine, active: bool) -> Svgr<'static> {
    let land = ENGINES_AT[i];
    if lb < land - 0.3 {
        return Svgr::empty();
    }
    let (o, d) = rise(lb, land - 0.3, 30.0);
    let y = 380.0 + i as f32 * 160.0;
    let dim = if active { 1.0 } else { 0.35 };
    let bar = if active {
        prog(lb, land, land + 0.8)
    } else {
        0.0
    };
    let number = format!("0{}", i + 1);
    fframes::svgr!(
        <g opacity={o * dim} transform={at(0.0, d)}>
            <rect x="120" y={y - 34.0} width="4" height={(118.0 * bar).max(0.5)} fill={ACTION} opacity={if bar > 0.0 { 1.0 } else { 0.0 }} />
            <text x="148" y={y} font-family={MONO} font-weight="500" font-size="20" letter-spacing="3" fill={ACTION}>{number}</text>
            <text x="200" y={y + 2.0} font-family={SANS} font-weight={DISPLAY_WEIGHT} font-size="40" letter-spacing="-1.6" fill={TEXT}>{e.title}</text>
            <text x="148" y={y + 48.0} font-family={SANS} font-weight="400" font-size="24" fill={SECONDARY}>{e.lines[0]}</text>
            <text x="148" y={y + 78.0} font-family={SANS} font-weight="400" font-size="24" fill={SECONDARY}>{e.lines[1]}</text>
        </g>
    )
}

/// A chart whose frame, grid and labels are cached once while its line is drawn
/// every frame.
fn cached(lb: f32) -> Svgr<'static> {
    let (x, y, w, h) = DEMO;
    let (cx, cy, cw, ch) = (x + 70.0, y + 90.0, w - 140.0, h - 210.0);
    let grid: Vec<Svgr> = (0..5)
        .map(|k| {
            let gy = cy + k as f32 * ch / 4.0;
            fframes::svgr!(<rect x={cx} y={gy} width={cw} height="1.5" fill={RULE} />)
        })
        .collect();
    // the moving part: a line that draws itself and keeps scrolling
    let t = lb * BEAT;
    let n = 48;
    let reveal = prog(lb, ENGINES_AT[0], ENGINES_AT[0] + 2.0);
    let points: Vec<String> = (0..=n)
        .filter(|k| (*k as f32) <= n as f32 * reveal)
        .map(|k| {
            let u = k as f32 / n as f32;
            let v = 0.5 + 0.28 * (u * 9.0 + t * 2.0).sin() * (0.6 + 0.4 * (u * 3.0 - t).cos())
                - u * 0.25;
            format!("{} {}", cx + u * cw, cy + ch * (1.0 - v.clamp(0.02, 0.98)))
        })
        .collect();
    let line = if points.len() > 1 {
        let d = format!("M{}", points.join(" L"));
        fframes::svgr!(<path d={d} fill="none" stroke={CANDIDATE} stroke-width="4" stroke-linejoin="round" />)
    } else {
        Svgr::empty()
    };
    let redrawn = ((lb - ENGINES_AT[0]) * BEAT * 60.0).max(0.0) as usize;
    let tag_y = y + h - 70.0;
    // what is cached stays put like an accepted state, the line is redrawn work
    fframes::svgr!(
        <g>
            <text x={cx} y={y + 54.0} font-family={MONO} font-weight="500" font-size="18" letter-spacing="3" fill={SECONDARY}>"REVENUE · LIVE"</text>
            {grid}
            <rect x={cx - 14.0} y={cy - 14.0} width={cw + 28.0} height={ch + 28.0} rx={RADIUS} fill="none" stroke={ACCEPTED} stroke-width="1.5" />
            {line}
            <rect x={cx} y={tag_y - 18.0} width="14" height="14" fill="none" stroke={ACCEPTED} stroke-width="1.5" />
            <text x={cx + 26.0} y={tag_y - 5.0} font-family={MONO} font-weight="400" font-size="19" fill={ACCEPTED}>"cached once"</text>
            <rect x={cx + 260.0} y={tag_y - 13.0} width="22" height="4" fill={CANDIDATE} />
            <text x={cx + 294.0} y={tag_y - 5.0} font-family={MONO} font-weight="400" font-size="19" fill={CANDIDATE}>{format!("redrawn: {redrawn} frames")}</text>
        </g>
    )
}

/// Frames leaving the CPU backend and the GPU backend, the GPU lane ten times as
/// often.
fn lanes(local: f32) -> Svgr<'static> {
    let (x, y, w, _) = DEMO;
    let t = local.max(0.0) * BEAT;
    let lane = |row: usize, label: &'static str, color: &'static str, per_second: f32| {
        let ly = y + 110.0 + row as f32 * 190.0;
        let (tx, tw) = (x + 60.0, w - 120.0);
        let made = (t * per_second) as usize;
        let tiles: Vec<Svgr> = (0..made.min(400))
            .filter_map(|k| {
                let age = t - k as f32 / per_second;
                let px = tx + age * 520.0;
                (px < tx + tw - 40.0).then(|| {
                    fframes::svgr!(<rect x={px} y={ly + 40.0} width="16" height="56" rx="3" fill={color} opacity="0.85" />)
                })
            })
            .collect();
        fframes::svgr!(
            <g>
                <text x={tx} y={ly} font-family={MONO} font-weight="500" font-size="18" letter-spacing="3" fill={color}>{label}</text>
                <text x={tx + tw} y={ly} text-anchor="end" font-family={MONO} font-weight="400" font-size="19" fill={SECONDARY}>{format!("{made} frames")}</text>
                <rect x={tx} y={ly + 20.0} width={tw} height="96" rx={RADIUS} fill={EMPHASIS} />
                {tiles}
            </g>
        )
    };
    fframes::svgr!(
        <g>
            {lane(0, "CPU BACKEND", SECONDARY, 2.0)}
            {lane(1, "SKIA ON THE GPU", CANDIDATE, 20.0)}
        </g>
    )
}

/// The pipeline from the tree to the file, with the PNG detour crossed out.
fn encoder(local: f32) -> Svgr<'static> {
    let (x, y, w, _) = DEMO;
    let stages = ["svgr! tree", "Skia", "libav", "launch.mp4"];
    let bw = 180.0;
    let gap = (w - 120.0 - bw * 4.0) / 3.0;
    let by = y + 120.0;
    let boxes: Vec<Svgr> = stages
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let bx = x + 60.0 + k as f32 * (bw + gap);
            let (fill, color) = if k == 3 {
                (ACCEPTED_SURFACE, ACCEPTED)
            } else {
                (PANEL, TEXT)
            };
            fframes::svgr!(
                <g>
                    <rect x={bx} y={by} width={bw} height="90" rx={RADIUS} fill={fill} stroke={if k == 3 { ACCEPTED } else { CONTROL }} stroke-width="1.5" />
                    <text x={bx + bw / 2.0} y={by + 53.0} text-anchor="middle" font-family={MONO} font-weight="500" font-size="22" fill={color}>{*s}</text>
                </g>
            )
        })
        .collect();
    // frames travelling the pipe
    let t = local.max(0.0) * BEAT;
    let span = w - 120.0 - bw;
    let packets: Vec<Svgr> = (0..12)
        .filter_map(|k| {
            let u = t * 0.9 - k as f32 * 0.12;
            (0.0..1.0).contains(&u).then(|| {
                let px = x + 60.0 + bw / 2.0 + u * span;
                fframes::svgr!(<rect x={px - 7.0} y={by + 112.0} width="14" height="14" rx="2" fill={CANDIDATE} />)
            })
        })
        .collect();
    let strike = prog(local, 1.5, 2.3);
    let dy = y + 340.0;
    let dx = x + 60.0;
    fframes::svgr!(
        <g>
            {boxes}
            <rect x={x + 60.0 + bw / 2.0} y={by + 118.0} width={span} height="2" fill={CONTROL} opacity="0.6" />
            {packets}
            <g opacity="0.8">
                <rect x={dx} y={dy} width={w - 120.0} height="110" rx={RADIUS} fill="none" stroke={CONTROL} stroke-width="1.5" stroke-dasharray="8 8" />
                <text x={dx + 30.0} y={dy + 48.0} font-family={MONO} font-weight="400" font-size="20" fill={SECONDARY}>"frame_0001.png  frame_0002.png  frame_0003.png  …"</text>
                <text x={dx + 30.0} y={dy + 84.0} font-family={SANS} font-weight="400" font-size="22" fill={SECONDARY}>"the usual detour: every frame written to disk, then read back"</text>
                <rect x={dx + 20.0} y={dy + 40.0} width={((w - 160.0) * strike).max(0.5)} height="3" opacity={if strike > 0.0 { 1.0 } else { 0.0 }} fill={FAILURE} />
            </g>
        </g>
    )
}

/// Frames of this video, and the seconds its own render took on 32 CPU cores (four
/// 8-core Intel Xeon Icelake sockets) with no GPU: Skia on the CPU, encoded to H.264.
const THIS_FRAMES: f32 = 7401.0;
const THIS_RENDER_S: f32 = 26.0;

/// This very video, rendered on a CPU: the time, and the frames per second.
fn proof(lb: f32) -> Svgr<'static> {
    if lb < PROOF - 0.3 {
        return Svgr::empty();
    }
    let (eo, ed) = rise(lb, PROOF - 0.3, 24.0);
    let (ho, hd) = rise(lb, PROOF, 40.0);
    let count = expo_out(prog(lb, PROOF + 1.0, PROOF + 3.5));
    let frames = (THIS_FRAMES * count).round() as u32;
    let seconds = THIS_RENDER_S * count;
    let fps = THIS_FRAMES / THIS_RENDER_S;
    let (so, sd) = rise(lb, PROOF + 0.8, 30.0);
    let (po, pd) = rise(lb, PROOF + 1.6, 30.0);
    let no = prog(lb, PROOF + 3.5, PROOF + 4.1);
    fframes::svgr!(
        <g>
            <g opacity={eo} transform={at(0.0, ed)}>{act_eyebrow("06", "MEASURED")}</g>
            <g opacity={ho} transform={at(0.0, hd)}>{headline(116.0, 252.0, "This video, rendered.", 72.0, TEXT)}</g>
            <g opacity={ho}>
                <text x="120" y="326" font-family={SANS} font-weight="400" font-size="30" fill={SECONDARY}>"7,401 frames at 1920×1080, 60 fps, to an H.264 MP4. No GPU."</text>
            </g>
            <g opacity={so} transform={at(0.0, sd)}>
                <text x="112" y="560" font-family={SANS} font-weight={DISPLAY_WEIGHT} font-size="180" letter-spacing="-7.2" fill={ACTION}>{format!("{seconds:.1} s")}</text>
                <text x="124" y="620" font-family={MONO} font-weight="500" font-size="20" letter-spacing="3" fill={SECONDARY}>"TO RENDER AND ENCODE ALL OF IT"</text>
            </g>
            <g opacity={po} transform={at(0.0, pd)}>
                <text x="1052" y="560" font-family={SANS} font-weight={DISPLAY_WEIGHT} font-size="180" letter-spacing="-7.2" fill={TEXT}>{format!("{:.0}", fps * count.min(1.0))}</text>
                <text x="1064" y="620" font-family={MONO} font-weight="500" font-size="20" letter-spacing="3" fill={SECONDARY}>"FRAMES EVERY SECOND"</text>
            </g>
            <g opacity={so}>
                <rect x="120" y="700" width="1680" height="8" rx="4" fill={EMPHASIS} />
                <rect x="120" y="700" width={(1680.0 * count).max(0.5)} height="8" rx="4" fill={ACTION} />
                <text x="120" y="756" font-family={MONO} font-weight="500" font-size="22" fill={TEXT}>{format!("{frames} / 7401 frames")}</text>
            </g>
            <g opacity={no}>
                <text x="120" y="900" font-family={MONO} font-weight="400" font-size="17" letter-spacing="2" fill={SECONDARY}>"SKIA ON 32 CPU CORES · INTEL XEON (ICELAKE) · ONE RUN"</text>

            </g>
        </g>
    )
}
