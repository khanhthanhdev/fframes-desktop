//! Install: three steps that finish themselves, no terminal involved.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use crate::beat::*;
use crate::shaders::SHADERS;
use crate::ui::*;

beat_scene!(InstallScene, Some(0.0), Some(16.0));

/// Beats (on the video grid) where a step finishes.
pub const BLIP_BEATS: &[f32] = &[7.0, 9.0, 12.5];

const CARD_W: f32 = 520.0;
const CARD_H: f32 = 300.0;
const CARD_Y: f32 = 560.0;

pub(crate) struct Step {
    pub num: &'static str,
    pub title: &'static str,
    /// Beat the work starts and the beat it is done.
    pub from: f32,
    pub to: f32,
}

pub(crate) const STEPS: [Step; 3] = [
    Step {
        num: "01",
        title: "Download",
        from: 4.6,
        to: 7.0,
    },
    Step {
        num: "02",
        title: "Connect an agent",
        from: 7.4,
        to: 9.0,
    },
    Step {
        num: "03",
        title: "Get the SDK",
        from: 9.4,
        to: 12.5,
    },
];

pub(crate) fn check(x: f32, y: f32, p: f32) -> Svgr<'static> {
    if p <= 0.0 {
        return Svgr::empty();
    }
    let len = 40.0;
    fframes::svgr!(
        <path d={format!("M{x} {y} l9 9 l18 -20")} fill="none" stroke="#6fcf8a" stroke-width="5"
              stroke-linecap="round" stroke-linejoin="round" stroke-dasharray={format!("{} {len}", len * p.min(1.0))} />
    )
}

pub(crate) fn bar(x: f32, y: f32, w: f32, p: f32) -> Svgr<'static> {
    fframes::svgr!(
        <g>
            <rect x={x} y={y} width={w} height="8" fill={DIM} />
            <rect x={x} y={y} width={(w * p).max(0.5)} height="8" fill={ACCENT} />
        </g>
    )
}

impl Scene for InstallScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let bg = SHADERS.grid.draw(
            &frame,
            ShaderUniforms::new()
                .float("uSpeed", 0.5)
                .float("uHorizon", 0.5)
                .float("uBright", 0.22)
                .color("uInk", Color::hex("#6f6a63"))
                .color("uHot", Color::hex(ACCENT)),
        );

        let head = Slam::new(160.0, 330.0, "Install it.", DISPLAY, 170.0, BONE);
        let sub = Slam::new(160.0, 470.0, "like any app.", SERIF, 120.0, ACCENT).italic();

        let cards: Vec<Svgr> = STEPS
            .iter()
            .enumerate()
            .map(|(i, step)| {
                let x = 160.0 + i as f32 * (CARD_W + 40.0);
                let at = 3.0 + i as f32 * 0.6;
                let s = snap(lb - at);
                let work = prog(lb, step.from, step.to);
                let done = work >= 1.0;
                let detail = match i {
                    0 => platforms(lb),
                    1 => agents(lb),
                    _ => sdk(work, CARD_W - 72.0),
                };
                let status = if done {
                    "READY".to_owned()
                } else if work > 0.0 {
                    "WORKING".to_owned()
                } else {
                    "WAITING".to_owned()
                };
                let stroke = if done { "#6fcf8a" } else if work > 0.0 { ACCENT } else { "#3d3a35" };
                fframes::svgr!(
                    <g opacity={prog(lb, at, at + 0.15)} transform={format!("translate(0 {})", (1.0 - s) * 50.0)}>
                        <rect x={x} y={CARD_Y} width={CARD_W} height={CARD_H} fill="#0f0e0d" fill-opacity="0.92" stroke={stroke} stroke-width="2" />
                        <text x={x + 36.0} y={CARD_Y + 62.0} font-family={MONO} font-weight="600" font-size="24" letter-spacing="3" fill={ACCENT}>{step.num}</text>
                        <text x={x + CARD_W - 36.0} y={CARD_Y + 62.0} text-anchor="end" font-family={MONO} font-weight="500" font-size="18" letter-spacing="3" fill={if done { "#6fcf8a" } else { GREY }}>{status}</text>
                        <text x={x + 36.0} y={CARD_Y + 124.0} font-family={DISPLAY} font-size="42" letter-spacing="-1" fill={BONE}>{step.title}</text>
                        <g transform={format!("translate({} {})", x + 36.0, CARD_Y + 160.0)}>{detail}</g>
                        {bar(x + 36.0, CARD_Y + CARD_H - 44.0, CARD_W - 72.0, work)}
                        {check(x + CARD_W - 76.0, CARD_Y + CARD_H - 62.0, prog(lb, step.to, step.to + 0.5))}
                    </g>
                )
            })
            .collect();

        let tag = snap(lb - 13.0);
        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1920" height="1080" />
                {head.draw(lb - 0.0, -120.0, 0.0)}
                {sub.draw(lb - 1.0, 120.0, 0.0)}
                {cards}
                <g opacity={prog(lb, 13.0, 13.3)} transform={format!("translate(0 {})", (1.0 - tag) * 24.0)}>
                    <text x="160" y="950" font-family={MONO} font-weight="600" font-size="30" letter-spacing="3" fill={BONE}>"NO TERMINAL.  NO SETUP SCRIPTS."</text>
                </g>
            </g>
        )
    }
}

/// The three desktop platforms the installers target.
pub(crate) fn platforms(lb: f32) -> Svgr<'static> {
    let items = ["macOS", "Windows", "Linux"];
    let rows: Vec<Svgr> = items
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let x = i as f32 * 150.0;
            let lit = prog(lb, 4.8 + i as f32 * 0.5, 5.2 + i as f32 * 0.5);
            fframes::svgr!(
                <g transform={format!("translate({x} 0)")}>
                    <rect x="0" y="-26" width="136" height="40" fill="none" stroke={if lit > 0.5 { ACCENT } else { "#3d3a35" }} stroke-width="1.5" />
                    <text x="68" y="2" text-anchor="middle" font-family={MONO} font-weight="500" font-size="21" fill={if lit > 0.5 { BONE } else { GREY }}>{*name}</text>
                </g>
            )
        })
        .collect();
    fframes::svgr!(<g>{rows}</g>)
}

pub(crate) fn agents(lb: f32) -> Svgr<'static> {
    let items = ["Codex", "Claude Code"];
    let rows: Vec<Svgr> = items
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let at = 7.6 + i as f32 * 0.6;
            let lit = prog(lb, at, at + 0.3);
            let x = i as f32 * 190.0;
            fframes::svgr!(
                <g transform={format!("translate({x} 0)")}>
                    <circle cx="10" cy="-8" r="8" fill={if lit > 0.5 { "#6fcf8a" } else { DIM }} />
                    <text x="32" y="0" font-family={MONO} font-weight="500" font-size="23" fill={if lit > 0.5 { BONE } else { GREY }}>{*name}</text>
                </g>
            )
        })
        .collect();
    fframes::svgr!(<g>{rows}</g>)
}

pub(crate) fn sdk(work: f32, width: f32) -> Svgr<'static> {
    let pct = format!("{:>3}%", (work * 100.0) as u32);
    fframes::svgr!(
        <g>
            <text x="0" y="0" font-family={MONO} font-weight="500" font-size="23" fill={GREY}>"fframes SDK"</text>
            <text x={width} y="0" text-anchor="end" font-family={MONO} font-weight="600" font-size="23" fill={BONE}>{pct}</text>
        </g>
    )
}
