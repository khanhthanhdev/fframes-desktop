use crate::audio_map::AudioMap;
use crate::error::Result;
use crate::{
    AudioTimelineUnit, Color, Duration, FFramesContext, Frame, ResolvedAudioMap, SceneInfo, Svgr,
    TimeBase, scenes::*,
};

/// The base fframes video trait. It represents how to render a video for a struct which becomes an
/// input of the video.
pub trait Video: Sync + Sized {
    const FPS: usize;
    const WIDTH: usize;
    const HEIGHT: usize;

    /// Background color of the video. This allows to set a solid color background.
    ///
    /// Make sure if you want to render a transparent video use `Color::TRANSPARENT` here
    /// **and** set the proper encoder and `pixel_format` that supports transparency
    /// (e.g. encoder libx265 with yuva420p pixel format) when rendering the video.
    const BACKGROUND_COLOR: Color = Color::BLACK;

    /// Defines either dynamic or inferred duration of the video
    fn duration(&self) -> Duration<'_>;

    /// Defines the audio timeline of the video (when and how long audio tracks are played)
    fn audio(&self) -> AudioMap<'_>;

    /// Defines the scenes timeline of the video.
    /// Each scene is an dyn object which implements the `Scene` trait.
    ///
    /// Every scene must be either bound to the `&self` lifetime or be a zero sized type.
    /// In short: put your scenes to the `&self` or do not add any fields to the scene struct.
    ///
    /// # Example
    /// ```no_run
    /// use fframes::{Scene, Scenes, Video};
    ///
    /// #[derive(Debug)]
    /// struct Intro;
    /// #[derive(Debug)]
    /// struct Chapter {
    ///     title: String,
    /// }
    ///
    /// impl Scene for Intro { /* duration, render_frame */ }
    /// impl Scene for Chapter { /* duration, render_frame */ }
    ///
    /// struct MyVideo {
    ///     chapter: Chapter,
    /// }
    ///
    /// impl Video for MyVideo {
    ///     fn define_scenes(&self) -> Scenes<'_> {
    ///         let scenes: Vec<&dyn Scene> = vec![
    ///             // a zero sized type, so the reference can be created right here
    ///             &Intro,
    ///             // a scene with data is borrowed from `self`
    ///             &self.chapter,
    ///         ];
    ///         Scenes::from(scenes)
    ///     }
    /// }
    /// ```
    fn define_scenes(&self) -> Scenes<'_> {
        Scenes(None)
    }

    /// This function is going to be called for each frame of the video and expects to return
    /// a valid SVG rendering tree for the specific frame.
    ///
    /// This function is going to be called thousands of times per rendering, so it is important to reduce
    /// amount of allocations and cpu bound operations happening during the render frame. It is
    /// possible to cache the data in the `self` and use it in the function or to memoize the data
    /// in `self` using `once_cell::LazyLock` or similar constructs.
    ///
    /// **Tip:** Avoid panicking in this function as much as possible, this function does not
    /// return `Result` because it is extremely expensive to stop the rendering once it has
    /// started. Prepare compiler guaranteed data in advance and read it from `self`.
    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a>;

    /// Stable author-supplied identity for the root video scene, when it is
    /// intentionally exposed to canvas selection.
    fn editor_instance_key(&self) -> Option<&str> {
        None
    }
}

/// One clip on the resolved timeline: a code scene, or the part of it a timing
/// document keeps, placed at `range`.
#[derive(Debug, Clone)]
pub struct ResolvedSceneClip<'a> {
    /// Global frames of the clip, overlaps included.
    pub range: std::ops::Range<usize>,
    pub info: SceneInfo,
    pub scene: &'a (dyn Scene + 'a),
    /// Scene frames cut before the clip. The scene sees frame `trim_start` at `range.start`.
    pub trim_start: usize,
    pub trim_end: usize,
    /// The scene's timing key, see `scene_timing_keys`.
    pub timing_key: String,
    /// Occurrence of the scene among the clips (0 unless the scene is split).
    pub clip_index: usize,
}

impl ResolvedSceneClip<'_> {
    /// Editor object key of the clip, when the scene has an editor instance key.
    pub fn editor_key(&self) -> Option<String> {
        self.scene
            .editor_instance_key()
            .map(|key| crate::clip_editor_key(key, self.clip_index))
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedScenesTimeline<'a> {
    pub total_scenes_duration: usize,
    pub(crate) timeline: Vec<ResolvedSceneClip<'a>>,
    /// Orphan and appended scenes of the applied timing document.
    pub timing_report: crate::TimingReport,
}

