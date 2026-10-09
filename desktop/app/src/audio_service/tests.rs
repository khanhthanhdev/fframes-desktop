use super::*;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    fs,
    sync::{Arc, Barrier},
};

use fframes_studio_protocol::{
    PREVIEW_CONTRACT_VERSION, PreparedAudioDescriptor, PreviewEnvelope, PreviewIdentity,
};
use studio_engine::build_materialization::{materialize, sdk_pin};
use studio_sdk::CompatibilityManifest;

struct ThreadCountingAllocator;
thread_local! {
    static COUNT_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
unsafe impl GlobalAlloc for ThreadCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = COUNT_ALLOCATIONS.try_with(|enabled| {
            if enabled.get() {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let _ = COUNT_ALLOCATIONS.try_with(|enabled| {
            if enabled.get() {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static TEST_ALLOCATOR: ThreadCountingAllocator = ThreadCountingAllocator;

fn control(epoch: u64) -> Control {
    Control {
        wanted: AtomicU64::new(epoch),
        committed: AtomicU64::new(epoch),
        playing: AtomicBool::new(true),
        muted: AtomicBool::new(false),
        closed: AtomicBool::new(false),
    }
}

fn invoke<T: SizedSample + FromSample<f32>>(
    output: &mut [T],
    channels: usize,
    state: &OutputState,
    control: &Control,
    epoch: u64,
    total: u64,
    latency: Option<f64>,
) {
    callback(
        output, channels, state, control, epoch, 48_000, total, 12.0, latency,
    );
}

#[test]
fn callback_maps_asymmetric_channels_and_unsigned_silence_without_allocating() {
    let state = OutputState::new(8, 0.01);
    assert!(state.ring.push(0, [0.8, -0.2]));
    assert!(state.ring.push(1, [-1.0, 0.5]));
    let control = control(7);
    let mut mono = [0u16; 1];
    let mut surround = [0u16; 4];
    ALLOCATIONS.with(|count| count.set(0));
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(true));
    invoke(&mut mono, 1, &state, &control, 7, 2, Some(0.02));
    invoke(&mut surround, 4, &state, &control, 7, 2, Some(0.02));
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0, "audio callback allocated");
    assert!((mono[0] as i32 - 42598).abs() <= 1);
    assert_eq!(surround[0], 0);
    assert_eq!(surround[1], 49152);
    assert_eq!(&surround[2..], &[32768; 2]);

    control.playing.store(false, Ordering::Release);
    let mut silence = [u16::MAX; 3];
    invoke(&mut silence, 3, &state, &control, 7, 3, None);
    assert_eq!(silence, [32768; 3]);
}

#[test]
fn callback_gates_do_not_consume_but_mute_underrun_and_stale_do() {
    for gate in 0..3 {
        let state = OutputState::new(8, 0.01);
        state.ring.push(0, [0.25, -0.25]);
        let control = control(9);
        match gate {
            0 => control.playing.store(false, Ordering::Release),
            1 => control.committed.store(8, Ordering::Release),
            _ => control.wanted.store(10, Ordering::Release),
        }
        let mut out = [1.0f32; 2];
        invoke(&mut out, 2, &state, &control, 9, 1, None);
        assert_eq!(out, [0.0; 2]);
        assert_eq!(state.cursor.load(Ordering::Acquire), 0);
        assert_eq!(state.ring.head.load(Ordering::Acquire), 0);
    }

    let state = OutputState::new(8, 0.01);
    state.ring.push(0, [0.7, 0.6]);
    let muted_control = control(4);
    muted_control.muted.store(true, Ordering::Release);
    let mut out = [1.0f32; 2];
    invoke(&mut out, 2, &state, &muted_control, 4, 1, Some(0.01));
    assert_eq!(out, [0.0; 2]);
    assert_eq!(state.cursor.load(Ordering::Acquire), 1);

    let state = OutputState::new(8, 0.01);
    state.ring.push(0, [0.9, 0.9]);
    state.cursor.store(1, Ordering::Release);
    let mut out = [1.0f32; 4];
    invoke(&mut out, 2, &state, &control(4), 4, 3, Some(0.01));
    assert_eq!(out, [0.0; 4]);
    assert_eq!(state.cursor.load(Ordering::Acquire), 3);
    assert_eq!(state.stale.load(Ordering::Relaxed), 1);
    assert_eq!(state.underruns.load(Ordering::Relaxed), 2);
    assert_eq!(
        state.ring.head.load(Ordering::Acquire),
        1,
        "stale PCM replayable"
    );
}

#[test]
fn eof_keeps_final_anchor_and_invalid_latency_is_visible_and_estimated() {
    let state = OutputState::new(4, 0.025);
    state.ring.push(0, [0.1, 0.2]);
    let control = control(2);
    let mut out = [0.0f32; 2];
    invoke(&mut out, 2, &state, &control, 2, 1, Some(f64::NAN));
    let final_anchor = state.snapshot.read(2, 48_000).unwrap();
    assert_eq!(final_anchor.start_sample, 0);
    assert_eq!(final_anchor.sample_count, 1);
    assert_eq!(final_anchor.playback_seconds, 12.025);
    assert_eq!(state.invalid_timestamps.load(Ordering::Relaxed), 1);
    invoke(&mut out, 2, &state, &control, 2, 1, Some(2.001));
    assert_eq!(state.snapshot.read(2, 48_000).unwrap(), final_anchor);
    assert_eq!(state.invalid_timestamps.load(Ordering::Relaxed), 2);
    assert_eq!(f64::from_bits(state.latency.load(Ordering::Relaxed)), 0.025);
}

#[test]
fn ring_wrap_is_exact_and_capacity_accounts_for_pcm_plus_packet_metadata() {
    assert_eq!(std::mem::size_of::<Packet>(), 16);
    let ring = Ring::new(3);
    let mut stale = 0;
    for base in [0, 3, 6] {
        for i in 0..3 {
            let p = base + i;
            assert!(ring.push(p, [p as f32 + 0.25, -(p as f32)]));
        }
        assert!(!ring.push(99, [0.0; 2]));
        for i in 0..3 {
            let p = base + i;
            assert_eq!(
                ring.sample(p, &mut stale),
                Some([p as f32 + 0.25, -(p as f32)])
            );
        }
    }
    assert_eq!(stale, 0);
    let rate = MAX_OUTPUT_RATE as usize;
    let capacity = rate / 4;
    assert_eq!(capacity, 250 * rate / 1000);
    assert_eq!(capacity * std::mem::size_of::<Packet>(), capacity * (8 + 8));
}

#[test]
fn ring_spsc_transfers_one_hundred_thousand_indexed_frames() {
    const N: u64 = 100_000;
    let ring = Arc::new(Ring::new(257));
    let barrier = Arc::new(Barrier::new(2));
    let producer = {
        let ring = ring.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            for i in 0..N {
                while !ring.push(i, [i as f32, -(i as f32)]) {
                    std::hint::spin_loop();
                }
            }
        })
    };
    barrier.wait();
    let mut stale = 0;
    for i in 0..N {
        let got = loop {
            if let Some(sample) = ring.sample(i, &mut stale) {
                break sample;
            }
            std::hint::spin_loop();
        };
        assert_eq!(got, [i as f32, -(i as f32)]);
    }
    producer.join().unwrap();
    assert_eq!(stale, 0);
}

#[test]
fn atomic_snapshot_is_coherent_with_concurrent_publisher_and_readers() {
    const N: u64 = 40_000;
    let snapshot = Arc::new(AtomicSnapshot::default());
    let done = Arc::new(AtomicBool::new(false));
    let publisher = {
        let snapshot = snapshot.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            for n in 1..=N {
                snapshot.publish(n, n * 3 + 1, n as f64 + 0.25, n as f64 * 2.0 + 0.5);
            }
            done.store(true, Ordering::Release);
        })
    };
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let snapshot = snapshot.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                let mut reads = 0;
                while !done.load(Ordering::Acquire) || reads < 100 {
                    if let Some(s) = snapshot.read(77, 44_100) {
                        assert_eq!(s.sample_count, s.start_sample * 3 + 1);
                        assert_eq!(s.callback_seconds, s.start_sample as f64 + 0.25);
                        assert_eq!(s.playback_seconds, s.start_sample as f64 * 2.0 + 0.5);
                        reads += 1;
                    }
                }
            })
        })
        .collect();
    publisher.join().unwrap();
    for reader in readers {
        reader.join().unwrap();
    }
}

