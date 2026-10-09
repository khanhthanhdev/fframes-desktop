//! A 57 second introduction to fframes Studio, made with fframes.
//!
//! The desktop app in one pass: install it, select something on the canvas,
//! prompt the agent, watch it retrieve the code and edit it, export. Everything
//! sits on the beat grid of the soundtrack of `examples/fframes-intro` (see
//! `beat.rs`). `cargo run --release -p fframes-studio-intro -- timeline` lists
//! the scenes with their bars.

pub mod beat;
pub mod facts;
pub mod scenes;
pub mod shaders;
pub mod short;
pub mod ui;
pub mod wall;

use fframes::{
    AudioMap, AudioTimestamp::*, AudioTrack, Color, Duration, FFramesContext, Frame, Scene, Scenes,
    ShaderUniforms, Svgr, Video, include_media_dir,
};

use beat::*;
use scenes::*;
use shaders::SHADERS;
use ui::*;

include_media_dir!(pub struct StudioIntroMedia, "examples/fframes-studio-intro/media");

pub const WIDTH: usize = 1920;
pub const HEIGHT: usize = 1080;

#[derive(Debug)]
pub struct StudioIntroVideo;

/// Section titles for the HUD, by the beat they start on.
const SECTIONS: &[(f32, &str)] = &[
    (-99.0, "00 / HOOK"),
    (0.0, "01 / INSTALL"),
    (16.0, "02 / WORKSPACE"),
    (22.0, "03 / SELECT"),
    (30.0, "04 / PROMPT"),
    (48.0, "05 / RETRIEVE"),
    (60.0, "06 / EDIT"),
    (68.0, "07 / PREVIEW"),
    (76.0, "08 / EXPORT"),
    (80.0, "09 / PROJECT"),
    (96.0, "10 / STUDIO"),
    (112.0, "11 / HOW IT WORKS"),
    (144.0, "12 / SPEED"),
    (176.0, "13 / WHY IT IS FAST"),
    (208.0, "14 / FFRAMES STUDIO"),
];

/// Section titles of the 16:9 cut, which starts with the first run of the app and
/// ends on the wall of squares that becomes the QR code.
const SECTIONS_LONG: &[(f32, &str)] = &[
    (-99.0, "00 / HOOK"),
    (0.0, "01 / INSTALL"),
    (16.0, "02 / NEW VIDEO"),
    (26.4, "03 / GENERATE"),
    (33.5, "04 / SELECT"),
    (36.5, "05 / PROMPT"),
    (48.0, "06 / RETRIEVE"),
    (60.0, "07 / EDIT"),
    (68.0, "08 / PREVIEW"),
    (76.0, "09 / EXPORT"),
    (80.0, "10 / PROJECT"),
    (96.0, "11 / STUDIO"),
    (112.0, "12 / HOW IT WORKS"),
    (144.0, "13 / SPEED"),
    (176.0, "14 / WHY IT IS FAST"),
    (198.0, "15 / EVERY CORE"),
    (208.0, "16 / FFRAMES STUDIO"),
];

/// Sound effects, placed on the beat minus the file's attack.
fn sfx(name: &'static str, at: f32, gain: f32) -> AudioTrack<'static> {
    AudioTrack::new(name, Second(at.max(0.0))..Eof).gain_db(gain)
}

impl Video for StudioIntroVideo {
    const FPS: usize = FPS;
    const WIDTH: usize = WIDTH;
    const HEIGHT: usize = HEIGHT;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn duration(&self) -> Duration<'_> {
        Duration::Auto
    }

    fn audio(&self) -> AudioMap<'_> {
        audio_map_long()
    }

    fn define_scenes(&self) -> Scenes<'_> {
        Scenes::from(vec![
            &HookScene as &dyn Scene,
            &InstallScene,
            &StudioScene,
            &ProjectScene,
            &OutroScene,
            &HowScene,
            &FastScene,
            &WhyScene,
            &ParallelScene,
            &EndScene,
        ])
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let b = gbeat(&frame);
        let grain = SHADERS.grain.draw(
            &frame,
            ShaderUniforms::new()
                .float("uGrain", 0.07)
                .float("uVignette", 0.55),
        );
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" width="1920" height="1080" viewBox="0 0 1920 1080">
                <rect width="1920" height="1080" fill={BG} />
                {energy(&frame, ctx, b)}
                <image href={grain.href()} x="0" y="0" width="1920" height="1080" />
                {hud(&frame, b)}
            </svg>
        )
    }
}

