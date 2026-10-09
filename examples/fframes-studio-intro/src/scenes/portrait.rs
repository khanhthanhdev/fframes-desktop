//! The 9:16 cut (1080x1920) for social media: the same beats, sound and story
//! as the landscape video, recomposed for a phone. Text stacks, cards become
//! rows and the Studio window is shot with a closer camera.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use super::install::{self, STEPS};
use super::studio;
use crate::beat::*;
use crate::shaders::SHADERS;
use crate::ui::*;

pub const WIDTH: usize = 1080;
pub const HEIGHT: usize = 1920;

// ---------------------------------------------------------------------------
// cold open

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
        let caret_x = 150.0
            + if on_second {
                (typed - split) as f32
            } else {
                typed as f32
            } * adv;
        let caret_y = if on_second { 962.0 } else { 872.0 };
        let after = t - enter_at;
        let lift = if after > 0.0 {
            -expo_out(after / 0.25) * 300.0
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
                .float2("uWell", 0.7, 0.55)
                .float("uDepth", 0.6)
                .float("uDensity", 12.0)
                .float("uBright", 0.35 + enter_flash * 0.4)
                .color("uInk", Color::hex("#8a857c"))
                .color("uHot", Color::hex(ACCENT)),
        );
        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                <g transform={format!("translate(0 {lift})")}>
                    <text x="80" y="716" font-family={MONO} font-weight="500" font-size="22" letter-spacing="2" fill={GREY}>"FFRAMES STUDIO"</text>
                    <text x="1000" y="716" text-anchor="end" font-family={MONO} font-weight="500" font-size="22" letter-spacing="2" fill={GREY}>"DESKTOP APP"</text>
                    <rect x="60" y="750" width="960" height="320" rx="18" fill="#0f0e0d" fill-opacity="0.85"
                          stroke={if after > 0.0 { ACCENT } else { "#4a4640" }} stroke-width="2" />
                    <text x="96" y="872" font-family={MONO} font-weight="600" font-size="64" fill={ACCENT}>"›"</text>
                    <text x="150" y="872" font-family={MONO} font-weight="500" font-size="64" fill={BONE}>{line1}</text>
                    <text x="150" y="962" font-family={MONO} font-weight="500" font-size="64" fill={ACCENT}>{line2}</text>
                    <rect x={caret_x + 4.0} y={caret_y - 50.0} width="32" height="64" fill={ACCENT} opacity={if caret_on && after <= 0.0 { 1.0 } else { 0.0 }} />
                </g>
            </g>
        )
    }
}

// ---------------------------------------------------------------------------
// install

beat_scene!(InstallScene, Some(0.0), Some(16.0));

const I_CARD_W: f32 = 920.0;
const I_CARD_H: f32 = 250.0;

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
                .float("uHorizon", 0.4)
                .float("uBright", 0.22)
                .color("uInk", Color::hex("#6f6a63"))
                .color("uHot", Color::hex(ACCENT)),
        );
        let head = Slam::new(80.0, 330.0, "Install it.", DISPLAY, 116.0, BONE);
        let sub = Slam::new(80.0, 450.0, "like any app.", SERIF, 112.0, ACCENT).italic();
        let cards: Vec<Svgr> = STEPS
            .iter()
            .enumerate()
            .map(|(i, step)| {
                let x = 80.0;
                let y = 640.0 + i as f32 * (I_CARD_H + 24.0);
                let at = 3.0 + i as f32 * 0.6;
                let s = snap(lb - at);
                let work = prog(lb, step.from, step.to);
                let done = work >= 1.0;
                let detail = match i {
                    0 => install::platforms(lb),
                    1 => install::agents(lb),
                    _ => install::sdk(work, I_CARD_W - 72.0),
                };
                let status = if done {
                    "READY"
                } else if work > 0.0 {
                    "WORKING"
                } else {
                    "WAITING"
                };
                let stroke = if done {
                    "#6fcf8a"
                } else if work > 0.0 {
                    ACCENT
                } else {
                    "#3d3a35"
                };
                fframes::svgr!(
                    <g opacity={prog(lb, at, at + 0.15)} transform={format!("translate(0 {})", (1.0 - s) * 60.0)}>
                        <rect x={x} y={y} width={I_CARD_W} height={I_CARD_H} fill="#0f0e0d" fill-opacity="0.92" stroke={stroke} stroke-width="2" />
                        <text x={x + 36.0} y={y + 56.0} font-family={MONO} font-weight="600" font-size="24" letter-spacing="3" fill={ACCENT}>{step.num}</text>
                        <text x={x + I_CARD_W - 36.0} y={y + 56.0} text-anchor="end" font-family={MONO} font-weight="500" font-size="20" letter-spacing="3" fill={if done { "#6fcf8a" } else { GREY }}>{status}</text>
                        <text x={x + 36.0} y={y + 118.0} font-family={DISPLAY} font-size="46" letter-spacing="-1" fill={BONE}>{step.title}</text>
                        <g transform={format!("translate({} {})", x + 36.0, y + 168.0)}>{detail}</g>
                        {install::bar(x + 36.0, y + I_CARD_H - 40.0, I_CARD_W - 72.0, work)}
                        {install::check(x + I_CARD_W - 76.0, y + 78.0, prog(lb, step.to, step.to + 0.5))}
                    </g>
                )
            })
            .collect();
        let tag = snap(lb - 13.0);
        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                {head.draw(lb, -120.0, 0.0)}
                {sub.draw(lb - 1.0, 120.0, 0.0)}
                {cards}
                <g opacity={prog(lb, 13.0, 13.3)} transform={format!("translate(0 {})", (1.0 - tag) * 24.0)}>
                    <text x="80" y="1600" font-family={MONO} font-weight="600" font-size="30" letter-spacing="3" fill={BONE}>"NO TERMINAL."</text>
                    <text x="80" y="1646" font-family={MONO} font-weight="600" font-size="30" letter-spacing="3" fill={BONE}>"NO SETUP SCRIPTS."</text>
                </g>
            </g>
        )
    }
}

