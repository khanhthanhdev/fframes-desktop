//! The maths of the clip track's edits: Cap's drag snapping (`arm_drag_snap`,
//! `snap_dragged_time`), the trim a dragged edge asks for, where a dragged clip lands and
//! which clip a split at the playhead cuts. Frames are video frames of the displayed
//! report; clips are positions in the engine's `TimelineSnapshot::clips`.
use studio_engine::timing::{ResolvedClip, TrimEdge};

use super::model::ClipSegment;
use super::stops::StopClip;

/// How close, in pixels, a dragged edge must come to a target to snap to it.
pub const DRAG_SNAP_PX: f64 = 8.;
/// A press becomes a drag once the pointer has moved this far, in pixels.
pub const DRAG_THRESHOLD_PX: f32 = 2.;

/// The times, in seconds, a dragged edge snaps to. Gathered once when a drag starts, so
/// they hold still while the preview retimes under the drag.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapTargets(Vec<f64>);

impl SnapTargets {
    /// The targets of a trim of `clip`: zero, the playhead, the audio files' edges, and the
    /// edges of the clips that play before it. A trim moves every clip after it and the
    /// end of the video, so those are left out, as Cap leaves them out of a clip trim.
    pub fn for_trim(
        clip: usize,
        segments: &[ClipSegment],
        audio: impl IntoIterator<Item = (f64, f64)>,
        playhead: f64,
        fps: f64,
    ) -> Self {
        let mut targets = vec![0., playhead];
        for (start, end) in audio {
            targets.extend([start, end]);
        }
        for segment in segments
            .iter()
            .filter(|segment| segment.clip.is_some_and(|other| other < clip))
        {
            targets.extend([segment.start as f64 / fps, segment.end as f64 / fps]);
        }
        Self(targets)
    }

    /// The targets of a dragged slide stop: the playhead, and every clip's first frame
    /// and the frame of its `"end"` stop. A stop moves nothing else, so all stay put.
    pub fn for_stop(clips: &[StopClip], playhead: f64, fps: f64) -> Self {
        let mut targets = vec![playhead];
        for clip in clips {
            targets.push(clip.start as f64 / fps);
            targets.extend(clip.end_frame().map(|end| end as f64 / fps));
        }
        Self(targets)
    }

    /// `value` moved to the nearest target within [`DRAG_SNAP_PX`], with the target it
    /// snapped to; unchanged when none is that close or snapping is `disabled`.
    pub fn snap(&self, value: f64, seconds_per_pixel: f64, disabled: bool) -> (f64, Option<f64>) {
        if disabled {
            return (value, None);
        }
        let radius = DRAG_SNAP_PX * seconds_per_pixel;
        let best = self
            .0
            .iter()
            .copied()
            .filter(|target| (target - value).abs() <= radius)
            .min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()));
        match best {
            Some(target) => (target, Some(target)),
            None => (value, None),
        }
    }
}

/// The trim that puts the `edge` of `clip`, which started at frame `start` when the drag
/// began, at frame `to`. The clip keeps at least one frame and never plays more of its
/// scene than there is.
pub fn trim_for_edge(clip: &ResolvedClip, start: usize, edge: TrimEdge, to: usize) -> usize {
    match edge {
        TrimEdge::Start => {
            let moved = to as i64 - start as i64;
            let most = clip.scene_frames - clip.trim_end_frames - 1;
            (clip.trim_start_frames as i64 + moved).clamp(0, most as i64) as usize
        }
        TrimEdge::End => {
            let played = to.saturating_sub(start).max(1);
            (clip.scene_frames - clip.trim_start_frames).saturating_sub(played)
        }
    }
}

/// Where clip `from` lands when it is dropped at frame `at`: before the first other clip
/// whose middle lies after `at`. `segments` are the clip track's segments.
pub fn drop_position(segments: &[ClipSegment], from: usize, at: usize) -> usize {
    segments
        .iter()
        .filter(|segment| segment.clip.is_some_and(|clip| clip != from))
        .filter(|segment| (segment.start + segment.end) / 2 < at)
        .count()
}

/// The frame where the insertion marker of a drop at `position` is drawn: the start of the
/// clip it goes before, or the end of the last one.
pub fn drop_marker(segments: &[ClipSegment], from: usize, position: usize) -> Option<usize> {
    let mut others = segments
        .iter()
        .filter(|segment| segment.clip.is_some_and(|clip| clip != from));
    match others.clone().nth(position) {
        Some(next) => Some(next.start),
        None => others.next_back().map(|last| last.end),
    }
}

