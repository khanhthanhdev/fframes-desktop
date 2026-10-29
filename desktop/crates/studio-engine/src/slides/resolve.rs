//! Resolves the stops of a slides document to video frames of a laid-out timeline.
//!
//! Pure: the same document and timeline always give the same stops. The timeline is the
//! one the person sees (a build's or a retime's), so a trim, split or move changes the
//! resolved frames at once, without a rebuild.
//!
//! - A clip draws scene frames `trim_start .. trim_start + (end_frame - start_frame)`,
//!   which includes the parts it overlaps its neighbours with.
//! - `"end"` is video frame `min(end_frame, next.start_frame) - 1`: the clip's last
//!   frame before the next clip starts to cross-fade over it. The n-th `"end"` stop of a
//!   key names the n-th clip of that key in play order (a split scene has several).
//! - A frame resolves through the first clip in play order whose drawn range has it.
//! - A video without scenes has one virtual full-video clip with reserved key `"video"`.
//!   This does not replace missing clip metadata on timelines that do have scenes.
//! - A stop no clip draws is unresolved and reported, never moved.
//! - Resolved stops are sorted by video frame; equal frames merge (the first stop's title
//!   and notes win); a stop at frame 0 is dropped. The video's last frame is always the
//!   final stop.
use std::fmt;

use fframes_studio_protocol::{PreviewClipTiming, PreviewTimelineResponse};

use super::doc::{SlidesDoc, StopAt};

/// Reported when a timeline has no clip data.
pub const WORKER_TOO_OLD: &str =
    "the preview worker is too old for slides; rebuild with the current SDK";

/// Where a resolved stop comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopSource {
    /// The stop at this position of the document's `stops`.
    Explicit(usize),
    /// A default stop (the end of a clip of a document without stops) or the video's
    /// final frame.
    Default,
}

/// A stop at a video frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStop {
    pub video_frame: usize,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub source: StopSource,
}

/// Why a stop is not where the document says, or why nothing resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlidesProblem {
    /// The timeline has no clip data: nothing resolves.
    WorkerTooOld,
    /// No clip draws the stop at this position of the document; it is skipped.
    Unresolved { stop: usize, reason: String },
    /// The stop lands on the frame of an earlier stop and merged into it.
    Merged { stop: usize, video_frame: usize },
    /// The stop lands on the video's first frame, where every deck starts; dropped.
    AtStart { stop: usize },
}

impl SlidesProblem {
    /// Whether the stop is missing from the deck (not only merged or dropped as
    /// redundant).
    pub fn is_unresolved(&self) -> bool {
        matches!(self, Self::WorkerTooOld | Self::Unresolved { .. })
    }
}

impl fmt::Display for SlidesProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerTooOld => f.write_str(WORKER_TOO_OLD),
            Self::Unresolved { stop, reason } => write!(f, "stop {}: {reason}", stop + 1),
            Self::Merged { stop, video_frame } => write!(
                f,
                "stop {} lands on video frame {video_frame} like an earlier stop and was merged into it",
                stop + 1
            ),
            Self::AtStart { stop } => write!(
                f,
                "stop {} lands on the first frame of the video, where the deck starts; it was dropped",
                stop + 1
            ),
        }
    }
}

/// One clip in play order.
struct Clip<'a> {
    timing: &'a PreviewClipTiming,
    start: usize,
    end: usize,
    /// The video frame after the clip's last frame before the next clip starts.
    end_before_next: usize,
}

impl Clip<'_> {
    /// The video frame that draws scene frame `at`, when the clip draws it.
    fn frame(&self, at: usize) -> Option<usize> {
        let first = self.timing.trim_start_frames;
        (at >= first && at - first < self.end - self.start).then(|| self.start + (at - first))
    }

    /// The video frame of `"end"`, unless the next clip covers all of this one.
    fn end_frame(&self) -> Option<usize> {
        (self.end_before_next > self.start).then(|| self.end_before_next - 1)
    }
}

