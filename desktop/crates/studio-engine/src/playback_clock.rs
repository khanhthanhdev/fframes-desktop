//! A presentation clock driven by predicted audio output timestamps.
//!
//! `OutputClockSnapshot` describes media samples submitted by one callback and the host-time
//! instant at which its first sample is predicted to reach the output. It is deliberately not a
//! claim about the exact physical DAC position.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputClockSnapshot {
    pub epoch: u64,
    pub start_sample: u64,
    pub sample_count: u64,
    pub sample_rate: u32,
    pub callback_seconds: f64,
    pub playback_seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackClockError {
    InvalidFps,
    InvalidTime,
    EpochExhausted,
}

impl fmt::Display for PlaybackClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFps => formatter.write_str("playback fps must be greater than zero"),
            Self::InvalidTime => formatter.write_str("playback host time must be finite"),
            Self::EpochExhausted => formatter.write_str("playback clock epoch exhausted"),
        }
    }
}

impl std::error::Error for PlaybackClockError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Waiting,
    Output,
    Fallback,
}

/// The single cursor owner for preview playback.
#[derive(Debug, Clone)]
pub struct PlaybackClock {
    fps: usize,
    total_frames: usize,
    position: usize,
    epoch: u64,
    playing: bool,
    source: Source,
    epoch_start_position: usize,
    fallback_anchor: f64,
    output: Option<OutputState>,
}

#[derive(Debug, Clone)]
struct OutputState {
    snapshot: OutputClockSnapshot,
    submitted_end: u64,
    mapped_sample: f64,
}

impl PlaybackClock {
    /// Replace a revision without ever reusing an older output epoch.
    pub fn reinstall(
        &mut self,
        fps: usize,
        total_frames: usize,
        position: usize,
        now: f64,
    ) -> Result<(), PlaybackClockError> {
        if fps == 0 {
            return Err(PlaybackClockError::InvalidFps);
        }
        validate_time(now)?;
        self.advance_epoch()?;
        self.fps = fps;
        self.total_frames = total_frames;
        self.position = position.min(total_frames);
        self.epoch_start_position = self.position;
        self.playing &= self.position < total_frames;
        self.fallback_anchor = now;
        self.source = Source::Waiting;
        self.output = None;
        Ok(())
    }

