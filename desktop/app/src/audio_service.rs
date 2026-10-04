//! App-owned CPAL output. Native stream operations and positioned PCM reads run on
//! the owner/feeder threads. The callback uses only fixed storage and atomics.
use std::{
    fs::File,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use fframes_studio_protocol::PreviewIdentity;
use parking_lot::Mutex;
use studio_engine::{OutputClockSnapshot, ReadyPreview};

pub const READER_BYTES: usize = 65536;
pub const MAX_OUTPUT_RATE: u32 = 192000;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OutputDevice {
    #[default]
    Default,
    Named(String),
    Unavailable,
}

struct Request {
    ready: Arc<ReadyPreview>,
    epoch: u64,
    position: usize,
    device: OutputDevice,
}

#[expect(
    clippy::large_enum_variant,
    reason = "One bounded event mailbox; no event queue or callback allocation"
)]
pub enum AudioEvent {
    Staged {
        epoch: u64,
        identity: PreviewIdentity,
        output: Option<OutputHandle>,
        unavailable: Option<String>,
    },
    Failed {
        epoch: u64,
        error: String,
    },
    Lost {
        epoch: u64,
    },
}

#[derive(Default)]
struct Mailbox {
    request: Option<Request>,
    event: Option<AudioEvent>,
    devices: Vec<String>,
}

struct Control {
    wanted: AtomicU64,
    committed: AtomicU64,
    playing: AtomicBool,
    muted: AtomicBool,
    closed: AtomicBool,
}

/// One replaceable request/event and one native owner. Pausing invalidates callback
/// output immediately; the owner closes the old stream before staged readiness.
pub struct AudioService {
    mailbox: Arc<Mutex<Mailbox>>,
    control: Arc<Control>,
    task: Mutex<Option<JoinHandle<()>>>,
}
impl AudioService {
    pub fn new(origin: Instant) -> Self {
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        let control = Arc::new(Control {
            wanted: AtomicU64::new(0),
            committed: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        });
        let m = mailbox.clone();
        let c = control.clone();
        let task = thread::spawn(move || owner(m, c, origin));
        Self {
            mailbox,
            control,
            task: Mutex::new(Some(task)),
        }
    }
    pub fn stage(
        &self,
        ready: Arc<ReadyPreview>,
        epoch: u64,
        position: usize,
        device: OutputDevice,
    ) {
        self.control.playing.store(false, Ordering::Release);
        self.control.wanted.store(epoch, Ordering::Release);
        let mut mailbox = self.mailbox.lock();
        mailbox.event = None;
        mailbox.request = Some(Request {
            ready,
            epoch,
            position,
            device,
        });
    }
    pub fn commit(&self, epoch: u64, playing: bool) {
        if self.control.wanted.load(Ordering::Acquire) == epoch {
            self.control.committed.store(epoch, Ordering::Release);
            self.control.playing.store(playing, Ordering::Release);
        }
    }
    pub fn mute(&self, muted: bool) {
        self.control.muted.store(muted, Ordering::Release);
    }
    pub fn stop(&self, epoch: u64) {
        self.control.playing.store(false, Ordering::Release);
        self.control.wanted.store(epoch, Ordering::Release);
        let mut mailbox = self.mailbox.lock();
        mailbox.request = None;
        mailbox.event = None;
    }
    pub fn event(&self) -> Option<AudioEvent> {
        self.mailbox.lock().event.take()
    }
    pub fn devices(&self) -> Vec<String> {
        self.mailbox.lock().devices.clone()
    }
    pub fn shutdown(&self) {
        self.control.playing.store(false, Ordering::Release);
        self.control.closed.store(true, Ordering::Release);
    }
    /// Off-UI qualification/close barrier. Ordinary UI drop never waits for CPAL.
    pub fn join(&self) {
        self.shutdown();
        if let Some(task) = self.task.lock().take() {
            let _ = task.join();
        }
    }
}
impl Drop for AudioService {
    fn drop(&mut self) {
        self.shutdown();
        // The owner retains its resources until it has stopped and reaped the feeder.
    }
}

