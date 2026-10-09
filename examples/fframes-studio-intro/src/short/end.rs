//! The end card: the wordmark, a QR code to scan, and where to get it.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use super::qr::{CX, CY, QR_SIZE, TILE, tile};
use crate::beat::*;
use crate::shaders::SHADERS;
use crate::ui::*;

short_scene!(EndScene, Some(super::END_AT), super::LAST_BEAT);

const WORD_SIZE: usize = 200;

impl Scene for EndScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, mut frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let t_end = super::total_seconds() - gsec(&frame);
        let fade_out = prog(1.6 - t_end, 0.0, 1.6);
        let flash = (-lb * 9.0).exp();
        let bg = SHADERS.contour.draw(
            &frame,
            ShaderUniforms::new()
                .float2("uWell", 0.5, 0.3)
                .float("uDepth", 1.1)
                .float("uDensity", 14.0)
                .float("uBright", 0.25 + 0.5 * (1.0 - expo_out(prog(lb, 0.0, 6.0))))
                .color("uInk", Color::hex("#8a857c"))
                .color("uHot", Color::hex(ACCENT)),
        );

        // the wordmark with its trail of f's
        let w = measure(&mut frame, ctx, SERIF, WORD_SIZE, 400, true, "fframes");
        let fw = measure(&mut frame, ctx, SERIF, WORD_SIZE, 400, true, "f");
        let trail = 3.0;
        let step = fw * 0.78;
        let total = w + step * trail;
        let x0 = 540.0 - total / 2.0 + step * trail;
        let y = 400.0;
        let word_in = spring(lb * BEAT, 260.0, 20.0);
        let word_s = 1.0 + (1.0 - expo_out(lb / 0.5)) * 0.25;
        let echoes: Vec<Svgr> = (0..3)
            .map(|k| {
                let k = k as f32;
                let s = soft(lb - 0.5 - k * 0.5);
                let dx = -step * (k + 1.0) * s;
                let colors = [ACCENT, "#2563c9", EMBER];
                let o = s.clamp(0.0, 1.0) * (1.0 - k * 0.18);
                fframes::svgr!(
                    <text x={x0 + dx} y={y} font-family={SERIF} font-style="italic" font-size={WORD_SIZE} fill={colors[k as usize]} opacity={o}>"f"</text>
                )
            })
            .rev()
            .collect();
        let tag = prog(lb, 1.5, 2.1);
        let studio = snap(lb - 1.5);

        // the QR code was built by the render matrix of the scene before: it is
        // already here, the corners lock on
        let corner = prog(lb, 0.2, 0.5);
        let scan = prog(lb, 1.0, 1.4);

        // commands
        let rows = [
            (
                "fframes Studio",
                "macOS  /  Windows  /  Linux",
                "THE DESKTOP APP",
            ),
            ("cargo fframes new my-video", "", "OR THE CLI"),
        ];
        let cmd_rows: Vec<Svgr> = rows
            .iter()
            .enumerate()
            .map(|(i, (cmd, extra, note))| {
                let at = 3.0 + i as f32 * 1.0;
                let s = snap(lb - at);
                let ry = 1290.0 + i as f32 * 125.0;
                let extra = (*extra).to_owned();
                fframes::svgr!(
                    <g opacity={prog(lb, at, at + 0.1)} transform={format!("translate(0 {})", (1.0 - s) * 30.0)}>
                        <rect x="80" y={ry - 44.0} width="920" height="110" fill="#0f0e0d" fill-opacity="0.9" stroke="#3d3a35" stroke-width="1.5" />
                        <rect x="112" y={ry - 14.0} width="16" height="16" fill={ACCENT} />
                        <text x="150" y={ry} font-family={MONO} font-weight="500" font-size="32" fill={BONE}>{*cmd}</text>
                        <text x="150" y={ry + 38.0} font-family={MONO} font-weight="500" font-size="22" fill={GREY}>{extra}</text>
                        <text x="968" y={ry - 18.0} text-anchor="end" font-family={MONO} font-weight="500" font-size="15" letter-spacing="2" fill={GREY}>{*note}</text>
                    </g>
                )
            })
            .collect();
        let url = prog(lb, 5.5, 6.0);

        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                <g opacity={1.0 - fade_out}>
                    {echoes}
                    <g transform={format!("translate(540 {y}) scale({word_s}) translate(-540 -{y})")} opacity={word_in.min(1.0)}>
                        <text x={x0} y={y} font-family={SERIF} font-style="italic" font-size={WORD_SIZE} fill={BONE}>"fframes"</text>
                    </g>
                    <g transform={format!("translate(0 {})", (1.0 - studio) * 30.0)} opacity={tag}>
                        <text x={x0 + w} y={y - 170.0} text-anchor="end" font-family={MONO} font-weight="600" font-size="30" letter-spacing="10" fill={ACCENT}>"STUDIO"</text>
                    </g>
                    <text x="540" y="486" text-anchor="middle" font-family={MONO} font-weight="500" font-size="28" letter-spacing="2" fill={BONE} opacity={tag}>"select it. prompt it. ship it."</text>
                    {tile(CX, CY, QR_SIZE)}
                    <g opacity={corner}>
                        {corners(CX - TILE / 2.0 - 16.0, CY - TILE / 2.0 - 16.0, TILE + 32.0, TILE + 32.0, 34.0, ACCENT, 4.0)}
                    </g>
                    <text x="540" y="1186" text-anchor="middle" font-family={MONO} font-weight="600" font-size="22" letter-spacing="5" fill={ACCENT} opacity={scan}>"SCAN TO GET STUDIO"</text>
                    {cmd_rows}
                    <g opacity={url}>
                        <text x="540" y="1590" text-anchor="middle" font-family={MONO} font-weight="600" font-size="29" fill={ACCENT}>"github.com/khanhthanhdev/fframes-desktop"</text>
                    </g>
                </g>
                <rect width="1080" height="1920" fill={BONE} opacity={flash} />
                <rect width="1080" height="1920" fill="#000000" opacity={fade_out} />
            </g>
        )
    }
}
