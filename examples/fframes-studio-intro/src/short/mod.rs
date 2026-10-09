//! A 41 second 9:16 introduction to fframes Studio, designed for a phone from the
//! start (not a crop of the landscape video): the Studio window is a vertical stack
//! of canvas, timeline and agent chat, the type is sized for a thumb-sized screen.
//! It follows a first run (title and description in, a video out), then the edit by
//! selection, then a wall of frames rendering in parallel that resolves into the QR
//! code of the end card.
//!
//! It shares the beat grid and the soundtrack with the long cuts (see `beat.rs`); the
//! scenes are cut to the same drop on beat 48.

#[macro_use]
mod macros {
    /// A scene with its place on the beat grid. `None` as start means the start of
    /// the video; every end is explicit because the short cut stops on a bar.
    macro_rules! short_scene {
        ($name:ident, $start:expr, $end:expr) => {
            #[derive(Debug)]
            pub struct $name;
            impl $name {
                pub const START: Option<f32> = $start;
                pub const END: Option<f32> = Some($end);
                /// Beat inside the scene (0 on its first downbeat).
                #[allow(dead_code)]
                pub fn lb(frame: &fframes::Frame) -> f32 {
                    crate::beat::gbeat(frame) - Self::START.unwrap_or(0.0)
                }
                pub fn frames() -> fframes::Duration<'static> {
                    fframes::Duration::Frames(crate::beat::span_frames(Self::START, Self::END))
                }
            }
        };
    }
}

mod create;
mod end;
mod fast;
mod install;
mod parallel;
pub mod qr;
mod studio;

use fframes::{
    AudioMap, AudioTimestamp::*, AudioTrack, Color, Duration, FFramesContext, Frame, Scene, Scenes,
    ShaderUniforms, Svgr, Video,
};

use crate::beat::*;
use crate::scenes::portrait::HookScene;
use crate::shaders::SHADERS;
use crate::ui::*;

pub const WIDTH: usize = 1080;
pub const HEIGHT: usize = 1920;

/// The beat the video stops on, a bar line.
pub const LAST_BEAT: f32 = 86.0;
/// Length of the video in seconds.
pub fn total_seconds() -> f32 {
    beat_time(LAST_BEAT)
}

/// Beats the scenes start on.
const STUDIO_AT: f32 = 12.0;
const FAST_AT: f32 = 58.0;
const PARALLEL_AT: f32 = 64.0;
const END_AT: f32 = 72.0;
/// The drop of the soundtrack: the agent's edit lands on it.
const DROP: f32 = 48.0;

#[derive(Debug)]
pub struct StudioShortVideo;

const SECTIONS: &[(f32, &str)] = &[
    (-99.0, "HOOK"),
    (0.0, "INSTALL"),
    (12.0, "NEW VIDEO"),
    (22.0, "GENERATE"),
    (30.0, "SELECT"),
    (32.5, "PROMPT"),
    (40.5, "RETRIEVE"),
    (48.0, "EDIT"),
    (52.0, "PREVIEW"),
    (54.5, "EXPORT"),
    (58.0, "SPEED"),
    (64.0, "PARALLEL"),
    (72.0, "GET IT"),
];

/// Sections where the full beat plays: the picture pumps with the kick.
const PUMPING: &[(f32, f32)] = &[(48.0, 80.0)];
const GLITCH_CUTS: &[f32] = &[12.0, 58.0, 64.0];
const DROPS: &[f32] = &[48.0, 72.0];

