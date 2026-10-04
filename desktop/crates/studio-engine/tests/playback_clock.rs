use studio_engine::{OutputClockSnapshot, PlaybackClock, PlaybackClockError};

fn output(
    clock: &PlaybackClock,
    start: u64,
    count: u64,
    callback: f64,
    playback: f64,
) -> OutputClockSnapshot {
    OutputClockSnapshot {
        epoch: clock.epoch(),
        start_sample: start,
        sample_count: count,
        sample_rate: 48_000,
        callback_seconds: callback,
        playback_seconds: playback,
    }
}

#[test]
fn output_anchor_uses_predicted_playback_not_callback_or_producer_cursor() {
    let mut clock = PlaybackClock::install(30, 300, 0, 10.0).unwrap();
    clock.toggle(10.0).unwrap();
    assert!(clock.accept_output(output(&clock, 0, 9_600, 10.0, 10.075)));
    assert_eq!(clock.tick(10.075), 0);
    assert_eq!(clock.tick(10.175), 3);
    assert_eq!(clock.tick(10.400), 6); // submitted samples, not host extrapolation
}

#[test]
fn future_anchor_can_map_into_samples_submitted_before_latest_callback() {
    let mut clock = PlaybackClock::install(30, 300, 0, 0.0).unwrap();
    clock.toggle(0.0).unwrap();
    assert!(clock.accept_output(output(&clock, 0, 4_800, 1.0, 1.2)));
    assert!(clock.accept_output(output(&clock, 4_800, 4_800, 1.1, 1.3)));
    assert_eq!(clock.tick(1.25), 1); // 2,400 samples, not callback start_sample 4,800
    assert_eq!(clock.tick(1.5), 6);
}

#[test]
fn rejects_stale_backwards_invalid_and_rate_change_snapshots() {
    let mut clock = PlaybackClock::install(24, 100, 0, 0.0).unwrap();
    clock.toggle(0.0).unwrap();
    let first = output(&clock, 0, 480, 1.0, 1.01);
    assert!(clock.accept_output(first));
    let mut bad = output(&clock, 480, 480, 0.9, 1.02);
    assert!(!clock.accept_output(bad));
    bad.callback_seconds = 1.1;
    bad.playback_seconds = f64::NAN;
    assert!(!clock.accept_output(bad));
    bad.playback_seconds = 1.09;
    assert!(!clock.accept_output(bad));
    bad.playback_seconds = 1.2;
    bad.sample_rate = 44_100;
    assert!(!clock.accept_output(bad));
    clock.seek(4, 2.0).unwrap();
    assert!(!clock.accept_output(first));
}

#[test]
fn zero_submitted_interval_freezes_and_noninteger_conversion_floors() {
    let mut clock = PlaybackClock::install(29, 100, 7, 0.0).unwrap();
    clock.toggle(0.0).unwrap();
    assert!(clock.accept_output(output(&clock, 0, 0, 0.0, 0.05)));
    assert_eq!(clock.tick(20.0), 7);
    assert!(clock.accept_output(output(&clock, 0, 50_000, 20.0, 20.1)));
    assert_eq!(clock.tick(21.1), 36); // floor(48,000 * 29 / 48,000)
}

#[test]
fn pause_seek_end_and_replay_have_inclusive_end_cursor() {
    let mut clock = PlaybackClock::install(10, 3, 0, 0.0).unwrap();
    clock.toggle(0.0).unwrap();
    clock.fallback(0.0).unwrap();
    assert_eq!(clock.tick(0.2), 2); // frame 2 is final renderable frame
    assert_eq!(clock.tick(0.3), 3); // cursor end
    assert!(!clock.playing());
    let end_epoch = clock.epoch();
    clock.toggle(1.0).unwrap();
    assert!(clock.playing());
    assert_eq!(clock.position(), 0);
    assert!(clock.epoch() > end_epoch);
    clock.fallback(1.0).unwrap();
    clock.pause(1.1).unwrap();
    let paused = clock.position();
    assert_eq!(clock.tick(9.0), paused);
    clock.seek(99, 9.0).unwrap();
    assert_eq!(clock.position(), 3);
}

#[test]
fn empty_and_shorter_install_clamp_without_playing() {
    let mut empty = PlaybackClock::install(30, 0, 9, 0.0).unwrap();
    empty.toggle(0.0).unwrap();
    assert_eq!(empty.position(), 0);
    assert!(!empty.playing());
    let shorter = PlaybackClock::install(30, 4, 99, 0.0).unwrap();
    assert_eq!(shorter.position(), 4);
    assert_eq!(shorter.seconds(), 4.0 / 30.0);
    assert_eq!(
        PlaybackClock::install(0, 1, 0, 0.0).unwrap_err(),
        PlaybackClockError::InvalidFps
    );
}

#[test]
fn fallback_and_reconnect_waiting_preserve_intent_without_jump() {
    let mut clock = PlaybackClock::install(30, 300, 0, 0.0).unwrap();
    clock.toggle(0.0).unwrap();
    assert!(clock.accept_output(output(&clock, 0, 48_000, 0.0, 0.1)));
    clock.fallback(0.6).unwrap();
    assert_eq!(clock.position(), 15);
    assert_eq!(clock.tick(0.8), 21);
    clock.wait_for_output().unwrap();
    assert!(clock.playing());
    assert_eq!(clock.tick(5.0), 21);
    assert!(clock.accept_output(output(&clock, 0, 48_000, 5.0, 5.1)));
    assert_eq!(clock.tick(5.2), 24);
}

#[test]
fn revision_reservation_prevents_seek_from_reusing_staged_audio_epoch() {
    let mut active = PlaybackClock::install(30, 165, 91, 0.).unwrap();
    active.toggle(0.).unwrap();
    let old = output(&active, 0, 480, 0., 0.01);
    active.pause(0.).unwrap();
    let mut candidate = active.clone();
    candidate.reinstall(24, 60, 91, 1.).unwrap();
    active.wait_for_output().unwrap();
    assert_eq!(active.epoch(), candidate.epoch());
    assert_eq!(candidate.position(), 60);
    let staged = output(&candidate, 0, 480, 1., 1.01);
    active.seek(37, 1.).unwrap();
    assert!(active.epoch() > candidate.epoch());
    assert!(!active.accept_output(old));
    assert!(!active.accept_output(staged));
    assert_eq!(active.tick(100.), 37);
    let epoch = candidate.epoch();
    candidate.reinstall(30, 0, 60, 2.).unwrap();
    assert!(candidate.epoch() > epoch);
    assert_eq!(candidate.position(), 0);
    assert!(!candidate.playing());
}