#[derive(Clone)]
pub struct OutputHandle {
    pub epoch: u64,
    pub identity: PreviewIdentity,
    pub sample_rate: u32,
    pub channels: u16,
    pub device: String,
    pub ring_capacity: usize,
    state: Arc<OutputState>,
}
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct AudioMetrics {
    pub submitted_frames: u64,
    pub underrun_frames: u64,
    pub stale_frames: u64,
    pub invalid_timestamps: u64,
    pub ring_high_water: u64,
    pub predicted_latency_ms: f64,
}
impl OutputHandle {
    pub fn snapshot(&self) -> Option<OutputClockSnapshot> {
        self.state.snapshot.read(self.epoch, self.sample_rate)
    }
    pub fn metrics(&self) -> AudioMetrics {
        AudioMetrics {
            submitted_frames: self.state.cursor.load(Ordering::Acquire),
            underrun_frames: self.state.underruns.load(Ordering::Relaxed),
            stale_frames: self.state.stale.load(Ordering::Relaxed),
            invalid_timestamps: self.state.invalid_timestamps.load(Ordering::Relaxed),
            ring_high_water: self.state.ring.high_water.load(Ordering::Relaxed),
            predicted_latency_ms: f64::from_bits(self.state.latency.load(Ordering::Relaxed))
                * 1000.,
        }
    }
}

// All entries are atomic: no unsafe memory or allocation on either SPSC path.
// Release/acquire indexes transfer slot ownership; only producer writes tail and
// only callback writes head. Metadata costs 8 bytes/frame in addition to PCM.
struct Packet {
    sample: AtomicU64,
    position: AtomicU64,
}
struct Ring {
    slots: Box<[Packet]>,
    head: AtomicU64,
    tail: AtomicU64,
    high_water: AtomicU64,
}
impl Ring {
    fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity)
                .map(|_| Packet {
                    sample: AtomicU64::new(0),
                    position: AtomicU64::new(0),
                })
                .collect(),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            high_water: AtomicU64::new(0),
        }
    }
    fn push(&self, position: u64, sample: [f32; 2]) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let length = tail - self.head.load(Ordering::Acquire);
        if length >= self.slots.len() as u64 {
            return false;
        }
        let slot = &self.slots[tail as usize % self.slots.len()];
        slot.sample.store(
            sample[0].to_bits() as u64 | ((sample[1].to_bits() as u64) << 32),
            Ordering::Relaxed,
        );
        slot.position.store(position, Ordering::Relaxed);
        self.tail.store(tail + 1, Ordering::Release);
        self.high_water.fetch_max(length + 1, Ordering::Relaxed);
        true
    }
    fn sample(&self, position: u64, stale: &mut u64) -> Option<[f32; 2]> {
        // At most capacity stale samples are discarded in any callback, never an
        // unbounded producer race. Future samples stay queued during underruns.
        for _ in 0..self.slots.len() {
            let head = self.head.load(Ordering::Relaxed);
            if head == self.tail.load(Ordering::Acquire) {
                return None;
            }
            let slot = &self.slots[head as usize % self.slots.len()];
            let at = slot.position.load(Ordering::Relaxed);
            if at > position {
                return None;
            }
            let bits = slot.sample.load(Ordering::Relaxed);
            self.head.store(head + 1, Ordering::Release);
            if at == position {
                return Some([
                    f32::from_bits(bits as u32),
                    f32::from_bits((bits >> 32) as u32),
                ]);
            }
            *stale += 1;
        }
        None
    }
}

