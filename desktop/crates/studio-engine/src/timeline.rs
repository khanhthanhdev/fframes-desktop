//! Pure compiled-timeline validation, geometry and selection arithmetic.

use std::ops::Range;
use std::{cmp::Reverse, collections::BinaryHeap};

use fframes_studio_protocol::{PREVIEW_CONTRACT_VERSION, PreviewTimelineResponse};

const DEFAULT_PIXELS_PER_SECOND: f64 = 100.0;
const MIN_PIXELS_PER_SECOND: f64 = 0.01;
const MAX_PIXELS_PER_SECOND: f64 = 100_000.0;
const MAX_VIEWPORT_WIDTH: f64 = 1_000_000.0;
const MAX_GEOMETRY_ITEMS: usize = 100_000;
const MAX_TICKS: usize = 2_048;

#[derive(Debug, Clone)]
pub struct TimelineModel {
    report: PreviewTimelineResponse,
    scene_lanes: Vec<usize>,
}

impl TimelineModel {
    pub fn new(report: PreviewTimelineResponse) -> Result<Self, String> {
        validate_report(&report)?;
        let mut scene_lanes = vec![0; report.scenes.len()];
        let mut order: Vec<_> = (0..report.scenes.len())
            .filter(|index| report.scenes[*index].start_frame < report.scenes[*index].end_frame)
            .collect();
        order.sort_unstable_by_key(|index| {
            let scene = &report.scenes[*index];
            (scene.start_frame, scene.end_frame, *index)
        });
        let mut active = BinaryHeap::<Reverse<(usize, usize)>>::new();
        let mut available = BinaryHeap::<Reverse<usize>>::new();
        let mut lane_count = 0;
        for index in order {
            let scene = &report.scenes[index];
            while active
                .peek()
                .is_some_and(|Reverse((end, _))| *end <= scene.start_frame)
            {
                let Reverse((_, lane)) = active.pop().expect("peeked active lane");
                available.push(Reverse(lane));
            }
            let lane = available.pop().map_or_else(
                || {
                    let lane = lane_count;
                    lane_count += 1;
                    lane
                },
                |Reverse(lane)| lane,
            );
            scene_lanes[index] = lane;
            active.push(Reverse((scene.end_frame, lane)));
        }
        Ok(Self {
            report,
            scene_lanes,
        })
    }

    pub fn report(&self) -> &PreviewTimelineResponse {
        &self.report
    }

    /// Seconds at a cursor frame. The cursor may be exactly at the video end.
    pub fn seconds(&self, frame: usize) -> f64 {
        frame.min(self.report.total_frames) as f64 / self.report.fps as f64
    }

    /// Convert seconds to the nearest inclusive-end cursor frame.
    pub fn frame_at_seconds(&self, seconds: f64) -> Option<usize> {
        if !seconds.is_finite() || self.report.total_frames == 0 {
            return None;
        }
        if seconds <= 0.0 {
            return Some(0);
        }
        if seconds >= self.seconds(self.report.total_frames) {
            return Some(self.report.total_frames);
        }
        let frame = (seconds.max(0.0) * self.report.fps as f64).round();
        if !frame.is_finite() {
            return None;
        }
        Some(frame.clamp(0.0, self.report.total_frames as f64) as usize)
    }

    pub fn time_label(&self, frame: usize) -> String {
        let frame = frame.min(self.report.total_frames);
        format!("{:.3}s / frame {frame}", self.seconds(frame))
    }

    /// Scene indices containing `frame`, in compiled report order.
    pub fn scene_hits(&self, frame: usize) -> Vec<usize> {
        self.report
            .scenes
            .iter()
            .enumerate()
            .filter_map(|(index, scene)| {
                (scene.start_frame <= frame && frame < scene.end_frame).then_some(index)
            })
            .collect()
    }
}

