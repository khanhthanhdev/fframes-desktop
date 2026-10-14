//! Editor timing applied on top of the scenes a video defines in code.
//!
//! A [`TimingDoc`] reorders, trims and splits the scenes returned by
//! `Video::define_scenes` without changing the code: the previewer and the renderer
//! lay out the clips of the document instead of the code order. Scenes keep seeing
//! their own frame numbers, so a trimmed scene renders exactly the frames it would
//! have rendered at those positions without the trim.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::Scene;

/// The only document version this build reads.
pub const TIMING_DOC_VERSION: u32 = 1;
/// Upper bound on clips in one document.
pub const MAX_TIMING_CLIPS: usize = 4096;
/// Upper bound on the laid out timeline: 24 hours at the video's frame rate.
pub const MAX_TIMING_SECONDS: usize = 24 * 60 * 60;
/// Upper bound on the bytes of one clip key.
pub const MAX_TIMING_KEY_BYTES: usize = 512;

/// Play order, trims and splits of a video's code scenes.
///
/// Clips play in `clips` order. A split is two clips with the same `key` and trims
/// that select different parts of the scene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingDoc {
    pub version: u32,
    pub clips: Vec<TimingClip>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingClip {
    /// The scene's timing key, see [`scene_timing_keys`].
    pub key: String,
    /// Frames cut from the start of the scene.
    pub trim_start_frames: usize,
    /// Frames cut from the end of the scene.
    pub trim_end_frames: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimingError {
    Version(u32),
    TooManyClips(usize),
    InvalidKey(String),
    /// `trim_start + trim_end` is not below the scene's length.
    TrimTooLong {
        key: String,
        trim_start_frames: usize,
        trim_end_frames: usize,
        scene_frames: usize,
    },
    TooLong(usize),
}

impl fmt::Display for TimingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(v) => write!(
                f,
                "timing version {v} is not supported (expected {TIMING_DOC_VERSION})"
            ),
            Self::TooManyClips(n) => write!(f, "{n} clips exceed the limit of {MAX_TIMING_CLIPS}"),
            Self::InvalidKey(key) => write!(f, "invalid clip key {key:?}"),
            Self::TrimTooLong {
                key,
                trim_start_frames,
                trim_end_frames,
                scene_frames,
            } => write!(
                f,
                "clip {key:?} trims {trim_start_frames}+{trim_end_frames} frames of a {scene_frames} frame scene"
            ),
            Self::TooLong(frames) => write!(
                f,
                "timeline of {frames} frames exceeds {MAX_TIMING_SECONDS} seconds"
            ),
        }
    }
}

impl std::error::Error for TimingError {}

impl TimingDoc {
    /// Checks everything that does not depend on the video's scenes.
    pub fn validate(&self) -> Result<(), TimingError> {
        if self.version != TIMING_DOC_VERSION {
            return Err(TimingError::Version(self.version));
        }
        if self.clips.len() > MAX_TIMING_CLIPS {
            return Err(TimingError::TooManyClips(self.clips.len()));
        }
        for clip in &self.clips {
            if clip.key.is_empty()
                || clip.key.len() > MAX_TIMING_KEY_BYTES
                || clip.key.chars().any(char::is_control)
            {
                return Err(TimingError::InvalidKey(clip.key.clone()));
            }
        }
        Ok(())
    }
}

/// What applying a document changed besides the layout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TimingReport {
    /// Clip keys that match no code scene; those clips are skipped.
    pub orphan_clips: Vec<String>,
    /// Code scenes the document does not mention; they are appended untrimmed.
    pub appended_scenes: Vec<String>,
}

/// Timing keys of code scenes: `editor_instance_key()` if present, else `name()`.
/// The second and later scenes with the same key get `#2`, `#3`, ... in code order,
/// so adding a scene instance never renames the existing ones.
pub fn scene_timing_keys<'s>(
    scenes: impl IntoIterator<Item = &'s (dyn Scene + 's)>,
) -> Vec<String> {
    let mut seen = HashMap::<String, usize>::new();
    scenes
        .into_iter()
        .map(|scene| {
            let base = scene
                .editor_instance_key()
                .map_or_else(|| scene.name().to_owned(), str::to_owned);
            let count = seen.entry(base.clone()).or_default();
            *count += 1;
            if *count == 1 {
                base
            } else {
                format!("{base}#{count}")
            }
        })
        .collect()
}

/// One clip to lay out: which code scene and which part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedClip {
    /// Index into the code scenes.
    pub scene_index: usize,
    /// Occurrence of this scene among the clips so far (0 for the first clip).
    pub clip_index: usize,
    pub trim_start: usize,
    pub trim_end: usize,
}