/// The soundtrack and sound effects of the 16:9 cut.
fn audio_map_long() -> AudioMap<'static> {
    let mut tracks = vec![
        AudioTrack::new("music.wav", Second(0.0)..Second(TOTAL_SECONDS)).fade_out(1.8),
        sfx("sfx_typing.mp3", 0.02, -15.0),
        sfx("sfx_enter.mp3", beat_time(-1.0) - 0.067, -9.0),
        sfx("sfx_glitch.mp3", beat_time(16.0) - 0.2, -8.0),
        // the first run: title, description, Generate
        sfx("sfx_typing.mp3", beat_time(17.8), -17.0),
        sfx("sfx_typing.mp3", beat_time(20.5), -17.0),
        sfx("sfx_typing.mp3", beat_time(24.1), -17.0),
        sfx("sfx_enter.mp3", beat_time(26.0) - 0.067, -9.0),
        // the prompt is typed after the selection, then sent
        sfx("sfx_typing.mp3", beat_time(studio::RUN_TYPE_START), -17.0),
        sfx(
            "sfx_typing.mp3",
            beat_time(studio::RUN_TYPE_START + 3.5),
            -17.0,
        ),
        sfx(
            "sfx_typing.mp3",
            beat_time(studio::RUN_TYPE_START + 7.0),
            -17.0,
        ),
        sfx("sfx_enter.mp3", beat_time(studio::SEND_AT) - 0.067, -9.0),
        sfx("sfx_impact.mp3", beat_time(48.0), -10.0),
        sfx("sfx_shutter.mp3", beat_time(studio::RESULT_AT) - 0.02, -4.0),
        sfx("sfx_impact.mp3", beat_time(96.0), -10.0),
        sfx("sfx_braam.mp3", beat_time(96.0), -9.0),
        sfx("sfx_impact.mp3", beat_time(144.0), -12.0),
        sfx("sfx_impact.mp3", beat_time(208.0), -11.0),
        sfx("sfx_braam.mp3", beat_time(208.0), -9.0),
    ];
    for b in [16.0, 80.0, 96.0, 112.0, 144.0, 176.0, 198.0, 208.0] {
        tracks.push(sfx("sfx_whoosh.mp3", beat_time(b) - 0.18, -15.0));
    }
    for b in install::BLIP_BEATS
        .iter()
        .chain(studio::RUN_BLIP_BEATS)
        .chain(project::BLIP_BEATS)
        .chain(&[114.0, 120.0, 126.0, 132.0, 138.0])
        .chain(story::WHY_BLIP_BEATS)
        .chain(parallel::BLIP_BEATS)
    {
        tracks.push(sfx("sfx_blip.mp3", beat_time(*b), -9.0));
    }
    AudioMap::from(tracks)
}

/// The soundtrack and sound effects, shared by the landscape and portrait cuts.
fn audio_map() -> AudioMap<'static> {
    {
        let mut tracks = vec![
            AudioTrack::new("music.wav", Second(0.0)..Second(TOTAL_SECONDS)).fade_out(1.8),
            sfx("sfx_typing.mp3", 0.02, -15.0),
            sfx("sfx_enter.mp3", beat_time(-1.0) - 0.067, -9.0),
            sfx("sfx_glitch.mp3", beat_time(16.0) - 0.2, -8.0),
            // the prompt is typed over the second half of the select scene, then sent
            sfx("sfx_typing.mp3", beat_time(studio::TYPE_START), -17.0),
            sfx("sfx_typing.mp3", beat_time(studio::TYPE_START + 3.5), -17.0),
            sfx("sfx_enter.mp3", beat_time(studio::SEND_AT) - 0.067, -9.0),
            sfx("sfx_impact.mp3", beat_time(48.0), -10.0),
            sfx("sfx_shutter.mp3", beat_time(studio::RESULT_AT) - 0.02, -4.0),
            sfx("sfx_impact.mp3", beat_time(96.0), -10.0),
            sfx("sfx_braam.mp3", beat_time(96.0), -9.0),
            sfx("sfx_impact.mp3", beat_time(144.0), -12.0),
            sfx("sfx_impact.mp3", beat_time(208.0), -11.0),
            sfx("sfx_braam.mp3", beat_time(208.0), -9.0),
        ];
        for b in [16.0, 80.0, 96.0, 112.0, 144.0, 176.0, 208.0] {
            tracks.push(sfx("sfx_whoosh.mp3", beat_time(b) - 0.18, -15.0));
        }
        for b in install::BLIP_BEATS
            .iter()
            .chain(studio::BLIP_BEATS)
            .chain(project::BLIP_BEATS)
            .chain(story::BLIP_BEATS)
        {
            tracks.push(sfx("sfx_blip.mp3", beat_time(*b), -9.0));
        }
        AudioMap::from(tracks)
    }
}