fn validate_report(report: &PreviewTimelineResponse) -> Result<(), String> {
    crate::validate_preview_timeline(report)?;
    if report.envelope.contract_version != PREVIEW_CONTRACT_VERSION
        || report.envelope.request_id == 0
        || report
            .scenes
            .len()
            .saturating_add(report.audio_tracks.len())
            > MAX_GEOMETRY_ITEMS
    {
        return Err("Invalid compiled timeline timebase/dimensions".into());
    }
    for audio in &report.audio_tracks {
        if !(audio.start_seconds * MAX_PIXELS_PER_SECOND).is_finite()
            || !((audio.end_seconds - audio.start_seconds) * MAX_PIXELS_PER_SECOND).is_finite()
        {
            return Err("Compiled audio geometry overflows".into());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineRect {
    pub index: usize,
    pub x: f64,
    pub width: f64,
    pub lane: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineTick {
    pub x: f64,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineGeometry {
    pub scenes: Vec<TimelineRect>,
    pub audio: Vec<TimelineRect>,
    pub ticks: Vec<TimelineTick>,
    /// Reserve every compiled scene lane, including lanes outside the horizontal viewport.
    pub scene_lane_count: usize,
    pub width: f64,
}

#[derive(Debug, Clone)]
pub struct TimelineViewport {
    pub width: f64,
    pub pixels_per_second: f64,
    pub scroll_x: f64,
}

impl Default for TimelineViewport {
    fn default() -> Self {
        Self {
            width: 0.0,
            pixels_per_second: DEFAULT_PIXELS_PER_SECOND,
            scroll_x: 0.0,
        }
    }
}

impl TimelineViewport {
    pub fn resize(&mut self, width: f64, model: &TimelineModel) {
        if width.is_finite() && width >= 0.0 {
            self.width = width.min(MAX_VIEWPORT_WIDTH);
        }
        self.clamp(model);
    }

    pub fn fit(&mut self, model: &TimelineModel) {
        let duration = model.seconds(model.report.total_frames);
        self.pixels_per_second = if duration > 0.0 && self.width > 0.0 {
            (self.width / duration).clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND)
        } else {
            DEFAULT_PIXELS_PER_SECOND
        };
        self.scroll_x = 0.0;
        self.clamp(model);
    }

    pub fn zoom(&mut self, factor: f64, anchor_x: f64, model: &TimelineModel) {
        if !factor.is_finite() || factor <= 0.0 || !anchor_x.is_finite() {
            return;
        }
        let anchor = anchor_x.clamp(0.0, self.width);
        let seconds = (self.scroll_x + anchor) / self.pixels_per_second;
        let next =
            (self.pixels_per_second * factor).clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);
        if !next.is_finite() {
            return;
        }
        self.pixels_per_second = next;
        self.scroll_x = seconds.mul_add(next, -anchor);
        self.clamp(model);
    }

    pub fn scroll_by(&mut self, dx: f64, model: &TimelineModel) {
        if dx.is_finite() {
            self.scroll_x = match self.scroll_x + dx {
                value if value.is_finite() => value,
                _ if dx.is_sign_positive() => f64::MAX,
                _ => 0.0,
            };
            self.clamp(model);
        }
    }

    pub fn frame_at_x(&self, x: f64, model: &TimelineModel) -> Option<usize> {
        if !x.is_finite() {
            return None;
        }
        let position = self.scroll_x + x;
        if !position.is_finite() {
            return Some(if x.is_sign_positive() {
                model.report.total_frames
            } else {
                0
            });
        }
        model.frame_at_seconds(position / self.pixels_per_second)
    }

    pub fn x_at_frame(&self, frame: usize, model: &TimelineModel) -> f64 {
        model
            .seconds(frame)
            .mul_add(self.pixels_per_second, -self.scroll_x)
    }

    pub fn geometry(&self, model: &TimelineModel) -> TimelineGeometry {
        let timeline_width = self.timeline_width(model);
        let visible = |x: f64, width: f64| x + width >= 0.0 && x <= self.width;
        let scenes = model
            .report
            .scenes
            .iter()
            .enumerate()
            .filter_map(|(index, scene)| {
                let x = self.x_at_frame(scene.start_frame, model);
                let width = (scene.end_frame - scene.start_frame) as f64 / model.report.fps as f64
                    * self.pixels_per_second;
                visible(x, width).then_some(TimelineRect {
                    index,
                    x,
                    width,
                    lane: model.scene_lanes[index],
                })
            })
            .collect();
        let audio = model
            .report
            .audio_tracks
            .iter()
            .enumerate()
            .filter_map(|(index, track)| {
                let x = track
                    .start_seconds
                    .mul_add(self.pixels_per_second, -self.scroll_x);
                let width = (track.end_seconds - track.start_seconds) * self.pixels_per_second;
                visible(x, width).then_some(TimelineRect {
                    index,
                    x,
                    width,
                    lane: index,
                })
            })
            .collect();
        TimelineGeometry {
            scenes,
            audio,
            ticks: self.ticks(model),
            scene_lane_count: model
                .scene_lanes
                .iter()
                .map(|lane| lane + 1)
                .max()
                .unwrap_or(1),
            width: timeline_width,
        }
    }

    pub fn thumbnail_frames(&self, model: &TimelineModel) -> Vec<usize> {
        if model.report.total_frames == 0 || self.width <= 0.0 {
            return Vec::new();
        }
        let first = self.frame_at_x(0.0, model).unwrap_or(0).saturating_sub(1);
        let last = self
            .frame_at_x(self.width, model)
            .unwrap_or(model.report.total_frames)
            .saturating_add(1)
            .min(model.report.total_frames - 1);
        if first >= last {
            return vec![first.min(model.report.total_frames - 1)];
        }
        let count = (last - first + 1).min(12);
        (0..count)
            .map(|i| {
                first + (i as u128 * (last - first) as u128 / (count - 1).max(1) as u128) as usize
            })
            .collect()
    }

    fn ticks(&self, model: &TimelineModel) -> Vec<TimelineTick> {
        if self.width <= 0.0 || model.report.total_frames == 0 {
            return Vec::new();
        }
        let raw_step = 80.0 / self.pixels_per_second;
        let power = 10_f64.powf(raw_step.max(f64::MIN_POSITIVE).log10().floor());
        let step = [1.0, 2.0, 5.0, 10.0]
            .into_iter()
            .map(|multiple| multiple * power)
            .find(|candidate| *candidate >= raw_step)
            .unwrap_or(power * 10.0);
        let start_seconds = (self.scroll_x / self.pixels_per_second).max(0.0);
        let end_seconds = ((self.scroll_x + self.width) / self.pixels_per_second)
            .min(model.seconds(model.report.total_frames));
        let first = (start_seconds / step).ceil();
        let count = (((end_seconds / step).floor() - first + 1.0).max(0.0) as usize).min(MAX_TICKS);
        (0..count)
            .map(|index| {
                let seconds = (first + index as f64) * step;
                TimelineTick {
                    x: seconds.mul_add(self.pixels_per_second, -self.scroll_x),
                    label: format!("{seconds:.2}s"),
                }
            })
            .collect()
    }

    fn timeline_width(&self, model: &TimelineModel) -> f64 {
        model.seconds(model.report.total_frames) * self.pixels_per_second
    }

    fn clamp(&mut self, model: &TimelineModel) {
        self.width = self.width.clamp(0.0, MAX_VIEWPORT_WIDTH);
        self.pixels_per_second = self
            .pixels_per_second
            .clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);
        let maximum = (self.timeline_width(model) - self.width).max(0.0);
        self.scroll_x = if self.scroll_x.is_finite() {
            self.scroll_x.clamp(0.0, maximum)
        } else {
            0.0
        };
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineSelection {
    pub scene_id: Option<String>,
    pub range: Option<Range<usize>>,
}

impl TimelineSelection {
    pub fn select_scene(&mut self, index: usize, model: &TimelineModel) {
        if let Some(scene) = model.report.scenes.get(index) {
            self.scene_id = Some(scene.instance_id.clone());
            self.range = Some(scene.start_frame..scene.end_frame);
        }
    }

    pub fn select_range(&mut self, a: usize, b: usize, model: &TimelineModel) {
        let start = a.min(b).min(model.report.total_frames);
        let end = a.max(b).min(model.report.total_frames);
        self.scene_id = None;
        self.range = Some(start..end);
    }

    pub fn clamp(&mut self, model: &TimelineModel) {
        if self.scene_id.as_ref().is_some_and(|id| {
            !model
                .report
                .scenes
                .iter()
                .any(|scene| &scene.instance_id == id)
        }) {
            self.scene_id = None;
        }
        if let Some(range) = &mut self.range {
            range.start = range.start.min(model.report.total_frames);
            range.end = range.end.min(model.report.total_frames);
            if range.start > range.end {
                std::mem::swap(&mut range.start, &mut range.end);
            }
        }
    }

    pub fn cycle_scene_at(&mut self, frame: usize, model: &TimelineModel) {
        let hits = model.scene_hits(frame);
        if hits.is_empty() {
            self.scene_id = None;
            return;
        }
        let current = self.scene_id.as_deref();
        let next_position = hits
            .iter()
            .position(|index| Some(model.report.scenes[*index].instance_id.as_str()) == current)
            .map_or(0, |position| (position + 1) % hits.len());
        self.select_scene(hits[next_position], model);
    }
}
