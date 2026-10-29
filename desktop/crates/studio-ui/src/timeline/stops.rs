//! Slide stop markers: where each resolved stop sits on the timeline, which marker a
//! press hits, the clip a dragged marker stays in, and what a marker at a frame writes
//! into `slides.json`. Pure functions over the laid-out report and the slides document;
//! the view draws and drags the markers with them.
//!
//! The clips here follow the engine's resolution (`studio_engine::slides::resolve`):
//! play order, and an `"end"` stop on a clip's last frame before the next clip starts.
use fframes_studio_protocol::PreviewTimelineResponse;
use studio_engine::slides::{SlideStop, SlidesDoc, StopAt};

/// How close, in pixels, a press must come to a marker to grab it.
pub const STOP_HIT_PX: f32 = 6.;

/// One clip of the laid-out timeline as slide stops see it, in video frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopClip {
    /// The clip's timing key, which stops name.
    pub key: String,
    /// The clip's place among the clips of its key, in play order (a split scene has
    /// several).
    pub occurrence: usize,
    pub start: usize,
    pub end: usize,
    /// The frame after the clip's last frame before the next clip starts to overlap it.
    pub end_before_next: usize,
    pub trim_start: usize,
}

impl StopClip {
    /// The frame of the clip's `"end"` stop, unless the next clip covers all of it.
    pub fn end_frame(&self) -> Option<usize> {
        (self.end_before_next > self.start).then(|| self.end_before_next - 1)
    }

    /// `frame` moved into the frames a stop of this clip can be dragged to: from its
    /// start (never the video's first frame, where every deck starts) to its `"end"`.
    pub fn clamp(&self, frame: usize) -> Option<usize> {
        let (first, last) = (self.start.max(1), self.end_frame()?);
        (first <= last).then(|| frame.clamp(first, last))
    }

    fn contains(&self, frame: usize) -> bool {
        self.start <= frame && frame < self.end
    }
}

/// The clips of `report` in play order; empty when the worker reported no clip data
/// (nothing resolves then).
pub fn stop_clips(report: &PreviewTimelineResponse) -> Vec<StopClip> {
    let mut clips = Vec::with_capacity(report.scenes.len());
    for scene in &report.scenes {
        let Some(timing) = &scene.clip else {
            return Vec::new();
        };
        let end = scene.end_frame.max(scene.start_frame);
        clips.push(StopClip {
            key: timing.timing_key.clone(),
            occurrence: 0,
            start: scene.start_frame,
            end,
            end_before_next: end,
            trim_start: timing.trim_start_frames,
        });
    }
    clips.sort_by_key(|clip| clip.start);
    for n in 0..clips.len() {
        clips[n].occurrence = clips[..n].iter().filter(|c| c.key == clips[n].key).count();
        if let Some(next) = clips.get(n + 1).map(|c| c.start) {
            clips[n].end_before_next = clips[n].end.min(next);
        }
    }
    clips
}

/// The clip a stop of `key` resolved to `video_frame` belongs to: the first clip of the
/// key that draws the frame, as the engine resolves it.
pub fn clip_of<'a>(clips: &'a [StopClip], key: &str, video_frame: usize) -> Option<&'a StopClip> {
    clips
        .iter()
        .find(|clip| clip.key == key && clip.contains(video_frame))
}

/// The clip that plays `frame`: the one whose own part (before the next clip starts)
/// holds it, else the first that draws it.
pub fn clip_at(clips: &[StopClip], frame: usize) -> Option<&StopClip> {
    clips
        .iter()
        .find(|clip| clip.start <= frame && frame < clip.end_before_next)
        .or_else(|| clips.iter().find(|clip| clip.contains(frame)))
}

/// How many `"end"` stops of `key` come before position `index` of the document: the
/// n-th `"end"` stop of a key names the n-th clip of that key.
pub fn ends_before(doc: &SlidesDoc, key: &str, index: usize) -> usize {
    doc.stops[..index.min(doc.stops.len())]
        .iter()
        .filter(|stop| stop.scene == key && stop.at == StopAt::End)
        .count()
}

/// Where a stop at video frame `frame` of `clip` points in the document: `"end"` on the
/// clip's end frame when an `"end"` stop at that place names this clip (`ends_before`
/// is the count of earlier `"end"` stops of its key), else the scene frame.
pub fn stop_at(clip: &StopClip, frame: usize, ends_before: usize) -> StopAt {
    if Some(frame) == clip.end_frame() && clip.occurrence == ends_before {
        StopAt::End
    } else {
        StopAt::Frame(frame.saturating_sub(clip.start) + clip.trim_start)
    }
}

/// The stop a press at video frame `frame` adds to the end of `doc`; `None` on the
/// first frame (where the deck starts) or where no clip plays.
pub fn new_stop(doc: &SlidesDoc, clips: &[StopClip], frame: usize) -> Option<SlideStop> {
    if frame == 0 {
        return None;
    }
    let clip = clip_at(clips, frame)?;
    let ends = ends_before(doc, &clip.key, doc.stops.len());
    Some(SlideStop {
        scene: clip.key.clone(),
        at: stop_at(clip, frame, ends),
        title: None,
        notes: None,
    })
}