impl<'a> ResolvedScenesTimeline<'a> {
    pub fn iter(&self) -> impl Iterator<Item = &ResolvedSceneClip<'a>> {
        self.timeline.iter()
    }

    fn from_scenes(
        time_base: &TimeBase,
        scenes: &[SceneWithAudio<'a>],
        timing: Option<&crate::TimingDoc>,
        resolve_audio_duration: &impl Fn(&str) -> super::error::Result<f64>,
    ) -> Result<Self> {
        let scene_frames = scenes
            .iter()
            .map(|SceneWithAudio { scene, audio_map }| {
                scene
                    .duration()
                    .to_frames_async(time_base.fps, audio_map, &resolve_audio_duration)
            })
            .collect::<Result<Vec<_>>>()?;
        let keys = crate::scene_timing_keys(scenes.iter().map(|s| s.scene));
        let (clips, timing_report) = crate::timing::plan_clips(timing, &keys, &scene_frames)
            .map_err(crate::error::FFramesError::InvalidTiming)?;
        let max_frames = crate::MAX_TIMING_SECONDS.saturating_mul(time_base.fps);

        let clips_count = clips.len();
        let mut final_duration: usize = 0;
        let mut resolved_scenes = Vec::with_capacity(clips_count);

        for (index, clip) in clips.into_iter().enumerate() {
            let scene = scenes[clip.scene_index].scene;
            let full = scene_frames[clip.scene_index];
            // plan_clips guarantees trim_start + trim_end < full
            let duration = full - clip.trim_start - clip.trim_end;

            // Overlaps belong to the scene's natural edges: a trimmed edge is a hard cut.
            let (overlap_prev, overlap_next) = scene.overlap().to_frames(time_base.fps);
            let overlap_prev = if clip.trim_start == 0 {
                overlap_prev
            } else {
                0
            };
            let overlap_next = if clip.trim_end == 0 { overlap_next } else { 0 };
            // An overlap reaching before the first frame (a reordered first scene) is clamped.
            let start_frame = final_duration.saturating_sub(overlap_prev);
            let end_frame = final_duration
                .checked_add(duration)
                .and_then(|end| end.checked_add(overlap_next))
                .filter(|end| timing.is_none() || *end <= max_frames)
                .ok_or(crate::error::FFramesError::InvalidTiming(
                    crate::TimingError::TooLong(final_duration.saturating_add(duration)),
                ))?;
            resolved_scenes.push(ResolvedSceneClip {
                range: start_frame..end_frame,
                info: SceneInfo {
                    index,
                    start_frame,
                    end_frame,
                    total_scenes_in_video: clips_count,
                    // The scene's own length: trims cut the window, not the scene.
                    duration_in_frames: full,
                    is_last: index + 1 == clips_count,
                },
                scene,
                trim_start: clip.trim_start,
                trim_end: clip.trim_end,
                timing_key: keys[clip.scene_index].clone(),
                clip_index: clip.clip_index,
            });

            // the scene duration is reuded by any overlap but we should be careful with the scenes
            // that have the overlap larger than the scene duration itself
            final_duration = (final_duration + duration).saturating_sub(overlap_next);
        }

        Ok(ResolvedScenesTimeline {
            total_scenes_duration: final_duration,
            timeline: resolved_scenes,
            timing_report,
        })
    }
}

pub struct ResolvedRenderingTimeline<'a, TAudioUnit: AudioTimelineUnit + std::fmt::Debug> {
    pub audio_map: Option<ResolvedAudioMap<TAudioUnit>>,
    pub scenes: Option<ResolvedScenesTimeline<'a>>,
    pub duration_in_frames: usize,
}

pub fn resolve_timeline<
    'a,
    TAudioUnit: AudioTimelineUnit + std::fmt::Debug + Copy,
    TFun: Fn(&str) -> super::error::Result<f64>,
>(
    duration: &Duration,
    scenes: &ScenesWithAudio<'a>,
    time_base: &TimeBase,
    top_level_audio_map: &AudioMap,
    resolve_audio_duration: TFun,
) -> Result<ResolvedRenderingTimeline<'a, TAudioUnit>> {
    resolve_timeline_with_timing(
        duration,
        scenes,
        time_base,
        top_level_audio_map,
        None,
        resolve_audio_duration,
    )
}

/// `resolve_timeline` with the scenes laid out by a timing document instead of the
/// code order; `None` keeps the code order.
pub fn resolve_timeline_with_timing<
    'a,
    TAudioUnit: AudioTimelineUnit + std::fmt::Debug + Copy,
    TFun: Fn(&str) -> super::error::Result<f64>,
>(
    duration: &Duration,
    scenes: &ScenesWithAudio<'a>,
    time_base: &TimeBase,
    top_level_audio_map: &AudioMap,
    timing: Option<&crate::TimingDoc>,
    resolve_audio_duration: TFun,
) -> Result<ResolvedRenderingTimeline<'a, TAudioUnit>> {
    let (duration, resolved_scenes) = match (scenes.0.as_deref(), duration) {
        (Some(scenes), Duration::Auto) => {
            let timeline = ResolvedScenesTimeline::from_scenes(
                time_base,
                scenes,
                timing,
                &resolve_audio_duration,
            )?;

            (timeline.total_scenes_duration, Some(timeline))
        }
        // if both duration and scenes provided we use duration
        (Some(scenes), duration) => {
            let timeline = ResolvedScenesTimeline::from_scenes(
                time_base,
                scenes,
                timing,
                &resolve_audio_duration,
            )?;
            let total_duration = duration.to_frames_async(
                time_base.fps,
                top_level_audio_map,
                &resolve_audio_duration,
            )?;

            (total_duration, Some(timeline))
        }
        (None, duration) => {
            let total_duration = duration.to_frames_async(
                time_base.fps,
                top_level_audio_map,
                &resolve_audio_duration,
            )?;

            (total_duration, None)
        }
    };

    let mut resolved_audio_map = top_level_audio_map.resolve_with_scenes::<TAudioUnit>(
        resolved_scenes.as_ref(),
        time_base,
        &resolve_audio_duration,
    )?;

    if let Some(resolved_audio_map) = resolved_audio_map.as_mut() {
        resolved_audio_map.round_max_duration(TAudioUnit::from_frames(duration, time_base));
    }

    Ok(ResolvedRenderingTimeline {
        audio_map: resolved_audio_map,
        scenes: resolved_scenes,
        duration_in_frames: duration,
    })
}