// ---------------------------------------------------------------------------
// studio demo

beat_scene!(StudioScene, Some(16.0), Some(80.0));

/// Camera path for a phone: closer than the landscape one, one region at a time.
const CAMERA: &[(f32, f32, f32, f32)] = &[
    (0.0, 800.0, 370.0, 0.58),
    (6.0, 800.0, 370.0, 0.64),
    (7.0, 800.0, 370.0, 0.64),
    (10.0, 675.0, 300.0, 1.2),
    (13.0, 675.0, 300.0, 1.2),
    (15.5, 1350.0, 470.0, 1.9),
    (31.0, 1350.0, 470.0, 1.9),
    (33.0, 580.0, 350.0, 1.18),
    (49.0, 580.0, 350.0, 1.18),
    (52.0, 675.0, 300.0, 1.2),
    (54.5, 675.0, 300.0, 1.2),
    (56.0, 1350.0, 520.0, 1.9),
    (59.0, 1350.0, 520.0, 1.9),
    (60.6, 1240.0, 170.0, 1.5),
    (64.0, 1240.0, 170.0, 1.5),
];

/// (start beat, tag, two headline lines)
const CAPTIONS: &[(f32, &str, &str, &str)] = &[
    (0.0, "01 / WORKSPACE", "One window.", "Your whole video."),
    (7.0, "02 / SELECT", "Click anything", "on the canvas."),
    (14.0, "03 / PROMPT", "Say what should", "change."),
    (32.0, "04 / RETRIEVE", "The agent finds", "the exact code."),
    (44.0, "05 / EDIT", "A real diff", "in your Rust."),
    (52.0, "06 / PREVIEW", "Rebuilt, checked,", "playing."),
    (60.0, "07 / EXPORT", "Happy? Export", "an MP4."),
];

fn captions(lb: f32) -> Svgr<'static> {
    let items: Vec<Svgr> = CAPTIONS
        .iter()
        .enumerate()
        .filter_map(|(i, (start, tag, l1, l2))| {
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
                    <text x="80" y="190" font-family={MONO} font-weight="600" font-size="24" letter-spacing="4" fill={ACCENT}>{*tag}</text>
                    <text x="80" y="290" font-family={DISPLAY} font-size="72" letter-spacing="-2" fill={BONE}>{*l1}</text>
                    <text x="80" y="376" font-family={DISPLAY} font-size="72" letter-spacing="-2" fill={BONE}>{*l2}</text>
                </g>
            ))
        })
        .collect();
    fframes::svgr!(<g>{items}</g>)
}

impl Scene for StudioScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, mut frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let (cx, cy, s) = studio::camera_in(CAMERA, lb);
        let bg = SHADERS.contour.draw(
            &frame,
            ShaderUniforms::new()
                .float2("uWell", 0.5, 0.8)
                .float("uDepth", 0.5)
                .float("uDensity", 12.0)
                .float("uBright", 0.14 + (-(lb - 32.0).abs() * 0.8).exp() * 0.12)
                .color("uInk", Color::hex("#8a857c"))
                .color("uHot", Color::hex(ACCENT)),
        );
        let win = studio::window(&mut frame, ctx, lb);
        let enter = prog(lb, 0.0, 0.8);
        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                {captions(lb)}
                <clipPath id="viewport"><rect x="0" y="430" width="1080" height="1180" /></clipPath>
                <g clip-path="url(#viewport)" opacity={enter}>
                    <g transform={format!("translate(540 1020) scale({s}) translate({} {})", -cx, -cy)}>{win}</g>
                </g>
            </g>
        )
    }
}