#[derive(Default)]
struct AtomicSnapshot {
    sequence: AtomicU64,
    start: AtomicU64,
    count: AtomicU64,
    callback: AtomicU64,
    playback: AtomicU64,
}
impl AtomicSnapshot {
    fn publish(&self, start: u64, count: u64, callback: f64, playback: f64) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.start.store(start, Ordering::SeqCst);
        self.count.store(count, Ordering::SeqCst);
        self.callback.store(callback.to_bits(), Ordering::SeqCst);
        self.playback.store(playback.to_bits(), Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }
    fn read(&self, epoch: u64, sample_rate: u32) -> Option<OutputClockSnapshot> {
        for _ in 0..3 {
            let before = self.sequence.load(Ordering::SeqCst);
            if before == 0 || !before.is_multiple_of(2) {
                continue;
            }
            let snapshot = OutputClockSnapshot {
                epoch,
                sample_rate,
                start_sample: self.start.load(Ordering::SeqCst),
                sample_count: self.count.load(Ordering::SeqCst),
                callback_seconds: f64::from_bits(self.callback.load(Ordering::SeqCst)),
                playback_seconds: f64::from_bits(self.playback.load(Ordering::SeqCst)),
            };
            if self.sequence.load(Ordering::SeqCst) == before {
                return Some(snapshot);
            }
        }
        None
    }
}

struct OutputState {
    ring: Ring,
    snapshot: AtomicSnapshot,
    cursor: AtomicU64,
    underruns: AtomicU64,
    stale: AtomicU64,
    invalid_timestamps: AtomicU64,
    latency: AtomicU64,
    failed: AtomicBool,
    closed: AtomicBool,
}
impl OutputState {
    fn new(capacity: usize, estimated_latency: f64) -> Self {
        Self {
            ring: Ring::new(capacity),
            snapshot: AtomicSnapshot::default(),
            cursor: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            stale: AtomicU64::new(0),
            invalid_timestamps: AtomicU64::new(0),
            latency: AtomicU64::new(estimated_latency.to_bits()),
            failed: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        }
    }
}

/// One 64KiB positioned read window. The 32-tap windowed-sinc converter runs
/// only on the feeder, preserves source timing and never resets the core mixer.
struct PcmReader {
    file: Arc<studio_engine::PreparedAudioSource>,
    bytes: Box<[u8]>,
    start: u64,
    length: usize,
}
impl PcmReader {
    fn new(file: Arc<studio_engine::PreparedAudioSource>) -> Self {
        Self {
            file,
            bytes: vec![0; READER_BYTES].into_boxed_slice(),
            start: 0,
            length: 0,
        }
    }
    fn sample(&mut self, index: i64) -> io::Result<[f32; 2]> {
        if index < 0 || index as u64 >= self.file.descriptor.sample_count {
            return Ok([0.; 2]);
        }
        let index = index as u64;
        if index < self.start || index >= self.start + self.length as u64 / 8 {
            self.start = index.saturating_sub(32);
            self.length = ((self.file.descriptor.sample_count - self.start) * 8)
                .min(READER_BYTES as u64) as usize;
            read_at(
                &self.file.file,
                &mut self.bytes[..self.length],
                self.start * 8,
            )?;
        }
        let offset = (index - self.start) as usize * 8;
        let result = [
            f32::from_le_bytes(self.bytes[offset..offset + 4].try_into().unwrap()),
            f32::from_le_bytes(self.bytes[offset + 4..offset + 8].try_into().unwrap()),
        ];
        if result.iter().any(|v| !v.is_finite()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PCM sample changed/nonfinite",
            ));
        }
        Ok(result)
    }
    fn resample(&mut self, at: f64, ratio: f64) -> io::Result<[f32; 2]> {
        if ratio == 1. && at.fract() == 0. {
            return self.sample(at as i64);
        }
        let cutoff = (1. / ratio).min(1.);
        let center = at.floor() as i64;
        let mut total = [0.; 2];
        let mut weights = 0.;
        for index in center - 15..=center + 16 {
            let distance = index as f64 - at;
            let scaled = distance * cutoff;
            let sinc = if scaled.abs() < 1e-12 {
                1.
            } else {
                (scaled * std::f64::consts::PI).sin() / (scaled * std::f64::consts::PI)
            };
            let window = 0.5 + 0.5 * (distance * std::f64::consts::PI / 16.).cos();
            let weight = sinc * window * cutoff;
            let sample = self.sample(index)?;
            total[0] += sample[0] as f64 * weight;
            total[1] += sample[1] as f64 * weight;
            weights += weight;
        }
        Ok([(total[0] / weights) as f32, (total[1] / weights) as f32])
    }
}

