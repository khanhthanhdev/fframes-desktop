//! Why it is fast, the visual: a wall of squares, each one a frame, all rendering at
//! once. When the last one lands the wall resolves into the QR code, which slides to
//! the place it has on the end card.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use crate::beat::*;
use crate::shaders::SHADERS;
use crate::short::qr::{QR_SIZE, TILE};
use crate::ui::*;
use crate::wall::wall;

beat_scene!(ParallelScene, Some(198.0), Some(208.0));

/// Where the QR code sits on the end card.
pub const QR_AT: (f32, f32) = (1420.0, 540.0);
pub const QR_PX: f32 = 520.0;
/// The wall while it renders.
const WALL_AT: (f32, f32) = (1130.0, 540.0);
const WALL_PX: f32 = 700.0;

pub const BLIP_BEATS: &[f32] = &[205.0];

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
                .float("uHorizon", 0.55)
                .float("uBright", 0.3)
                .color("uInk", Color::hex("#6f6a63"))
                .color("uHot", Color::hex(ACCENT)),
        );

        let resolve = cubic_in_out(prog(lb, 4.8, 5.8));
        let to_qr = prog(lb, 5.7, 6.1);
        let slide = cubic_in_out(prog(lb, 6.6, 8.8));
        let size = lerp(WALL_PX, QR_PX, slide);
        let at = (
            lerp(WALL_AT.0, QR_AT.0, slide),
            lerp(WALL_AT.1, QR_AT.1, slide),
        );
        let pad = size * (TILE - QR_SIZE) / QR_SIZE / 2.0;
        let (wall, done) = wall(lb, at, size, pad, resolve, to_qr);
        let tile_o = prog(lb, 0.0, 0.3);
        let tile_s = 0.9 + 0.1 * snap(lb);

        let head = snap(lb);
        let text_o = prog(lb, 0.0, 0.2) * (1.0 - prog(lb, 5.6, 6.0));
        let count_o = prog(lb, 0.3, 0.5) * (1.0 - prog(lb, 5.3, 5.7));
        let count = format!("{done}");
        let (cx, cy) = at;

        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1920" height="1080" />
                <g opacity={text_o} transform={format!("translate(0 {})", (1.0 - head) * 24.0)}>
                    <text x="110" y="250" font-family={MONO} font-weight="600" font-size="22" letter-spacing="4" fill={ACCENT}>"WHY IT IS FAST"</text>
                    <text x="110" y="350" font-family={DISPLAY} font-size="84" letter-spacing="-2" fill={BONE}>"Every core."</text>
                    <text x="110" y="440" font-family={DISPLAY} font-size="84" letter-spacing="-2" fill={ACCENT}>"Every frame."</text>
                </g>
                <g opacity={count_o}>
                    <text x="110" y="700" font-family={DISPLAY} font-size="150" letter-spacing="-5" fill={BONE}>{count}</text>
                    <text x="116" y="752" font-family={MONO} font-weight="600" font-size="24" letter-spacing="4" fill={ACCENT}>"FRAMES DONE"</text>
                    <text x="116" y="800" font-family={MONO} font-weight="500" font-size="20" fill={GREY}>"Each square is a frame, all rendering at once."</text>
                </g>
                <g opacity={tile_o} transform={format!("translate({cx} {cy}) scale({tile_s}) translate(-{cx} -{cy})")}>
                    {wall}
                </g>
            </g>
        )
    }
}