fn hud(frame: &Frame, b: f32) -> Svgr<'static> {
    let appear = prog(gsec(frame), 0.1, 0.5);
    if appear <= 0.0 {
        return Svgr::empty();
    }
    let section = SECTIONS_LONG
        .iter()
        .rev()
        .find(|(start, _)| b >= *start)
        .map_or("", |(_, name)| *name);
    let bar = if b < 0.0 {
        0
    } else {
        (b / 4.0).floor() as i32 + 1
    };
    let beat_in_bar = if b < 0.0 {
        0
    } else {
        (b.rem_euclid(4.0)).floor() as i32 + 1
    };
    let tc = timecode(gsec(frame));
    let fnum = format!("F {:05}", frame.global_index);
    let bpm = format!("132 BPM   BAR {bar:03}.{beat_in_bar}");
    // a tick that flashes on every beat
    let tick = if b < 0.0 { 0.0 } else { pulse(b, 7.0) };
    let progress = (gsec(frame) / TOTAL_SECONDS * 1788.0).max(0.5);
    fframes::svgr!(
        <g opacity={appear * 0.9} style="mix-blend-mode:difference">
            {corners(36.0, 36.0, 1848.0, 1008.0, 26.0, "#d8d4cc", 2.0)}
            {label(66.0, 78.0, "FFRAMES STUDIO".to_owned(), "#d8d4cc", 17.0, "start")}
            {label(1854.0, 78.0, bpm, "#d8d4cc", 17.0, "end")}
            <rect x="1840" y="92" width="14" height="14" fill={ACCENT} opacity={0.25 + tick * 0.75} />
            {label(66.0, 1018.0, tc, "#d8d4cc", 17.0, "start")}
            {label(290.0, 1018.0, fnum, "#77736d", 17.0, "start")}
            {label(1854.0, 1018.0, section.to_owned(), "#d8d4cc", 17.0, "end")}
            <rect x="66" y="1034" width="1788" height="1" fill="#d8d4cc" opacity="0.25" />
            <rect x="66" y="1033" width={progress} height="3" fill="#d8d4cc" />
        </g>
    )
}

/// Sections where the full beat plays: the picture pumps with the kick.
const PUMPING: &[(f32, f32)] = &[(48.0, 80.0), (96.0, 160.0), (208.0, 232.0)];
/// Cuts that glitch for a few frames.
const GLITCH_CUTS: &[f32] = &[16.0, 80.0, 96.0, 112.0, 144.0, 176.0, 208.0];
/// The cuts of the 16:9 cut: not into the end card, a glitch there would shear the
/// QR code.
const GLITCH_CUTS_LONG: &[f32] = &[16.0, 80.0, 96.0, 112.0, 144.0, 176.0, 198.0];
/// Drops that flash.
const DROPS: &[f32] = &[48.0, 96.0, 144.0, 208.0];

fn energy<'a>(frame: &Frame, ctx: &FFramesContext<'a, '_>, b: f32) -> Svgr<'a> {
    let pumping = PUMPING.iter().any(|(s, e)| (*s..*e).contains(&b));
    let pump = if pumping { pulse(b, 9.0) } else { 0.0 };
    let s = 1.0 + pump * 0.006;
    let flash = DROPS
        .iter()
        .map(|d| if b >= *d { (-(b - d) * 7.0).exp() } else { 0.0 })
        .fold(0.0_f32, f32::max);

    // a cut glitches for 3 frames: the picture splits in bands pushed sideways
    let since_cut = GLITCH_CUTS_LONG
        .iter()
        .map(|c| (b - c) * BEAT * FPS as f32)
        .find(|f| *f >= 0.0 && *f < 3.0);
    let scene = if let Some(f) = since_cut {
        let bands: Vec<Svgr> = (0..6)
            .map(|i| {
                let y = i as f32 * 180.0;
                let dx = (hash(i as f32 * 13.0 + f.floor() * 7.0) - 0.5) * 140.0 * (1.0 - f / 3.0);
                let id = format!("glitch-{i}");
                let url = format!("url(#{id})");
                fframes::svgr!(
                    <g>
                        <clipPath id={id.clone()}><rect x="0" y={y} width="1920" height="180" /></clipPath>
                        <g clip-path={url} transform={format!("translate({dx} 0)")}>{ctx.render_scenes(frame)}</g>
                    </g>
                )
            })
            .collect();
        fframes::svgr!(<g>{bands}</g>)
    } else {
        ctx.render_scenes(frame)
    };
    fframes::svgr!(
        <g>
            <g transform={format!("translate(960 540) scale({s}) translate(-960 -540)")}>{scene}</g>
            <rect width="1920" height="1080" fill={BONE} opacity={flash * 0.55} />
        </g>
    )
}

// ---------------------------------------------------------------------------
// the 9:16 cut

/// The same video for phones: 1080x1920, same beats, sound and length.
#[derive(Debug)]
pub struct StudioIntroPortraitVideo;