fn read_at(file: &File, bytes: &mut [u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(bytes, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut remaining = bytes;
        let mut at = offset;
        while !remaining.is_empty() {
            let n = file.seek_read(remaining, at)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            at += n as u64;
            remaining = &mut remaining[n..];
        }
        Ok(())
    }
}

struct Session {
    stream: Option<cpal::Stream>,
    handle: OutputHandle,
    feeder: Option<JoinHandle<()>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.handle.state.closed.store(true, Ordering::Release);
        // The native owner stops the callback before retiring its ring/source.
        drop(self.stream.take());
        if let Some(feeder) = self.feeder.take() {
            let _ = feeder.join();
        }
    }
}

fn owner(mailbox: Arc<Mutex<Mailbox>>, control: Arc<Control>, origin: Instant) {
    let host = cpal::default_host();
    mailbox.lock().devices = host
        .output_devices()
        .into_iter()
        .flatten()
        .filter_map(|d| d.description().ok().map(|d| d.name().to_owned()))
        .take(32)
        .collect();
    let mut current: Option<Session> = None;
    while !control.closed.load(Ordering::Acquire) {
        let request = mailbox.lock().request.take();
        if let Some(request) = request {
            let epoch = request.epoch;
            let identity = request.ready.identity().clone();
            let prepared = prepare_session(&host, &request, &control, origin);
            let mut guard = mailbox.lock();
            if control.closed.load(Ordering::Acquire)
                || control.wanted.load(Ordering::Acquire) != epoch
            {
                drop(guard);
                drop(prepared);
                continue;
            }
            match prepared {
                Ok(session) => {
                    drop(guard);
                    drop(current.take());
                    let output = session.as_ref().map(|s| s.handle.clone());
                    current = session;
                    let mut mailbox = mailbox.lock();
                    if control.wanted.load(Ordering::Acquire) != epoch
                        || control.closed.load(Ordering::Acquire)
                    {
                        continue;
                    }
                    mailbox.event = Some(AudioEvent::Staged {
                        epoch,
                        identity,
                        output,
                        unavailable: None,
                    });
                }
                Err(PreparationError::Unavailable(error)) => {
                    guard.event = Some(AudioEvent::Staged {
                        epoch,
                        identity,
                        output: None,
                        unavailable: Some(error),
                    });
                }
                Err(PreparationError::Source(error)) => {
                    guard.event = Some(AudioEvent::Failed { epoch, error });
                }
            }
        }
        if current
            .as_ref()
            .is_some_and(|s| s.handle.state.failed.load(Ordering::Acquire))
        {
            let epoch = current.as_ref().unwrap().handle.epoch;
            drop(current.take());
            let mut mailbox = mailbox.lock();
            if control.wanted.load(Ordering::Acquire) == epoch
                && !control.closed.load(Ordering::Acquire)
            {
                mailbox.event = Some(AudioEvent::Lost { epoch });
            }
        }
        if current
            .as_ref()
            .is_some_and(|s| s.handle.epoch != control.wanted.load(Ordering::Acquire))
            && mailbox.lock().request.is_none()
            && control.committed.load(Ordering::Acquire) != control.wanted.load(Ordering::Acquire)
        {
            drop(current.take());
        }
        thread::sleep(Duration::from_millis(2));
    }
    drop(current);
    let mut mailbox = mailbox.lock();
    mailbox.request = None;
    mailbox.event = None;
}

enum PreparationError {
    Unavailable(String),
    Source(String),
}
impl From<String> for PreparationError {
    fn from(error: String) -> Self {
        Self::Unavailable(error)
    }
}
impl From<&str> for PreparationError {
    fn from(error: &str) -> Self {
        Self::Unavailable(error.into())
    }
}