// ---------------------------------------------------------------------------
// project

beat_scene!(ProjectScene, Some(80.0), Some(96.0));

const CARDS: [(&str, &str, [&str; 2]); 3] = [
    (
        "01 / AGENTS",
        "Bring your agent",
        ["Codex or Claude Code.", "Switch mid-task, context follows."],
    ),
    (
        "02 / HISTORY",
        "Undo any edit",
        ["Every change is a revision.", "Compare, restore, export."],
    ),
    (
        "03 / STYLE",
        "Style presets",
        [
            "Pick a look for the project.",
            "The agent adapts the scenes.",
        ],
    ),
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
                .float("uHorizon", 0.4)
                .float("uBright", 0.3)
                .color("uInk", Color::hex("#6f6a63"))
                .color("uHot", Color::hex(ACCENT)),
        );
        let h1 = Slam::new(80.0, 310.0, "It's just", DISPLAY, 130.0, BONE);
        let h2 = Slam::new(80.0, 450.0, "Rust.", DISPLAY, 130.0, ACCENT);
        let sub = prog(lb, 1.2, 1.6);
        let cards: Vec<Svgr> = CARDS
            .iter()
            .enumerate()
            .map(|(i, (tag, title, lines))| {
                let y = 760.0 + i as f32 * 276.0;
                let at = 3.0 + i as f32 * 3.0;
                let s = snap(lb - at);
                fframes::svgr!(
                    <g opacity={prog(lb, at, at + 0.15)} transform={format!("translate(0 {})", (1.0 - s) * 60.0)}>
                        <rect x="80" y={y} width="920" height="252" fill="#0f0e0d" fill-opacity="0.92" stroke="#3d3a35" stroke-width="2" />
                        <rect x="80" y={y} width={(920.0 * s.clamp(0.0, 1.0)).max(0.5)} height="4" fill={ACCENT} />
                        <text x="116" y={y + 56.0} font-family={MONO} font-weight="600" font-size="22" letter-spacing="3" fill={ACCENT}>{*tag}</text>
                        <text x="116" y={y + 124.0} font-family={DISPLAY} font-size="50" letter-spacing="-1" fill={BONE}>{*title}</text>
                        <text x="116" y={y + 178.0} font-family={MONO} font-weight="500" font-size="26" fill="#b9b4ab">{lines[0]}</text>
                        <text x="116" y={y + 216.0} font-family={MONO} font-weight="500" font-size="26" fill="#b9b4ab">{lines[1]}</text>
                    </g>
                )
            })
            .collect();
        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                {h1.draw(lb, -140.0, 0.0)}
                {h2.draw(lb - 0.5, -140.0, 0.0)}
                <g opacity={sub}>
                    <text x="84" y="530" font-family={MONO} font-weight="500" font-size="28" letter-spacing="1" fill={GREY}>"An agent edits an ordinary"</text>
                    <text x="84" y="572" font-family={MONO} font-weight="500" font-size="28" letter-spacing="1" fill={GREY}>"fframes project. Open it in any"</text>
                    <text x="84" y="614" font-family={MONO} font-weight="500" font-size="28" letter-spacing="1" fill={GREY}>"editor. Render it with the CLI."</text>
                </g>
                {cards}
            </g>
        )
    }
}

// ---------------------------------------------------------------------------
// outro

beat_scene!(OutroScene, Some(96.0), Some(112.0));
beat_scene!(EndScene, Some(208.0), None);

const WORD_SIZE: usize = 200;

impl Scene for OutroScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        card(frame, ctx, lb)
    }
}

impl Scene for EndScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        card(frame, ctx, lb)
    }
}

