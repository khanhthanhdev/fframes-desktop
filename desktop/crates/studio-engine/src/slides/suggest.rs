//! Bounded scan planning and resolution of untrusted component stop suggestions.
use std::collections::BTreeSet;

use fframes_studio_protocol::{PreviewTimelineResponse, StopSuggestions};

use super::{SlideStop, SlidesDoc, StopAt, StopSource, resolve};

/// Maximum sampled frames per explicit scan, independent of video length.
pub const MAX_SCAN_SAMPLES: usize = 2000;

/// Clip starts first, then half-second samples. Work and allocation are bounded.
pub fn sample_frames(timeline: &PreviewTimelineResponse) -> (Vec<usize>, bool) {
    let mut frames = BTreeSet::new();
    let mut limited = false;
    for scene in &timeline.scenes {
        if scene.start_frame < timeline.total_frames {
            if frames.len() == MAX_SCAN_SAMPLES && !frames.contains(&scene.start_frame) {
                limited = true;
                break;
            }
            frames.insert(scene.start_frame);
        }
    }
    // Integer half-second positions avoid cumulative rounding drift at odd frame rates.
    if timeline.fps == 0 {
        return (frames.into_iter().collect(), limited);
    }
    let mut half_seconds = 0usize;
    loop {
        let frame =
            (half_seconds as u128 * timeline.fps as u128 / 2).min(usize::MAX as u128) as usize;
        if frame >= timeline.total_frames {
            break;
        }
        if frames.len() == MAX_SCAN_SAMPLES && !frames.contains(&frame) {
            limited = true;
            break;
        }
        frames.insert(frame);
        half_seconds += 1;
    }
    (frames.into_iter().collect(), limited)
}

/// A proposal mapped to an editable scene-frame stop and its visible video frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopProposal {
    pub stop: SlideStop,
    pub video_frame: usize,
}

/// Revalidates input, maps video views through trims, resolves, and removes accepted stops.
/// In overlaps the later-starting clip owns video-view suggestions. Split/repeated scenes
/// still obey the document resolver's first matching occurrence rule.
pub fn proposals(
    suggestions: &StopSuggestions,
    timeline: &PreviewTimelineResponse,
    accepted: &SlidesDoc,
) -> Vec<StopProposal> {
    if timeline.fps == 0 {
        return Vec::new();
    }
    let mut bounded = suggestions.clone();
    bounded.bound();
    let mut occupied: BTreeSet<_> = resolve::resolve(accepted, timeline)
        .0
        .into_iter()
        .filter(|stop| matches!(stop.source, StopSource::Explicit(_)))
        .map(|stop| stop.video_frame)
        .collect();
    let mut result = Vec::new();
    for suggested in bounded.stops {
        // A stop holds after a reveal completes, never on the preceding frame.
        // Match the template's frame_at tolerance for f32 round trips at exact frames.
        let frame = (f64::from(suggested.seconds) * timeline.fps as f64 - 1e-3)
            .ceil()
            .max(0.0);
        if frame >= usize::MAX as f64 {
            continue;
        }
        let frame = frame as usize;
        let (scene, at, expected_video) =
            if suggested.scene == "video" && timeline.scenes.is_empty() {
                (suggested.scene, frame, Some(frame))
            } else if suggested.scene == "video" {
                let Some(scene) = timeline
                    .scenes
                    .iter()
                    .filter(|scene| scene.start_frame <= frame && frame < scene.end_frame)
                    .max_by_key(|scene| scene.start_frame)
                else {
                    continue;
                };
                let Some(clip) = &scene.clip else {
                    continue;
                };
                (
                    clip.timing_key.clone(),
                    clip.trim_start_frames
                        .saturating_add(frame - scene.start_frame),
                    Some(frame),
                )
            } else {
                (suggested.scene, frame, None)
            };
        let stop = SlideStop {
            scene,
            at: StopAt::Frame(at),
            title: suggested.label,
            notes: suggested.notes,
        };
        let doc = SlidesDoc {
            stops: vec![stop.clone()],
            ..SlidesDoc::default()
        };
        if doc.validate().is_err() {
            continue;
        }
        let Some(resolved) = resolve::resolve(&doc, timeline)
            .0
            .into_iter()
            .find(|stop| stop.source == StopSource::Explicit(0))
        else {
            continue;
        };
        if expected_video.is_some_and(|frame| frame != resolved.video_frame)
            || !occupied.insert(resolved.video_frame)
        {
            continue;
        }
        result.push(StopProposal {
            stop,
            video_frame: resolved.video_frame,
        });
    }
    result.sort_by_key(|proposal| proposal.video_frame);
    result
}

/// Builds a whole acceptance edit or explains why document limits refuse it. No I/O.
pub fn accepting(doc: &SlidesDoc, proposals: &[StopProposal]) -> Result<SlidesDoc, String> {
    let mut next = doc.clone();
    next.stops
        .extend(proposals.iter().map(|proposal| proposal.stop.clone()));
    next.validate()?;
    next.check_size()?;
    Ok(next)
}