fn sfx(name: &'static str, at: f32, gain: f32) -> AudioTrack<'static> {
    AudioTrack::new(name, Second(at.max(0.0))..Eof).gain_db(gain)
}

fn audio_map() -> AudioMap<'static> {
    let mut tracks = vec![
        AudioTrack::new("music.wav", Second(0.0)..Second(total_seconds())).fade_out(1.8),
        sfx("sfx_typing.mp3", 0.02, -15.0),
        sfx("sfx_enter.mp3", beat_time(-1.0) - 0.067, -9.0),
        sfx("sfx_glitch.mp3", beat_time(STUDIO_AT) - 0.2, -8.0),
        sfx("sfx_typing.mp3", beat_time(STUDIO_AT + 1.8), -17.0),
        sfx("sfx_typing.mp3", beat_time(STUDIO_AT + 4.5), -17.0),
        sfx("sfx_typing.mp3", beat_time(STUDIO_AT + 8.1), -17.0),
        sfx(
            "sfx_typing.mp3",
            beat_time(STUDIO_AT + studio::TYPE_FROM),
            -17.0,
        ),
        sfx(
            "sfx_typing.mp3",
            beat_time(STUDIO_AT + studio::TYPE_FROM + 3.5),
            -17.0,
        ),
        sfx(
            "sfx_enter.mp3",
            beat_time(STUDIO_AT + create::GENERATE) - 0.067,
            -9.0,
        ),
        sfx(
            "sfx_enter.mp3",
            beat_time(STUDIO_AT + studio::SEND) - 0.067,
            -9.0,
        ),
        sfx("sfx_impact.mp3", beat_time(DROP), -10.0),
        sfx("sfx_shutter.mp3", beat_time(DROP) - 0.02, -4.0),
        sfx("sfx_impact.mp3", beat_time(END_AT), -11.0),
        sfx("sfx_braam.mp3", beat_time(END_AT), -9.0),
    ];
    for b in [STUDIO_AT, FAST_AT, PARALLEL_AT, END_AT] {
        tracks.push(sfx("sfx_whoosh.mp3", beat_time(b) - 0.18, -15.0));
    }
    for b in install::BLIP_BEATS {
        tracks.push(sfx("sfx_blip.mp3", beat_time(*b), -9.0));
    }
    for b in create::BLIP_BEATS.iter().skip(1) {
        tracks.push(sfx("sfx_blip.mp3", beat_time(STUDIO_AT + *b), -9.0));
    }
    for b in parallel::BLIP_BEATS {
        tracks.push(sfx("sfx_blip.mp3", beat_time(PARALLEL_AT + *b), -9.0));
    }
    for b in studio::BLIP_BEATS {
        tracks.push(sfx("sfx_blip.mp3", beat_time(STUDIO_AT + *b), -9.0));
    }
    for b in fast::BLIP_BEATS {
        tracks.push(sfx("sfx_blip.mp3", beat_time(FAST_AT + *b), -9.0));
    }
    AudioMap::from(tracks)
}

impl Video for StudioShortVideo {
    const FPS: usize = FPS;
    const WIDTH: usize = WIDTH;
    const HEIGHT: usize = HEIGHT;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn duration(&self) -> Duration<'_> {
        Duration::Auto
    }

    fn audio(&self) -> AudioMap<'_> {
        audio_map()
    }

    fn define_scenes(&self) -> Scenes<'_> {
        Scenes::from(vec![
            &HookScene as &dyn Scene,
            &install::InstallScene,
            &studio::StudioScene,
            &fast::FastScene,
            &parallel::ParallelScene,
            &end::EndScene,
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
                {energy(&frame, ctx, b)}
                <image href={grain.href()} x="0" y="0" width="1080" height="1920" />
                {hud(&frame, b)}
            </svg>
        )
    }
}

fn hud(frame: &Frame, b: f32) -> Svgr<'static> {
    let appear = prog(gsec(frame), 0.1, 0.5);
    if appear <= 0.0 {
        return Svgr::empty();
    }
    let section = SECTIONS
        .iter()
        .rev()
        .find(|(start, _)| b >= *start)
        .map_or("", |(_, name)| *name);
    let tick = if b < 0.0 { 0.0 } else { pulse(b, 7.0) };
    let progress = (gsec(frame) / total_seconds() * 948.0).max(0.5);
    fframes::svgr!(
        <g opacity={appear * 0.9} style="mix-blend-mode:difference">
            {corners(36.0, 36.0, 1008.0, 1848.0, 26.0, "#d8d4cc", 2.0)}
            {label(66.0, 82.0, "FFRAMES STUDIO".to_owned(), "#d8d4cc", 17.0, "start")}
            {label(1014.0, 82.0, section.to_owned(), "#d8d4cc", 17.0, "end")}
            <rect x="1000" y="96" width="14" height="14" fill={ACCENT} opacity={0.25 + tick * 0.75} />
            <rect x="66" y="1882" width="948" height="1" fill="#d8d4cc" opacity="0.25" />
            <rect x="66" y="1881" width={progress} height="3" fill="#d8d4cc" />
        </g>
    )
}

fn energy<'a>(frame: &Frame, ctx: &FFramesContext<'a, '_>, b: f32) -> Svgr<'a> {
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
                let id = format!("glitch-s{i}");
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