fn card<'a>(mut frame: Frame, ctx: &FFramesContext<'a, '_>, lb: f32) -> Svgr<'a> {
    {
        let t_end = TOTAL_SECONDS - gsec(&frame);
        let fade_out = prog(1.6 - t_end, 0.0, 1.6);
        let flash = (-lb * 9.0).exp();
        let bg = SHADERS.contour.draw(
            &frame,
            ShaderUniforms::new()
                .float2("uWell", 0.5, 0.4)
                .float("uDepth", 1.1)
                .float("uDensity", 14.0)
                .float("uBright", 0.25 + 0.5 * (1.0 - expo_out(prog(lb, 0.0, 6.0))))
                .color("uInk", Color::hex("#8a857c"))
                .color("uHot", Color::hex(ACCENT)),
        );
        let w = measure(&mut frame, ctx, SERIF, WORD_SIZE, 400, true, "fframes");
        let fw = measure(&mut frame, ctx, SERIF, WORD_SIZE, 400, true, "f");
        let trail = 3.0;
        let step = fw * 0.78;
        let total = w + step * trail;
        let x0 = 540.0 - total / 2.0 + step * trail;
        let y = 760.0;
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
                let at = 8.0 + i as f32 * 1.0;
                let s = snap(lb - at);
                let ry = 1090.0 + i as f32 * 150.0;
                let extra = (*extra).to_owned();
                fframes::svgr!(
                    <g opacity={prog(lb, at, at + 0.1)} transform={format!("translate(0 {})", (1.0 - s) * 30.0)}>
                        <rect x="80" y={ry - 52.0} width="920" height="124" fill="#0f0e0d" fill-opacity="0.9" stroke="#3d3a35" stroke-width="1.5" />
                        <rect x="112" y={ry - 18.0} width="16" height="16" fill={ACCENT} />
                        <text x="150" y={ry} font-family={MONO} font-weight="500" font-size="30" fill={BONE}>{*cmd}</text>
                        <text x="150" y={ry + 40.0} font-family={MONO} font-weight="500" font-size="22" fill={GREY}>{extra}</text>
                        <text x="968" y={ry - 16.0} text-anchor="end" font-family={MONO} font-weight="500" font-size="15" letter-spacing="2" fill={GREY}>{*note}</text>
                    </g>
                )
            })
            .collect();
        let url = prog(lb, 12.0, 12.5);
        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                <g opacity={1.0 - fade_out}>
                    {echoes}
                    <g transform={format!("translate(540 {y}) scale({word_s}) translate(-540 -{y})")} opacity={word_in.min(1.0)}>
                        <text x={x0} y={y} font-family={SERIF} font-style="italic" font-size={WORD_SIZE} fill={BONE}>"fframes"</text>
                    </g>
                    <g transform={format!("translate(0 {})", (1.0 - studio) * 30.0)} opacity={tag}>
                        <text x={x0 + w} y={y - 190.0} text-anchor="end" font-family={MONO} font-weight="600" font-size="34" letter-spacing="10" fill={ACCENT}>"STUDIO"</text>
                    </g>
                    <text x="540" y="890" text-anchor="middle" font-family={MONO} font-weight="500" font-size="30" letter-spacing="2" fill={BONE} opacity={tag}>"select it. prompt it. ship it."</text>
                    {cmd_rows}
                    <g opacity={url}>
                        <text x="540" y="1480" text-anchor="middle" font-family={MONO} font-weight="600" font-size="24" letter-spacing="3" fill={ACCENT}>"GITHUB.COM/KHANHTHANHDEV/FFRAMES-DESKTOP"</text>
                    </g>
                </g>
                <rect width="1080" height="1920" fill={BONE} opacity={flash} />
                <rect width="1080" height="1920" fill="#000000" opacity={fade_out} />
            </g>
        )
    }
}

// ---------------------------------------------------------------------------
// the story after the demo: the same scenes as the landscape cut, laid out for a phone

beat_scene!(HowScene, Some(112.0), Some(144.0));
beat_scene!(FastScene, Some(144.0), Some(176.0));
beat_scene!(WhyScene, Some(176.0), Some(208.0));

fn story_bg(frame: &Frame, bright: f32) -> Svgr<'static> {
    let g = SHADERS.grid.draw(
        frame,
        ShaderUniforms::new()
            .float("uSpeed", 1.0)
            .float("uHorizon", 0.4)
            .float("uBright", bright)
            .color("uInk", Color::hex("#6f6a63"))
            .color("uHot", Color::hex(ACCENT)),
    );
    fframes::svgr!(<image href={g.href()} x="0" y="0" width="1080" height="1920" />)
}

impl Scene for HowScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }
    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        fframes::svgr!(<g>{story_bg(&frame, 0.2)}{super::story::how(lb, true)}</g>)
    }
}

impl Scene for FastScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }
    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        super::story::fast_scene(&frame, Self::lb(&frame), true)
    }
}

impl Scene for WhyScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }
    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        fframes::svgr!(<g>{story_bg(&frame, 0.28)}{super::story::why(lb, true)}</g>)
    }
}
