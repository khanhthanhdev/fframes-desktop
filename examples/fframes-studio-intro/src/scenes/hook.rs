//! Cold open: the two verbs of the product, typed into a prompt box.

use fframes::{Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use crate::beat::*;
use crate::shaders::SHADERS;
use crate::ui::*;

beat_scene!(HookScene, None, Some(0.0));

const PROMPT: &str = "select it. prompt it.";

impl Scene for HookScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let t = gsec(&frame);
        let enter_at = beat_time(-1.0);
        let chars = PROMPT.chars().count();
        let typed = ((6.0 + (t - 0.02) * 24.0) as usize).min(chars);
        let shown: String = PROMPT.chars().take(typed).collect();
        let done = typed >= chars;
        let caret_on = !done || (t * 3.0).fract() < 0.55;
        let split = PROMPT.find(". ").map_or(chars, |i| i + 2);
        let line1: String = shown.chars().take(split).collect();
        let line2: String = shown.chars().skip(split).collect();
        let adv = 64.0 * 0.6;
        let on_second = typed > split;
        let caret_x = 240.0
            + if on_second {
                (typed - split) as f32
            } else {
                typed as f32
            } * adv;
        let caret_y = if on_second { 612.0 } else { 522.0 };

        let after = t - enter_at;
        let lift = if after > 0.0 {
            -expo_out(after / 0.25) * 220.0
        } else {
            0.0
        };
        let enter_flash = if after > 0.0 {
            (-after * 9.0).exp()
        } else {
            0.0
        };

        let bg = SHADERS.contour.draw(
            &frame,
            ShaderUniforms::new()
                .float2("uWell", 0.74, 0.62)
                .float("uDepth", 0.6)
                .float("uDensity", 16.0)
                .float("uBright", 0.35 + enter_flash * 0.4)
                .color("uInk", fframes::Color::hex("#8a857c"))
                .color("uHot", fframes::Color::hex(ACCENT)),
        );

        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1920" height="1080" />
                <g transform={format!("translate(0 {lift})")}>
                    <text x="160" y="366" font-family={MONO} font-weight="500" font-size="22" letter-spacing="2" fill={GREY}>"FFRAMES STUDIO  ·  vinuni-tech-day"</text>
                    <text x="1760" y="366" text-anchor="end" font-family={MONO} font-weight="500" font-size="28" letter-spacing="2" fill={GREY}>{RELEASE_LABEL}</text>
                    <rect x="140" y="400" width="1640" height="270" rx="18" fill="#0f0e0d" fill-opacity="0.85"
                          stroke={if after > 0.0 { ACCENT } else { "#4a4640" }} stroke-width="2" />
                    <text x="180" y="522" font-family={MONO} font-weight="600" font-size="64" fill={ACCENT}>"›"</text>
                    <text x="240" y="522" font-family={MONO} font-weight="500" font-size="64" fill={BONE}>{line1}</text>
                    <text x="240" y="612" font-family={MONO} font-weight="500" font-size="64" fill={ACCENT}>{line2}</text>
                    <rect x={caret_x + 4.0} y={caret_y - 50.0} width="32" height="64" fill={ACCENT} opacity={if caret_on && after <= 0.0 { 1.0 } else { 0.0 }} />
                </g>
            </g>
        )
    }
}

/// Label shown top right of the cold open.
pub const RELEASE_LABEL: &str = "DESKTOP APP";