impl Video for StudioIntroPortraitVideo {
    const FPS: usize = FPS;
    const WIDTH: usize = scenes::portrait::WIDTH;
    const HEIGHT: usize = scenes::portrait::HEIGHT;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn duration(&self) -> Duration<'_> {
        Duration::Auto
    }

    fn audio(&self) -> AudioMap<'_> {
        audio_map()
    }

    fn define_scenes(&self) -> Scenes<'_> {
        use scenes::portrait::*;
        Scenes::from(vec![
            &HookScene as &dyn Scene,
            &InstallScene,
            &StudioScene,
            &ProjectScene,
            &OutroScene,
            &HowScene,
            &FastScene,
            &WhyScene,
            &EndScene,
        ])
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let b = gbeat(&frame);
        let grain = SHADERS.grain.draw(
            &frame,
            ShaderUniforms::new()
                .float("uGrain", 0.07)
                .float("uVignette", 0.55),
        );
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" width="1080" height="1920" viewBox="0 0 1080 1920">
                <rect width="1080" height="1920" fill={BG} />
                {energy_portrait(&frame, ctx, b)}
                <image href={grain.href()} x="0" y="0" width="1080" height="1920" />
                {hud_portrait(&frame, b)}
            </svg>
        )
    }
}

fn hud_portrait(frame: &Frame, b: f32) -> Svgr<'static> {
    let appear = prog(gsec(frame), 0.1, 0.5);
    if appear <= 0.0 {
        return Svgr::empty();
    }
    let section = SECTIONS
        .iter()
        .rev()
        .find(|(start, _)| b >= *start)
        .map_or("", |(_, name)| *name);
    let bar = if b < 0.0 {
        0
    } else {
        (b / 4.0).floor() as i32 + 1
    };
    let beat_in_bar = if b < 0.0 {
        0
    } else {
        (b.rem_euclid(4.0)).floor() as i32 + 1
    };
    let tc = timecode(gsec(frame));
    let bpm = format!("BAR {bar:03}.{beat_in_bar}");
    let tick = if b < 0.0 { 0.0 } else { pulse(b, 7.0) };
    let progress = (gsec(frame) / TOTAL_SECONDS * 948.0).max(0.5);
    fframes::svgr!(
        <g opacity={appear * 0.9} style="mix-blend-mode:difference">
            {corners(36.0, 36.0, 1008.0, 1848.0, 26.0, "#d8d4cc", 2.0)}
            {label(66.0, 82.0, "FFRAMES STUDIO".to_owned(), "#d8d4cc", 17.0, "start")}
            {label(1014.0, 82.0, bpm, "#d8d4cc", 17.0, "end")}
            <rect x="1000" y="96" width="14" height="14" fill={ACCENT} opacity={0.25 + tick * 0.75} />
            {label(66.0, 1866.0, tc, "#d8d4cc", 17.0, "start")}
            {label(1014.0, 1866.0, section.to_owned(), "#d8d4cc", 17.0, "end")}
            <rect x="66" y="1882" width="948" height="1" fill="#d8d4cc" opacity="0.25" />
            <rect x="66" y="1881" width={progress} height="3" fill="#d8d4cc" />
        </g>
    )
}

fn energy_portrait<'a>(frame: &Frame, ctx: &FFramesContext<'a, '_>, b: f32) -> Svgr<'a> {
    let pumping = PUMPING.iter().any(|(s, e)| (*s..*e).contains(&b));
    let pump = if pumping { pulse(b, 9.0) } else { 0.0 };
    let s = 1.0 + pump * 0.006;
    let flash = DROPS
        .iter()
        .map(|d| if b >= *d { (-(b - d) * 7.0).exp() } else { 0.0 })
        .fold(0.0_f32, f32::max);
    let since_cut = GLITCH_CUTS
        .iter()
        .map(|c| (b - c) * BEAT * FPS as f32)
        .find(|f| *f >= 0.0 && *f < 3.0);
    let scene = if let Some(f) = since_cut {
        let bands: Vec<Svgr> = (0..6)
            .map(|i| {
                let y = i as f32 * 320.0;
                let dx = (hash(i as f32 * 13.0 + f.floor() * 7.0) - 0.5) * 100.0 * (1.0 - f / 3.0);
                let id = format!("glitch-p{i}");
                let url = format!("url(#{id})");
                fframes::svgr!(
                    <g>
                        <clipPath id={id.clone()}><rect x="0" y={y} width="1080" height="320" /></clipPath>
                        <g clip-path={url} transform={format!("translate({dx} 0)")}>{ctx.render_scenes(frame)}</g>
                    </g>
                )
            })
            .collect();
        fframes::svgr!(<g>{bands}</g>)
    } else {
        ctx.render_scenes(frame)
    };
    fframes::svgr!(
        <g>
            <g transform={format!("translate(540 960) scale({s}) translate(-540 -960)")}>{scene}</g>
            <rect width="1080" height="1920" fill={BONE} opacity={flash * 0.55} />
        </g>
    )
}