fn source(samples: &[[f32; 2]], rate: u32) -> Arc<studio_engine::PreparedAudioSource> {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let sdk = temp.path().join("sdk");
    let compatibility = CompatibilityManifest::default_linux_x64();
    for name in [
        "fframes",
        "fframes-studio-runtime",
        "fframes-studio-protocol",
    ] {
        let dir = sdk.join("framework/framework").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
    }
    fs::create_dir_all(sdk.join("framework/vendor")).unwrap();
    let project = studio_project::create(
        &root,
        "AudioTest",
        sdk_pin(&compatibility),
        &compatibility.fframes_version,
        "0.1.0",
    )
    .unwrap();
    let lease =
        Arc::new(materialize(&project, &sdk, compatibility, &temp.path().join("builds")).unwrap());
    let path = lease.root.join("test.pcm");
    let mut bytes = Vec::with_capacity(samples.len() * 8);
    for sample in samples {
        bytes.extend_from_slice(&sample[0].to_le_bytes());
        bytes.extend_from_slice(&sample[1].to_le_bytes());
    }
    fs::write(&path, &bytes).unwrap();
    let envelope = PreviewEnvelope {
        contract_version: PREVIEW_CONTRACT_VERSION,
        identity: PreviewIdentity {
            project_id: "p".into(),
            open_session: "s".into(),
            source_revision: "r".into(),
            worker_generation: 1,
        },
        request_id: 1,
    };
    let descriptor = PreparedAudioDescriptor {
        envelope,
        artifact_id: "synthetic".into(),
        sample_rate: rate,
        channels: 2,
        sample_count: samples.len() as u64,
        byte_count: bytes.len() as u64,
        sha256: "0".repeat(64),
        silent: false,
    };
    Arc::new(studio_engine::PreparedAudioSource::new(
        fs::File::open(path).unwrap(),
        descriptor,
        lease,
    ))
}