/// The document's stops at video frames of `timeline`, and what did not resolve.
pub fn resolve(
    doc: &SlidesDoc,
    timeline: &PreviewTimelineResponse,
) -> (Vec<ResolvedStop>, Vec<SlidesProblem>) {
    let mut problems = Vec::new();
    let mut clips = Vec::with_capacity(timeline.scenes.len());
    let root = PreviewClipTiming {
        timing_key: "video".into(),
        clip_index: 0,
        trim_start_frames: 0,
        trim_end_frames: 0,
        scene_index: None,
        scene_frames: Some(timeline.total_frames),
    };
    if timeline.scenes.is_empty() {
        clips.push(Clip {
            timing: &root,
            start: 0,
            end: timeline.total_frames,
            end_before_next: timeline.total_frames,
        });
    }
    for scene in &timeline.scenes {
        let Some(timing) = &scene.clip else {
            return (Vec::new(), vec![SlidesProblem::WorkerTooOld]);
        };
        clips.push(Clip {
            timing,
            start: scene.start_frame,
            end: scene.end_frame.max(scene.start_frame),
            end_before_next: scene.end_frame,
        });
    }
    clips.sort_by_key(|clip| clip.start);
    for n in 1..clips.len() {
        let next = clips[n].start;
        let clip = &mut clips[n - 1];
        clip.end_before_next = clip.end.min(next);
    }

    let mut found: Vec<ResolvedStop> = Vec::new();
    if doc.stops.is_empty() {
        found.extend(
            clips
                .iter()
                .filter_map(Clip::end_frame)
                .map(|frame| ResolvedStop {
                    video_frame: frame,
                    title: None,
                    notes: None,
                    source: StopSource::Default,
                }),
        );
    }
    for (index, stop) in doc.stops.iter().enumerate() {
        let mut of_key = clips
            .iter()
            .filter(|clip| clip.timing.timing_key == stop.scene);
        let frame = match stop.at {
            StopAt::End => {
                let occurrence = doc.stops[..index]
                    .iter()
                    .filter(|earlier| earlier.scene == stop.scene && earlier.at == StopAt::End)
                    .count();
                match of_key.nth(occurrence) {
                    Some(clip) => clip.end_frame().ok_or_else(|| {
                        format!(
                            "the end of scene {:?} is covered by the next clip",
                            stop.scene
                        )
                    }),
                    None if occurrence == 0 => Err(format!("no clip plays scene {:?}", stop.scene)),
                    None => Err(format!(
                        "scene {:?} has no clip {} for this \"end\" stop",
                        stop.scene,
                        occurrence + 1
                    )),
                }
            }
            StopAt::Frame(at) => {
                let mut any = false;
                of_key
                    .find_map(|clip| {
                        any = true;
                        clip.frame(at)
                    })
                    .ok_or_else(|| {
                        if any {
                            format!(
                                "frame {at} of scene {:?} is not played (trimmed or past its end)",
                                stop.scene
                            )
                        } else {
                            format!("no clip plays scene {:?}", stop.scene)
                        }
                    })
            }
        };
        match frame {
            Ok(0) => problems.push(SlidesProblem::AtStart { stop: index }),
            Ok(video_frame) => found.push(ResolvedStop {
                video_frame,
                title: stop.title.clone(),
                notes: stop.notes.clone(),
                source: StopSource::Explicit(index),
            }),
            Err(reason) => problems.push(SlidesProblem::Unresolved {
                stop: index,
                reason,
            }),
        }
    }
    // Default stops at frame 0 (a one-frame first clip) go without a word.
    found.retain(|stop| stop.video_frame > 0);
    // Stable: of stops on one frame, the first in the document comes first.
    found.sort_by_key(|stop| stop.video_frame);
    let mut stops: Vec<ResolvedStop> = Vec::with_capacity(found.len() + 1);
    for stop in found {
        match stops.last() {
            Some(last) if last.video_frame == stop.video_frame => {
                if let StopSource::Explicit(index) = stop.source {
                    problems.push(SlidesProblem::Merged {
                        stop: index,
                        video_frame: stop.video_frame,
                    });
                }
            }
            _ => stops.push(stop),
        }
    }
    if let Some(last_frame) = timeline.total_frames.checked_sub(1)
        && stops
            .last()
            .is_none_or(|stop| stop.video_frame < last_frame)
    {
        stops.push(ResolvedStop {
            video_frame: last_frame,
            title: None,
            notes: None,
            source: StopSource::Default,
        });
    }
    (stops, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slides::doc::SlideStop;
    use fframes_studio_protocol::{
        PREVIEW_CONTRACT_VERSION, PreviewEnvelope, PreviewIdentity, PreviewSceneInfo,
    };

    /// A clip: `(key, start_frame, end_frame, trim_start_frames)`.
    type ClipSpec<'a> = (&'a str, usize, usize, usize);

    fn timeline(clips: &[ClipSpec], with_clip_data: bool) -> PreviewTimelineResponse {
        let total = clips.iter().map(|clip| clip.2).max().unwrap_or(0);
        PreviewTimelineResponse {
            envelope: PreviewEnvelope {
                contract_version: PREVIEW_CONTRACT_VERSION,
                identity: PreviewIdentity {
                    project_id: "p".into(),
                    open_session: "s".into(),
                    source_revision: "r".into(),
                    worker_generation: 1,
                },
                request_id: 1,
            },
            fps: 30,
            width: 1920,
            height: 1080,
            total_frames: total,
            duration_seconds: total as f32 / 30.,
            scenes: clips
                .iter()
                .enumerate()
                .map(|(index, (key, start, end, trim))| PreviewSceneInfo {
                    instance_id: format!("scene-{index}"),
                    editor_instance_key: Some((*key).into()),
                    index,
                    name: (*key).into(),
                    full_name: (*key).into(),
                    start_frame: *start,
                    end_frame: *end,
                    start_seconds: *start as f32 / 30.,
                    end_seconds: *end as f32 / 30.,
                    clip: with_clip_data.then(|| PreviewClipTiming {
                        timing_key: (*key).into(),
                        clip_index: 0,
                        trim_start_frames: *trim,
                        trim_end_frames: 0,
                        scene_index: Some(index),
                        scene_frames: Some(end - start + trim),
                    }),
                })
                .collect(),
            audio_tracks: vec![],
            timing_generation: 0,
        }
    }

    fn doc(stops: &[(&str, StopAt)]) -> SlidesDoc {
        SlidesDoc {
            version: 1,
            stops: stops
                .iter()
                .map(|(scene, at)| SlideStop {
                    scene: (*scene).into(),
                    at: *at,
                    title: Some(format!("{scene} {at:?}")),
                    notes: None,
                })
                .collect(),
        }
    }

    fn frames(stops: &[ResolvedStop]) -> Vec<usize> {
        stops.iter().map(|stop| stop.video_frame).collect()
    }

    /// The Slide deck template's stops all resolve on its deck's layout (the template's
    /// `timeline` command): six slides at 30 fps, each fading in over the last 12 frames
    /// of the one before.
    #[test]
    fn the_slide_deck_template_resolves_every_stop() {
        let template = SlidesDoc::parse(include_str!(
            "../../../studio-project/templates-slides/slides.json"
        ))
        .unwrap();
        let laid_out = timeline(
            &[
                ("intro", 0, 57, 0),
                ("agenda", 45, 150, 0),
                ("present", 138, 202, 0),
                ("outputs", 190, 268, 0),
                ("edit", 256, 361, 0),
                ("thanks", 349, 427, 0),
            ],
            true,
        );
        let (stops, problems) = resolve(&template, &laid_out);
        assert!(problems.is_empty(), "{problems:?}");
        // Bullets of `agenda` from video frame 45, of `edit` from 256; every "end" is the
        // frame before the next slide fades in.
        assert_eq!(
            frames(&stops),
            [44, 72, 90, 108, 137, 189, 255, 283, 301, 319, 348, 426]
        );
        assert!(stops.iter().all(|stop| stop.source != StopSource::Default));
    }

    #[test]
    fn a_document_without_stops_stops_at_every_clip_end_and_the_video_end() {
        let laid_out = timeline(
            &[("a", 0, 30, 0), ("b", 30, 90, 0), ("c", 90, 120, 0)],
            true,
        );
        let (stops, problems) = resolve(&SlidesDoc::default(), &laid_out);
        assert_eq!(frames(&stops), [29, 89, 119]);
        assert!(stops.iter().all(|stop| stop.source == StopSource::Default));
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_frame_of_a_clip_with_a_trimmed_start_maps_through_the_trim() {
        // `b` plays scene frames 10..70 from video frame 30.
        let laid_out = timeline(&[("a", 0, 30, 0), ("b", 30, 90, 10)], true);
        let (stops, problems) = resolve(&doc(&[("b", StopAt::Frame(40))]), &laid_out);
        assert_eq!(frames(&stops), [60, 89]);
        assert_eq!(stops[0].source, StopSource::Explicit(0));
        assert!(problems.is_empty());
        // A frame cut by the trim is unresolved, not moved to the clip start.
        let (stops, problems) = resolve(&doc(&[("b", StopAt::Frame(5))]), &laid_out);
        assert_eq!(frames(&stops), [89]);
        assert!(matches!(
            &problems[..],
            [SlidesProblem::Unresolved { stop: 0, reason }] if reason.contains("not played")
        ));
    }

    #[test]
    fn a_frame_inside_an_overlap_with_the_previous_clip_resolves() {
        // `b` cross-fades in over the last 10 frames of `a`: its range starts at 20.
        let laid_out = timeline(&[("a", 0, 30, 0), ("b", 20, 80, 0)], true);
        let (stops, problems) = resolve(&doc(&[("b", StopAt::Frame(5))]), &laid_out);
        assert_eq!(frames(&stops), [25, 79]);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn the_end_of_a_clip_lands_before_the_cross_fade_into_the_next() {
        let laid_out = timeline(&[("a", 0, 30, 0), ("b", 20, 80, 0)], true);
        let (stops, _) = resolve(&doc(&[("a", StopAt::End)]), &laid_out);
        assert_eq!(frames(&stops), [19, 79]);
        let (stops, _) = resolve(&SlidesDoc::default(), &laid_out);
        assert_eq!(frames(&stops), [19, 79], "default stops too");
    }

    #[test]
    fn a_reordered_timeline_sorts_stops_by_video_frame() {
        // `c` was moved first; the worker may list scenes in any order.
        let laid_out = timeline(
            &[("a", 30, 60, 0), ("b", 60, 120, 0), ("c", 0, 30, 0)],
            true,
        );
        let (stops, problems) = resolve(
            &doc(&[
                ("b", StopAt::End),
                ("a", StopAt::End),
                ("c", StopAt::Frame(10)),
            ]),
            &laid_out,
        );
        assert_eq!(frames(&stops), [10, 59, 119]);
        assert_eq!(
            stops.iter().map(|stop| stop.source).collect::<Vec<_>>(),
            [
                StopSource::Explicit(2),
                StopSource::Explicit(1),
                StopSource::Explicit(0)
            ]
        );
        assert!(problems.is_empty());
    }

    #[test]
    fn a_split_scene_resolves_frames_through_the_clip_that_draws_them() {
        // `b` (60 frames) split at 20: clip 1 plays 0..20, clip 2 plays 20..60 later.
        let laid_out = timeline(
            &[("b", 0, 20, 0), ("a", 20, 50, 0), ("b", 50, 90, 20)],
            true,
        );
        let (stops, _) = resolve(
            &doc(&[
                ("b", StopAt::Frame(10)),
                ("b", StopAt::Frame(30)),
                ("b", StopAt::End),
                ("b", StopAt::End),
            ]),
            &laid_out,
        );
        assert_eq!(frames(&stops), [10, 19, 60, 89]);
        // A third "end" of a scene with two clips has nothing to name.
        let (_, problems) = resolve(
            &doc(&[("b", StopAt::End), ("b", StopAt::End), ("b", StopAt::End)]),
            &laid_out,
        );
        assert!(matches!(
            &problems[..],
            [SlidesProblem::Unresolved { stop: 2, .. }]
        ));
    }

    #[test]
    fn stops_on_one_frame_merge_and_the_first_wins() {
        let laid_out = timeline(&[("a", 0, 30, 0), ("b", 30, 90, 0)], true);
        let (stops, problems) = resolve(
            &doc(&[("a", StopAt::Frame(29)), ("a", StopAt::End)]),
            &laid_out,
        );
        assert_eq!(frames(&stops), [29, 89]);
        assert_eq!(stops[0].source, StopSource::Explicit(0));
        assert_eq!(stops[0].title.as_deref(), Some("a Frame(29)"));
        assert_eq!(
            problems,
            [SlidesProblem::Merged {
                stop: 1,
                video_frame: 29
            }]
        );
        // A stop on the video's last frame replaces the implicit one silently.
        let (stops, problems) = resolve(&doc(&[("b", StopAt::End)]), &laid_out);
        assert_eq!(frames(&stops), [89]);
        assert_eq!(stops[0].source, StopSource::Explicit(0));
        assert!(problems.is_empty());
    }

    #[test]
    fn a_stop_on_the_first_frame_is_dropped_and_unknown_scenes_are_unresolved() {
        let laid_out = timeline(&[("a", 0, 30, 0)], true);
        let (stops, problems) = resolve(
            &doc(&[("a", StopAt::Frame(0)), ("missing", StopAt::End)]),
            &laid_out,
        );
        assert_eq!(frames(&stops), [29]);
        assert_eq!(problems[0], SlidesProblem::AtStart { stop: 0 });
        assert!(problems[1].is_unresolved());
        assert!(problems[1].to_string().contains("missing"));
    }

    #[test]
    fn a_timeline_without_clip_data_resolves_nothing() {
        let laid_out = timeline(&[("a", 0, 30, 0)], false);
        for document in [SlidesDoc::default(), doc(&[("a", StopAt::End)])] {
            let (stops, problems) = resolve(&document, &laid_out);
            assert!(stops.is_empty());
            assert_eq!(problems, [SlidesProblem::WorkerTooOld]);
            assert!(problems[0].to_string().contains("too old"));
        }
    }
}