/// The clip a split at `frame` cuts and how many of its frames stay before the cut: the
/// selected scene's clip when the playhead is inside it, else the lowest row's. A split
/// on a clip's first frame or past its end cuts nothing.
pub fn split_at(
    segments: &[ClipSegment],
    frame: usize,
    selected_scene: Option<usize>,
) -> Option<(usize, usize)> {
    let inside = || {
        segments.iter().filter(|segment| {
            segment.clip.is_some()
                && segment.scene.is_some()
                && segment.start < frame
                && frame < segment.end
        })
    };
    let segment = inside()
        .find(|segment| selected_scene.is_some() && segment.scene == selected_scene)
        .or_else(|| inside().min_by_key(|segment| segment.lane))?;
    Some((segment.clip?, frame - segment.start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(scene_frames: usize, trim_start: usize, trim_end: usize) -> ResolvedClip {
        ResolvedClip {
            key: "intro".into(),
            scene_frames,
            trim_start_frames: trim_start,
            trim_end_frames: trim_end,
        }
    }

    fn segment(clip: usize, start: usize, end: usize, lane: usize) -> ClipSegment {
        ClipSegment {
            start,
            end,
            lane,
            label: format!("clip {clip}"),
            clip: Some(clip),
            scene: Some(clip),
        }
    }

    #[test]
    fn a_dragged_edge_snaps_to_the_nearest_target_within_eight_pixels() {
        // 100 px/s: eight pixels are 0.08 s.
        let targets = SnapTargets(vec![0., 1.0, 1.05, 3.0]);
        assert_eq!(targets.snap(1.04, 0.01, false), (1.05, Some(1.05)));
        assert_eq!(targets.snap(1.02, 0.01, false), (1.0, Some(1.0)));
        assert_eq!(targets.snap(2.5, 0.01, false), (2.5, None));
        assert_eq!(targets.snap(2.93, 0.01, false), (3.0, Some(3.0)));
        assert_eq!(targets.snap(1.04, 0.01, true), (1.04, None), "Shift");
    }

    #[test]
    fn a_trim_targets_only_what_stays_put() {
        // Three clips end to end at 30 fps; the second is trimmed.
        let segments = [
            segment(0, 0, 30, 0),
            segment(1, 30, 60, 0),
            segment(2, 60, 90, 0),
        ];
        let SnapTargets(targets) = SnapTargets::for_trim(1, &segments, [(0.5, 2.5)], 1.25, 30.);
        assert_eq!(targets, [0., 1.25, 0.5, 2.5, 0., 1.]);
    }

    #[test]
    fn a_stop_targets_the_playhead_and_every_clips_start_and_end_stop() {
        let clips = [
            StopClip {
                key: "a".into(),
                occurrence: 0,
                start: 0,
                end: 30,
                end_before_next: 25,
                trim_start: 0,
            },
            StopClip {
                key: "b".into(),
                occurrence: 0,
                start: 25,
                end: 60,
                end_before_next: 60,
                trim_start: 0,
            },
        ];
        let SnapTargets(targets) = SnapTargets::for_stop(&clips, 1.25, 10.);
        assert_eq!(targets, [1.25, 0., 2.4, 2.5, 5.9]);
    }

    #[test]
    fn trims_follow_the_dragged_edge_and_keep_a_frame() {
        // A 60 frame scene played from 10 to 50 at frames [30, 70) of the video.
        let played = clip(60, 10, 10);
        let trim = |edge, to| trim_for_edge(&played, 30, edge, to);
        assert_eq!(trim(TrimEdge::End, 60), 20);
        assert_eq!(trim(TrimEdge::End, 90), 0, "the scene ends");
        assert_eq!(trim(TrimEdge::End, 10), 49, "one frame");
        assert_eq!(trim(TrimEdge::Start, 35), 15);
        assert_eq!(trim(TrimEdge::Start, 20), 0, "the scene starts");
        assert_eq!(trim(TrimEdge::Start, 90), 49, "one frame");
    }

    #[test]
    fn a_dropped_clip_goes_before_the_first_middle_after_the_pointer() {
        let segments = [
            segment(0, 0, 30, 0),
            segment(1, 30, 60, 0),
            segment(2, 60, 90, 0),
        ];
        // Dragging the first clip.
        assert_eq!(drop_position(&segments, 0, 10), 0);
        assert_eq!(drop_position(&segments, 0, 50), 1);
        assert_eq!(drop_position(&segments, 0, 89), 2);
        assert_eq!(drop_marker(&segments, 0, 1), Some(60));
        assert_eq!(drop_marker(&segments, 0, 2), Some(90));
        // Dragging the last clip to the front.
        assert_eq!(drop_position(&segments, 2, 5), 0);
        assert_eq!(drop_marker(&segments, 2, 0), Some(0));
    }

    #[test]
    fn a_split_cuts_the_selected_clip_under_the_playhead() {
        let mut overlay = segment(1, 20, 50, 1);
        overlay.scene = Some(7);
        let segments = [segment(0, 0, 30, 0), overlay, segment(2, 30, 60, 0)];
        assert_eq!(split_at(&segments, 25, None), Some((0, 25)));
        assert_eq!(split_at(&segments, 25, Some(7)), Some((1, 5)));
        // Frame 30 is the cut between clips 0 and 2: only the overlay plays across it.
        assert_eq!(split_at(&segments, 30, None), Some((1, 10)));
        assert_eq!(split_at(&segments, 0, None), None);
        assert_eq!(split_at(&segments, 60, None), None);
    }
}