fn prepare_session(
    host: &cpal::Host,
    request: &Request,
    control: &Arc<Control>,
    origin: Instant,
) -> Result<Option<Session>, PreparationError> {
    if request.device == OutputDevice::Unavailable {
        return Ok(None);
    }
    let device = match &request.device {
        OutputDevice::Default => host.default_output_device(),
        OutputDevice::Named(name) => host
            .output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.description().is_ok_and(|d| d.name() == name)),
        OutputDevice::Unavailable => None,
    };
    let Some(device) = device else {
        return Err("Audio output unavailable; monotonic fallback".into());
    };
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = supported.sample_rate();
    let channels = supported.channels();
    if !(8000..=MAX_OUTPUT_RATE).contains(&rate) || channels == 0 || channels > 32 {
        return Err("Unsupported device rate/channels; choose another output".into());
    }
    let source = request
        .ready
        .audio_source
        .clone()
        .ok_or_else(|| PreparationError::Source("Matching retained PCM file missing".into()))?;
    let total = ((request
        .ready
        .timeline
        .total_frames
        .saturating_sub(request.position) as u128
        * rate as u128)
        .div_ceil(request.ready.timeline.fps as u128)) as u64;
    let capacity = (rate as usize / 4).max(1);
    let mut config = supported.config();
    let period = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => (rate / 100).clamp(*min, *max),
        cpal::SupportedBufferSize::Unknown => rate / 100,
    };
    if period > rate / 4 {
        return Err("Device buffer exceeds 250ms; choose another output".into());
    }
    config.buffer_size = cpal::BufferSize::Fixed(period);
    let state = Arc::new(OutputState::new(capacity, period as f64 / rate as f64));
    let mut reader = PcmReader::new(source);
    let ratio = request.ready.audio.sample_rate as f64 / rate as f64;
    let start = request.position as f64 * request.ready.audio.sample_rate as f64
        / request.ready.timeline.fps as f64;
    let mut feed = 0;
    while feed < total.min(capacity as u64) {
        if control.closed.load(Ordering::Acquire)
            || control.wanted.load(Ordering::Acquire) != request.epoch
        {
            return Err("Audio preparation superseded".into());
        }
        let sample = reader
            .resample(start + feed as f64 * ratio, ratio)
            .map_err(|e| PreparationError::Source(e.to_string()))?;
        state.ring.push(feed, sample);
        feed += 1;
    }
    let handle = OutputHandle {
        epoch: request.epoch,
        identity: request.ready.identity().clone(),
        sample_rate: rate,
        channels,
        device: device
            .description()
            .map_err(|e| e.to_string())?
            .name()
            .to_owned(),
        ring_capacity: capacity,
        state: state.clone(),
    };
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            build_stream::<f32>(&device, config, &handle, control, origin, total)
        }
        cpal::SampleFormat::F64 => {
            build_stream::<f64>(&device, config, &handle, control, origin, total)
        }
        cpal::SampleFormat::I16 => {
            build_stream::<i16>(&device, config, &handle, control, origin, total)
        }
        cpal::SampleFormat::U16 => {
            build_stream::<u16>(&device, config, &handle, control, origin, total)
        }
        cpal::SampleFormat::I32 => {
            build_stream::<i32>(&device, config, &handle, control, origin, total)
        }
        cpal::SampleFormat::U32 => {
            build_stream::<u32>(&device, config, &handle, control, origin, total)
        }
        cpal::SampleFormat::I8 => {
            build_stream::<i8>(&device, config, &handle, control, origin, total)
        }
        cpal::SampleFormat::U8 => {
            build_stream::<u8>(&device, config, &handle, control, origin, total)
        }
        cpal::SampleFormat::I64 => {
            build_stream::<i64>(&device, config, &handle, control, origin, total)
        }
        cpal::SampleFormat::U64 => {
            build_stream::<u64>(&device, config, &handle, control, origin, total)
        }
        format => return Err(format!("Unsupported output format {format}").into()),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    let s = state.clone();
    let c = control.clone();
    let epoch = request.epoch;
    let feeder = thread::spawn(move || {
        while !s.closed.load(Ordering::Acquire)
            && !c.closed.load(Ordering::Acquire)
            && c.wanted.load(Ordering::Acquire) == epoch
        {
            feed = feed.max(s.cursor.load(Ordering::Acquire));
            if feed >= total {
                break;
            }
            if s.ring.tail.load(Ordering::Relaxed) - s.ring.head.load(Ordering::Acquire)
                >= capacity as u64
            {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            match reader.resample(start + feed as f64 * ratio, ratio) {
                Ok(sample) => {
                    if s.ring.push(feed, sample) {
                        feed += 1;
                    }
                }
                Err(_) => {
                    s.failed.store(true, Ordering::Release);
                    break;
                }
            }
        }
    });
    Ok(Some(Session {
        stream: Some(stream),
        handle,
        feeder: Some(feeder),
    }))
}

