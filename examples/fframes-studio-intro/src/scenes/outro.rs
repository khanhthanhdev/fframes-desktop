//! The wordmark slams in with its trail of f's, followed by where to get Studio.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use super::parallel::{QR_AT, QR_PX};
use crate::beat::*;
use crate::shaders::SHADERS;
use crate::short::qr::{QR_SIZE, TILE, tile};
use crate::ui::*;

beat_scene!(OutroScene, Some(96.0), Some(112.0));
beat_scene!(EndScene, Some(208.0), None);

const WORD_SIZE: usize = 300;

impl Scene for OutroScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        card(frame, ctx, lb, false)
    }
}

impl Scene for EndScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        card(frame, ctx, lb, true)
    }
}

/// The wordmark card. The last card fades to black with the video; the first
/// one is cut off by the next scene before it matters. With `qr` the card is the
/// end card: text on the left, the QR code the wall of squares just became on the
/// right.
fn card<'a>(mut frame: Frame, ctx: &FFramesContext<'a, '_>, lb: f32, qr: bool) -> Svgr<'a> {
    {
        let t_end = TOTAL_SECONDS - gsec(&frame);
        let fade_out = prog(1.6 - t_end, 0.0, 1.6);
        let flash = (-lb * 9.0).exp();

        let bg = SHADERS.contour.draw(
            &frame,
            ShaderUniforms::new()
                .float2("uWell", 0.5, 0.47)
                .float("uDepth", 1.1)
                .float("uDensity", 18.0)
                .float("uBright", 0.25 + 0.5 * (1.0 - expo_out(prog(lb, 0.0, 6.0))))
                .color("uInk", Color::hex("#8a857c"))
                .color("uHot", Color::hex(ACCENT)),
        );

        let ws = if qr { 200 } else { WORD_SIZE };
        let w = measure(&mut frame, ctx, SERIF, ws, 400, true, "fframes");
        let fw = measure(&mut frame, ctx, SERIF, ws, 400, true, "f");
        // the trail makes the whole mark wider: center mark + trail
        let trail = 3.0;
        let step = fw * 0.78;
        let total = w + step * trail;
        let left = 110.0;
        let mid = if qr { left + total / 2.0 } else { 960.0 };
        let x0 = mid - total / 2.0 + step * trail;
        let y = if qr { 400.0 } else { 470.0 };
        let word_in = spring(lb * BEAT, 260.0, 20.0);
        let word_s = 1.0 + (1.0 - expo_out(lb / 0.5)) * 0.25;

        let echoes: Vec<Svgr> = (0..3)
            .map(|k| {
                let k = k as f32;
                let s = soft(lb - 0.5 - k * 0.5);
                let dx = -step * (k + 1.0) * s;
                let colors = [ACCENT, "#2563c9", EMBER];
                let color = colors[k as usize];
                let o = s.clamp(0.0, 1.0) * (1.0 - k * 0.18);
                fframes::svgr!(
                    <text x={x0 + dx} y={y} font-family={SERIF} font-style="italic" font-size={ws} fill={color} opacity={o}>"f"</text>
                )
            })
            .rev()
            .collect();

        let (tag_x, tag_y) = if qr { (x0 + w, 250.0) } else { (1470.0, 215.0) };
        let (line_x, line_y, line_anchor) = if qr {
            (left, 476.0, "start")
        } else {
            (960.0, 580.0, "middle")
        };
        let tag = prog(lb, 3.0, 3.6);
        let studio = snap(lb - 3.0);
        let rows = [
            (
                "fframes Studio",
                "macOS  ·  Windows  ·  Linux",
                "THE DESKTOP APP",
            ),
            ("cargo fframes new my-video", "", "OR THE CLI"),
        ];
        let cmd_rows: Vec<Svgr> = rows
            .iter()
            .enumerate()
            .map(|(i, (cmd, extra, note))| {
                let at = if qr { 4.0 } else { 8.0 } + i as f32 * 1.0;
                let s = snap(lb - at);
                let ry = if qr { 640.0 } else { 790.0 } + i as f32 * 78.0;
                let (rx, rw, ex, nx) = if qr {
                    (left, 940.0, 480.0, 1030.0)
                } else {
                    (400.0, 1120.0, 1000.0, 1500.0)
                };
                let extra = (*extra).to_owned();
                fframes::svgr!(
                    <g opacity={prog(lb, at, at + 0.1)} transform={format!("translate(0 {})", (1.0 - s) * 30.0)}>
                        <rect x={rx} y={ry - 44.0} width={rw} height="62" fill="#0f0e0d" fill-opacity="0.9" stroke="#3d3a35" stroke-width="1.5" />
                        <rect x={rx + 30.0} y={ry - 22.0} width="16" height="16" fill={ACCENT} />
                        <text x={rx + 68.0} y={ry} font-family={MONO} font-weight="500" font-size="26" fill={BONE}>{*cmd}</text>
                        <text x={ex} y={ry} font-family={MONO} font-weight="500" font-size="22" fill={GREY}>{extra}</text>
                        <text x={nx} y={ry} text-anchor="end" font-family={MONO} font-weight="500" font-size="15" letter-spacing="2" fill={GREY}>{*note}</text>
                    </g>
                )
            })
            .collect();
        let url = prog(lb, if qr { 6.0 } else { 12.0 }, if qr { 6.5 } else { 12.5 });
        let (url_x, url_y, url_anchor) = if qr {
            (left, 880.0, "start")
        } else {
            (960.0, 990.0, "middle")
        };
        // the code was built by the wall of squares of the scene before: it is
        // already here, the corners lock on
        let code = if qr {
            let (qx, qy) = QR_AT;
            let half = TILE * QR_PX / QR_SIZE / 2.0 + 16.0;
            fframes::svgr!(
                <g>
                    {tile(qx, qy, QR_PX)}
                    <g opacity={prog(lb, 0.2, 0.5)}>
                        {corners(qx - half, qy - half, half * 2.0, half * 2.0, 34.0, ACCENT, 4.0)}
                    </g>
                    <text x={qx} y={qy + half + 56.0} text-anchor="middle" font-family={MONO} font-weight="600" font-size="22" letter-spacing="5" fill={ACCENT} opacity={prog(lb, 1.0, 1.4)}>"SCAN TO GET STUDIO"</text>
                </g>
            )
        } else {
            Svgr::empty()
        };

        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1920" height="1080" />
                <g opacity={1.0 - fade_out}>
                    {echoes}
                    <g transform={format!("translate({mid} {y}) scale({word_s}) translate(-{mid} -{y})")} opacity={word_in.min(1.0)}>
                        <text x={x0} y={y} font-family={SERIF} font-style="italic" font-size={ws} fill={BONE}>"fframes"</text>
                    </g>
                    <g transform={format!("translate(0 {})", (1.0 - studio) * 30.0)} opacity={tag}>
                        <text x={tag_x} y={tag_y} text-anchor="end" font-family={MONO} font-weight="600" font-size="34" letter-spacing="10" fill={ACCENT}>"STUDIO"</text>
                    </g>
                    <text x={line_x} y={line_y} text-anchor={line_anchor} font-family={MONO} font-weight="500" font-size="30" letter-spacing="2" fill={BONE} opacity={tag}>"select it.  prompt it.  ship it."</text>
                    {code}
                    {cmd_rows}
                    <g opacity={url}>
                        <text x={url_x} y={url_y} text-anchor={url_anchor} font-family={MONO} font-weight="600" font-size="26" letter-spacing="3" fill={ACCENT}>"GITHUB.COM/KHANHTHANHDEV/FFRAMES-DESKTOP"</text>
                    </g>
                </g>
                <rect width="1920" height="1080" fill={BONE} opacity={flash} />
                <rect width="1920" height="1080" fill="#000000" opacity={fade_out} />
            </g>
        )
    }
}
