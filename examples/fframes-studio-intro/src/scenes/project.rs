//! Under the hood: the project is plain Rust, and Studio keeps its history.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use crate::beat::*;
use crate::shaders::SHADERS;
use crate::ui::*;

beat_scene!(ProjectScene, Some(80.0), Some(96.0));

pub const BLIP_BEATS: &[f32] = &[84.0, 87.0, 90.0];

struct Card {
    tag: &'static str,
    title: &'static str,
    lines: [&'static str; 2],
}

const CARDS: [Card; 3] = [
    Card {
        tag: "01 / AGENTS",
        title: "Bring your agent",
        lines: ["Codex or Claude Code.", "Switch mid-task, context follows."],
    },
    Card {
        tag: "02 / HISTORY",
        title: "Undo any edit",
        lines: ["Every change is a revision.", "Compare, restore, export."],
    },
    Card {
        tag: "03 / STYLE",
        title: "Style presets",
        lines: [
            "Pick a look for the project.",
            "The agent adapts the scenes.",
        ],
    },
];

impl Scene for ProjectScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let bg = SHADERS.grid.draw(
            &frame,
            ShaderUniforms::new()
                .float("uSpeed", 0.8)
                .float("uHorizon", 0.5)
                .float("uBright", 0.3)
                .color("uInk", Color::hex("#6f6a63"))
                .color("uHot", Color::hex(ACCENT)),
        );
        let head = Slam::new(160.0, 330.0, "It's just Rust.", DISPLAY, 160.0, BONE);
        let sub = prog(lb, 1.2, 1.6);

        let cards: Vec<Svgr> = CARDS
            .iter()
            .enumerate()
            .map(|(i, card)| {
                let x = 160.0 + i as f32 * 560.0;
                let at = 3.0 + i as f32 * 3.0;
                let s = snap(lb - at);
                fframes::svgr!(
                    <g opacity={prog(lb, at, at + 0.15)} transform={format!("translate(0 {})", (1.0 - s) * 60.0)}>
                        <rect x={x} y="560" width="520" height="300" fill="#0f0e0d" fill-opacity="0.92" stroke="#3d3a35" stroke-width="2" />
                        <rect x={x} y="560" width={(520.0 * s.clamp(0.0, 1.0)).max(0.5)} height="4" fill={ACCENT} />
                        <text x={x + 36.0} y="622" font-family={MONO} font-weight="600" font-size="20" letter-spacing="3" fill={ACCENT}>{card.tag}</text>
                        <text x={x + 36.0} y="710" font-family={DISPLAY} font-size="44" letter-spacing="-1" fill={BONE}>{card.title}</text>
                        <text x={x + 36.0} y="776" font-family={MONO} font-weight="500" font-size="22" fill="#b9b4ab">{card.lines[0]}</text>
                        <text x={x + 36.0} y="816" font-family={MONO} font-weight="500" font-size="22" fill="#b9b4ab">{card.lines[1]}</text>
                    </g>
                )
            })
            .collect();

        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1920" height="1080" />
                {head.draw(lb, -140.0, 0.0)}
                <g opacity={sub}>
                    <text x="164" y="410" font-family={MONO} font-weight="500" font-size="30" letter-spacing="1" fill={GREY}>"An agent edits an ordinary fframes project."</text>
                    <text x="164" y="456" font-family={MONO} font-weight="500" font-size="30" letter-spacing="1" fill={GREY}>"Open it in any editor. Render it with the CLI."</text>
                </g>
                {cards}
            </g>
        )
    }
}