#[test]
fn positioned_reader_crosses_64k_window_boundaries_and_rejects_nonfinite_pcm() {
    let samples: Vec<_> = (0..20_000).map(|i| [i as f32, -(i as f32) - 0.5]).collect();
    let mut reader = PcmReader::new(source(&samples, 48_000));
    for i in [8191, 8192, 8193, 16_390, 17, 16_391] {
        assert_eq!(reader.sample(i).unwrap(), samples[i as usize]);
        assert!(reader.length <= READER_BYTES);
    }
    assert_eq!(reader.sample(-1).unwrap(), [0.0; 2]);
    assert_eq!(reader.sample(20_000).unwrap(), [0.0; 2]);
    let mut bad = samples;
    bad[9000][1] = f32::NAN;
    assert!(PcmReader::new(source(&bad, 48_000)).sample(9000).is_err());
}

fn tone(rate: u32, hz: f64, seconds: f64, right_scale: f32) -> Vec<[f32; 2]> {
    (0..(rate as f64 * seconds) as usize)
        .map(|i| {
            let x = (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32;
            [x, x * right_scale]
        })
        .collect()
}

#[test]
fn windowed_sinc_preserves_tone_timing_and_channel_identity_at_44k1_and_96k() {
    let input = tone(48_000, 997.0, 0.2, -0.37);
    for output_rate in [44_100, 96_000] {
        let ratio = 48_000.0 / output_rate as f64;
        let mut reader = PcmReader::new(source(&input, 48_000));
        let mut error = 0.0;
        let mut channel_error = 0.0;
        for n in 64..(output_rate / 10) as usize {
            let got = reader.resample(n as f64 * ratio, ratio).unwrap();
            let expected =
                (std::f64::consts::TAU * 997.0 * n as f64 / output_rate as f64).sin() as f32;
            error += (got[0] - expected).abs() as f64;
            channel_error += (got[1] + got[0] * 0.37).abs() as f64;
        }
        assert!(
            error / (output_rate as f64) < 0.0002,
            "timing/amplitude drift at {output_rate}"
        );
        assert!(
            channel_error / (output_rate as f64) < 1e-6,
            "channels crossed at {output_rate}"
        );
    }
}

#[test]
fn downsample_attenuates_analytic_out_of_band_alias_and_eof_does_not_restart() {
    let input = tone(48_000, 18_000.0, 0.2, 0.5);
    let mut reader = PcmReader::new(source(&input, 48_000));
    let output_rate = 24_000.0;
    let ratio = 2.0;
    let mut projected = 0.0;
    let mut energy = 0.0;
    let count = 2_000;
    // 18k aliases to 6k without a low-pass. Compare against that independent analytic tone.
    for n in 64..64 + count {
        let got = reader.resample(n as f64 * ratio, ratio).unwrap()[0] as f64;
        let alias = (std::f64::consts::TAU * 6_000.0 * n as f64 / output_rate).sin();
        projected += got * alias;
        energy += alias * alias;
    }
    assert!(
        (projected / energy).abs() < 0.03,
        "out-of-band tone aliased audibly"
    );
    assert_eq!(
        reader.resample(input.len() as f64 + 64.0, ratio).unwrap(),
        [0.0; 2]
    );
    assert_eq!(
        reader.resample(input.len() as f64 + 128.0, ratio).unwrap(),
        [0.0; 2]
    );
}