    pub fn install(
        fps: usize,
        total_frames: usize,
        position: usize,
        now: f64,
    ) -> Result<Self, PlaybackClockError> {
        if fps == 0 {
            return Err(PlaybackClockError::InvalidFps);
        }
        validate_time(now)?;
        let position = position.min(total_frames);
        Ok(Self {
            fps,
            total_frames,
            position,
            epoch: 1,
            playing: false,
            source: Source::Waiting,
            epoch_start_position: position,
            fallback_anchor: now,
            output: None,
        })
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn seconds(&self) -> f64 {
        self.position as f64 / self.fps as f64
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn seek(&mut self, frame: usize, now: f64) -> Result<(), PlaybackClockError> {
        validate_time(now)?;
        self.advance_epoch()?;
        self.position = frame.min(self.total_frames);
        self.playing &= self.position < self.total_frames;
        self.epoch_start_position = self.position;
        self.fallback_anchor = now;
        self.source = Source::Waiting;
        self.output = None;
        Ok(())
    }

    pub fn toggle(&mut self, now: f64) -> Result<(), PlaybackClockError> {
        if self.playing {
            self.pause(now)
        } else {
            validate_time(now)?;
            if self.position == self.total_frames && self.total_frames != 0 {
                self.seek(0, now)?;
            }
            self.playing = self.total_frames != 0;
            self.fallback_anchor = now;
            Ok(())
        }
    }

    pub fn pause(&mut self, now: f64) -> Result<(), PlaybackClockError> {
        validate_time(now)?;
        if self.playing {
            self.tick(now);
        }
        self.advance_epoch()?;
        self.playing = false;
        self.epoch_start_position = self.position;
        self.fallback_anchor = now;
        self.source = Source::Waiting;
        self.output = None;
        Ok(())
    }

    /// Return the inclusive-end cursor. `total_frames - 1` remains the final renderable frame.
    pub fn tick(&mut self, now: f64) -> usize {
        if !self.playing || !now.is_finite() {
            return self.position;
        }
        let next = match self.source {
            Source::Waiting => self.position,
            Source::Fallback => {
                let elapsed = (now - self.fallback_anchor).max(0.0);
                self.epoch_start_position
                    .saturating_add(seconds_to_frames(elapsed, self.fps))
            }
            Source::Output => self.output_position(now),
        };
        self.position = self.position.max(next.min(self.total_frames));
        if self.position == self.total_frames {
            self.playing = false;
        }
        self.position
    }

    /// Freeze while a stream is being primed, without changing play/pause intent.
    pub fn wait_for_output(&mut self) -> Result<(), PlaybackClockError> {
        self.advance_epoch()?;
        self.source = Source::Waiting;
        self.output = None;
        self.epoch_start_position = self.position;
        Ok(())
    }

    /// Switch from output timestamps to the monotonic host clock at the last mapped position.
    pub fn fallback(&mut self, now: f64) -> Result<(), PlaybackClockError> {
        validate_time(now)?;
        if self.playing && self.source == Source::Output {
            self.tick(now);
        }
        self.advance_epoch()?;
        self.epoch_start_position = self.position;
        self.fallback_anchor = now;
        self.source = Source::Fallback;
        self.output = None;
        Ok(())
    }

    /// Accept a timestamp only when it is usable and belongs to the active clock epoch.
    pub fn accept_output(&mut self, snapshot: OutputClockSnapshot) -> bool {
        if snapshot.epoch != self.epoch
            || !(8000..=192000).contains(&snapshot.sample_rate)
            || !snapshot.callback_seconds.is_finite()
            || !snapshot.playback_seconds.is_finite()
            || snapshot.playback_seconds < snapshot.callback_seconds
        {
            return false;
        }
        let Some(end) = snapshot.start_sample.checked_add(snapshot.sample_count) else {
            return false;
        };
        if let Some(output) = &self.output
            && (snapshot.callback_seconds < output.snapshot.callback_seconds
                || snapshot.playback_seconds < output.snapshot.playback_seconds
                || snapshot.sample_rate != output.snapshot.sample_rate
                || end < output.submitted_end)
        {
            return false;
        }
        let submitted_end = self
            .output
            .as_ref()
            .map_or(end, |old| old.submitted_end.max(end));
        let mapped_sample = self.output.as_ref().map_or(0.0, |old| old.mapped_sample);
        self.output = Some(OutputState {
            snapshot,
            submitted_end,
            mapped_sample,
        });
        self.source = Source::Output;
        true
    }

    fn output_position(&mut self, now: f64) -> usize {
        let Some(output) = &mut self.output else {
            return self.position;
        };
        let rate = output.snapshot.sample_rate as f64;
        let mapped = (output.snapshot.start_sample as f64
            + (now - output.snapshot.playback_seconds) * rate)
            .clamp(0.0, output.submitted_end as f64);
        output.mapped_sample = output.mapped_sample.max(mapped);
        let elapsed_frames = (output.mapped_sample * self.fps as f64 / rate).floor();
        self.epoch_start_position
            .saturating_add(elapsed_frames.min(usize::MAX as f64) as usize)
    }

    fn advance_epoch(&mut self) -> Result<(), PlaybackClockError> {
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or(PlaybackClockError::EpochExhausted)?;
        Ok(())
    }
}

fn validate_time(now: f64) -> Result<(), PlaybackClockError> {
    now.is_finite()
        .then_some(())
        .ok_or(PlaybackClockError::InvalidTime)
}

fn seconds_to_frames(seconds: f64, fps: usize) -> usize {
    (seconds * fps as f64).floor().clamp(0.0, usize::MAX as f64) as usize
}
