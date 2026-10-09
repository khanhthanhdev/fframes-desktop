//! Why it is fast, the visual: a wall of squares, each one a frame, all rendering at
//! once. When the last one lands the wall resolves into the QR code, which slides
//! to the place it has on the end card.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use super::qr::{CX, CY, QR_SIZE, TILE};
use crate::beat::*;
use crate::shaders::SHADERS;
use crate::ui::*;
use crate::wall::wall;

short_scene!(ParallelScene, Some(super::PARALLEL_AT), super::END_AT);

/// Size of the wall while it renders, and where its center is.
const WALL: f32 = 780.0;
const WALL_Y: f32 = 1010.0;

pub const BLIP_BEATS: &[f32] = &[5.0];

impl Scene for ParallelScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let bg = SHADERS.grid.draw(
            &frame,
            ShaderUniforms::new()
                .float("uSpeed", 3.0)
                .float("uHorizon", 0.4)
                .float("uBright", 0.3)
                .color("uInk", Color::hex("#6f6a63"))
                .color("uHot", Color::hex(ACCENT)),
        );

        let resolve = cubic_in_out(prog(lb, 4.6, 5.6));
        let to_qr = prog(lb, 5.5, 5.9);
        let slide = cubic_in_out(prog(lb, 6.0, 7.7));
        let size = lerp(WALL, QR_SIZE, slide);
        let cy = lerp(WALL_Y, CY, slide);

        let pad = size * (TILE - QR_SIZE) / QR_SIZE / 2.0;
        let (wall, done) = wall(lb, (CX, cy), size, pad, resolve, to_qr);
        let tile_o = prog(lb, 0.0, 0.3);
        let tile_s = 0.9 + 0.1 * snap(lb);

        let head = snap(lb);
        let head_o = prog(lb, 0.0, 0.2) * (1.0 - prog(lb, 5.6, 6.0));
        let count_o = prog(lb, 0.3, 0.5) * (1.0 - prog(lb, 5.3, 5.7));
        let count = format!("{done}");

        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                <g opacity={head_o} transform={format!("translate(0 {})", (1.0 - head) * 24.0)}>
                    <text x="80" y="300" font-family={MONO} font-weight="600" font-size="24" letter-spacing="4" fill={ACCENT}>"WHY IT IS FAST"</text>
                    <text x="80" y="372" font-family={DISPLAY} font-size="56" letter-spacing="-1" fill={BONE}>"Every core. Every frame."</text>
                </g>
                <g opacity={tile_o} transform={format!("translate(540 {cy}) scale({tile_s}) translate(-540 -{cy})")}>
                    {wall}
                </g>
                <g opacity={count_o}>
                    <text x="80" y="1626" font-family={DISPLAY} font-size="120" letter-spacing="-3" fill={BONE}>{count}</text>
                    <text x="1000" y="1626" text-anchor="end" font-family={MONO} font-weight="600" font-size="26" letter-spacing="3" fill={ACCENT}>"FRAMES DONE"</text>
                    <text x="80" y="1686" font-family={MONO} font-weight="500" font-size="21" fill={GREY}>"Each square is a frame, all rendering at once."</text>
                </g>
            </g>
        )
    }
}