/// The marker a press at `x` grabs: the nearest of `markers` (`(id, x)`) within
/// [`STOP_HIT_PX`].
pub fn hit_marker<T: Copy>(markers: impl IntoIterator<Item = (T, f32)>, x: f32) -> Option<T> {
    markers
        .into_iter()
        .map(|(id, at)| (id, (at - x).abs()))
        .filter(|(_, distance)| *distance <= STOP_HIT_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::model::tests::report;

    fn clip(key: &str, occurrence: usize, start: usize, end: usize, next: usize) -> StopClip {
        StopClip {
            key: key.into(),
            occurrence,
            start,
            end,
            end_before_next: next,
            trim_start: 0,
        }
    }

    fn stop(scene: &str, at: StopAt) -> SlideStop {
        SlideStop {
            scene: scene.into(),
            at,
            title: None,
            notes: None,
        }
    }

    #[test]
    fn clips_follow_play_order_and_end_before_a_cross_fade() {
        // `b` starts 5 frames before `a` ends; `a` plays again after `b`.
        let mut laid_out = report(10, &[("a", 0, 30), ("b", 25, 60), ("a", 60, 90)]);
        laid_out.scenes[1].clip.as_mut().unwrap().trim_start_frames = 4;
        let clips = stop_clips(&laid_out);
        assert_eq!(
            clips,
            [
                clip("a", 0, 0, 30, 25),
                StopClip {
                    trim_start: 4,
                    ..clip("b", 0, 25, 60, 60)
                },
                clip("a", 1, 60, 90, 90),
            ]
        );
        assert_eq!(clips[0].end_frame(), Some(24));
        // A worker without clip data resolves nothing.
        laid_out.scenes[2].clip = None;
        assert!(stop_clips(&laid_out).is_empty());
    }

    #[test]
    fn a_dragged_stop_stays_inside_its_clip() {
        let first = clip("a", 0, 0, 30, 25);
        assert_eq!(first.clamp(12), Some(12));
        assert_eq!(first.clamp(40), Some(24), "not into the cross-fade");
        assert_eq!(first.clamp(0), Some(1), "never the deck's first frame");
        let middle = clip("b", 0, 25, 60, 60);
        assert_eq!(middle.clamp(3), Some(25));
        assert_eq!(middle.clamp(200), Some(59));
        // The next clip covers all of this one: nowhere to put a stop.
        assert_eq!(clip("c", 0, 10, 20, 10).clamp(15), None);
        assert_eq!(clip("d", 0, 0, 1, 1).clamp(0), None);
    }

    #[test]
    fn markers_are_hit_within_six_pixels_and_the_nearest_wins() {
        let markers = [(0, 100.), (1, 109.), (2, 300.)];
        assert_eq!(hit_marker(markers, 100.), Some(0));
        assert_eq!(hit_marker(markers, 104.), Some(0));
        assert_eq!(hit_marker(markers, 105.5), Some(1));
        assert_eq!(hit_marker(markers, 115.), Some(1));
        assert_eq!(hit_marker(markers, 115.5), None);
        assert_eq!(hit_marker(markers, 294.), Some(2));
        assert_eq!(hit_marker::<usize>([], 0.), None);
    }

    #[test]
    fn a_stop_on_its_clips_end_frame_is_written_as_end() {
        let clips = [
            clip("a", 0, 0, 30, 30),
            StopClip {
                trim_start: 5,
                ..clip("b", 0, 30, 60, 60)
            },
            clip("a", 1, 60, 90, 90),
        ];
        assert_eq!(stop_at(&clips[1], 40, 0), StopAt::Frame(15), "trim start");
        assert_eq!(stop_at(&clips[1], 59, 0), StopAt::End);
        // The second clip of `a` only gets an "end" when an earlier "end" names the first.
        assert_eq!(stop_at(&clips[2], 89, 0), StopAt::Frame(29));
        assert_eq!(stop_at(&clips[2], 89, 1), StopAt::End);

        let mut doc = SlidesDoc::default();
        assert_eq!(new_stop(&doc, &clips, 0), None, "the deck's first frame");
        assert_eq!(new_stop(&doc, &clips, 95), None, "past the video");
        assert_eq!(new_stop(&doc, &clips, 29), Some(stop("a", StopAt::End)));
        assert_eq!(
            new_stop(&doc, &clips, 30),
            Some(stop("b", StopAt::Frame(5)))
        );
        doc.stops.push(stop("a", StopAt::End));
        assert_eq!(ends_before(&doc, "a", 1), 1);
        assert_eq!(new_stop(&doc, &clips, 89), Some(stop("a", StopAt::End)));
        assert_eq!(clip_of(&clips, "a", 70), Some(&clips[2]));
        assert_eq!(clip_of(&clips, "b", 70), None);
    }
}