fn build_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    handle: &OutputHandle,
    control: &Arc<Control>,
    origin: Instant,
    total: u64,
) -> Result<cpal::Stream, String> {
    let state = handle.state.clone();
    let error_state = state.clone();
    let control = control.clone();
    let epoch = handle.epoch;
    let rate = handle.sample_rate;
    let channels = handle.channels as usize;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let timestamp = info.timestamp();
                let latency = timestamp
                    .playback
                    .checked_duration_since(timestamp.callback)
                    .map(|d| d.as_secs_f64());
                let now = origin.elapsed().as_secs_f64();
                callback(
                    data, channels, &state, &control, epoch, rate, total, now, latency,
                );
            },
            move |_| {
                error_state.failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
fn callback<T: SizedSample + FromSample<f32>>(
    data: &mut [T],
    channels: usize,
    state: &OutputState,
    control: &Control,
    epoch: u64,
    rate: u32,
    total: u64,
    now: f64,
    latency: Option<f64>,
) {
    if control.closed.load(Ordering::Acquire)
        || !control.playing.load(Ordering::Acquire)
        || control.wanted.load(Ordering::Acquire) != epoch
        || control.committed.load(Ordering::Acquire) != epoch
    {
        data.fill(T::from_sample(0.));
        return;
    }
    let start = state.cursor.load(Ordering::Relaxed);
    let mut cursor = start;
    let mut underruns = 0;
    let mut stale = 0;
    let muted = control.muted.load(Ordering::Relaxed);
    for frame in data.chunks_mut(channels) {
        let mut sample = [0.; 2];
        if cursor < total {
            sample = state.ring.sample(cursor, &mut stale).unwrap_or_else(|| {
                underruns += 1;
                [0.; 2]
            });
            cursor += 1;
        }
        if muted {
            sample = [0.; 2];
        }
        match frame {
            [mono] => *mono = T::from_sample(f32::midpoint(sample[0], sample[1]).clamp(-1., 1.)),
            [left, right, rest @ ..] => {
                *left = T::from_sample(sample[0].clamp(-1., 1.));
                *right = T::from_sample(sample[1].clamp(-1., 1.));
                rest.fill(T::from_sample(0.));
            }
            [] => {}
        }
    }
    state.cursor.store(cursor, Ordering::Release);
    state.underruns.fetch_add(underruns, Ordering::Relaxed);
    state.stale.fetch_add(stale, Ordering::Relaxed);
    let latency = match latency.filter(|v| v.is_finite() && (0. ..=2.).contains(v)) {
        Some(latency) => {
            state.latency.store(latency.to_bits(), Ordering::Relaxed);
            latency
        }
        None => {
            state.invalid_timestamps.fetch_add(1, Ordering::Relaxed);
            f64::from_bits(state.latency.load(Ordering::Relaxed))
        }
    };
    // Keep the final submitted anchor. Publishing fresh zero-count anchors after
    // EOF would forever leave the audible cursor one device latency short of end.
    if cursor > start {
        state
            .snapshot
            .publish(start, cursor - start, now, now + latency);
    }
    let _ = rate; // sample-rate conversion belongs solely to the feeder/clock model.
}

#[cfg(test)]
#[path = "audio_service/tests.rs"]
mod tests;