/// Resolves a document against the code scenes and their lengths in frames.
/// Any error rejects the whole document.
pub(crate) fn plan_clips(
    doc: Option<&TimingDoc>,
    keys: &[String],
    scene_frames: &[usize],
) -> Result<(Vec<PlannedClip>, TimingReport), TimingError> {
    let Some(doc) = doc else {
        let clips = (0..keys.len())
            .map(|scene_index| PlannedClip {
                scene_index,
                clip_index: 0,
                trim_start: 0,
                trim_end: 0,
            })
            .collect();
        return Ok((clips, TimingReport::default()));
    };
    doc.validate()?;

    let index_of: HashMap<&str, usize> = keys
        .iter()
        .enumerate()
        .map(|(i, key)| (key.as_str(), i))
        .collect();
    let mut occurrences = vec![0usize; keys.len()];
    let mut report = TimingReport::default();
    let mut clips = Vec::with_capacity(doc.clips.len());

    for clip in &doc.clips {
        let Some(&scene_index) = index_of.get(clip.key.as_str()) else {
            report.orphan_clips.push(clip.key.clone());
            continue;
        };
        let trimmed = clip
            .trim_start_frames
            .checked_add(clip.trim_end_frames)
            .filter(|trimmed| *trimmed < scene_frames[scene_index]);
        if trimmed.is_none() {
            return Err(TimingError::TrimTooLong {
                key: clip.key.clone(),
                trim_start_frames: clip.trim_start_frames,
                trim_end_frames: clip.trim_end_frames,
                scene_frames: scene_frames[scene_index],
            });
        }
        clips.push(PlannedClip {
            scene_index,
            clip_index: occurrences[scene_index],
            trim_start: clip.trim_start_frames,
            trim_end: clip.trim_end_frames,
        });
        occurrences[scene_index] += 1;
    }

    let mentioned: HashSet<usize> = clips.iter().map(|c| c.scene_index).collect();
    for (scene_index, key) in keys.iter().enumerate() {
        if !mentioned.contains(&scene_index) {
            report.appended_scenes.push(key.clone());
            clips.push(PlannedClip {
                scene_index,
                clip_index: 0,
                trim_start: 0,
                trim_end: 0,
            });
        }
    }
    Ok((clips, report))
}

/// Editor object key of a clip: the scene's key for its first clip, `key#n` for the
/// n-th later clip of a split scene, so canvas selection stays unique after a split
/// and an unsplit scene keeps the key its source anchors use.
pub fn clip_editor_key(scene_key: &str, clip_index: usize) -> String {
    if clip_index == 0 {
        scene_key.to_owned()
    } else {
        format!("{scene_key}#{clip_index}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(clips: &[(&str, usize, usize)]) -> TimingDoc {
        TimingDoc {
            version: TIMING_DOC_VERSION,
            clips: clips
                .iter()
                .map(|(key, s, e)| TimingClip {
                    key: (*key).to_owned(),
                    trim_start_frames: *s,
                    trim_end_frames: *e,
                })
                .collect(),
        }
    }

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn rejects_unknown_fields_and_versions() {
        let unknown = r#"{"version":1,"clips":[],"extra":true}"#;
        assert!(serde_json::from_str::<TimingDoc>(unknown).is_err());
        let unknown_clip = r#"{"version":1,"clips":[{"key":"a","trim_start_frames":0,"trim_end_frames":0,"x":1}]}"#;
        assert!(serde_json::from_str::<TimingDoc>(unknown_clip).is_err());
        let v2: TimingDoc = serde_json::from_str(r#"{"version":2,"clips":[]}"#).unwrap();
        assert_eq!(v2.validate(), Err(TimingError::Version(2)));
        let negative =
            r#"{"version":1,"clips":[{"key":"a","trim_start_frames":-1,"trim_end_frames":0}]}"#;
        assert!(serde_json::from_str::<TimingDoc>(negative).is_err());
    }

    #[test]
    fn rejects_whole_doc_on_one_bad_clip() {
        let result = plan_clips(
            Some(&doc(&[("a", 0, 0), ("b", 5, 5)])),
            &keys(&["a", "b"]),
            &[10, 10],
        );
        assert!(matches!(result, Err(TimingError::TrimTooLong { .. })));
        let overflow = plan_clips(Some(&doc(&[("a", usize::MAX, 1)])), &keys(&["a"]), &[10]);
        assert!(matches!(overflow, Err(TimingError::TrimTooLong { .. })));
        let too_many = doc(&vec![("a", 0, 0); MAX_TIMING_CLIPS + 1]);
        assert_eq!(
            plan_clips(Some(&too_many), &keys(&["a"]), &[10]).unwrap_err(),
            TimingError::TooManyClips(MAX_TIMING_CLIPS + 1)
        );
        assert!(matches!(
            plan_clips(Some(&doc(&[("", 0, 0)])), &keys(&["a"]), &[10]),
            Err(TimingError::InvalidKey(_))
        ));
    }

    #[test]
    fn orders_splits_reports_orphans_and_appends_missing() {
        let (clips, report) = plan_clips(
            Some(&doc(&[
                ("c", 0, 0),
                ("gone", 0, 0),
                ("a", 0, 6),
                ("a", 4, 0),
            ])),
            &keys(&["a", "b", "c"]),
            &[10, 10, 10],
        )
        .unwrap();
        let shape: Vec<_> = clips
            .iter()
            .map(|c| (c.scene_index, c.clip_index, c.trim_start, c.trim_end))
            .collect();
        assert_eq!(
            shape,
            [(2, 0, 0, 0), (0, 0, 0, 6), (0, 1, 4, 0), (1, 0, 0, 0)]
        );
        assert_eq!(report.orphan_clips, ["gone"]);
        assert_eq!(report.appended_scenes, ["b"]);
    }

    #[test]
    fn repeated_keys_number_later_instances() {
        #[derive(Debug)]
        struct S(Option<&'static str>);
        impl Scene for S {
            fn duration(&self) -> crate::Duration<'_> {
                crate::Duration::Frames(1)
            }
            fn render_frame<'a>(
                &'a self,
                _: crate::Frame,
                _: &crate::FFramesContext<'a, '_>,
            ) -> crate::Svgr<'a> {
                crate::Svgr::empty()
            }
            fn editor_instance_key(&self) -> Option<&str> {
                self.0
            }
        }
        let scenes = [S(Some("intro")), S(None), S(Some("intro")), S(None)];
        let name = std::any::type_name::<S>();
        assert_eq!(
            scene_timing_keys(scenes.iter().map(|s| s as &dyn Scene)),
            [
                "intro".to_owned(),
                name.to_owned(),
                "intro#2".to_owned(),
                format!("{name}#2")
            ]
        );
        assert_eq!(clip_editor_key("intro", 0), "intro");
        assert_eq!(clip_editor_key("intro", 1), "intro#1");
    }
}
