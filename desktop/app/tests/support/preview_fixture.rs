use std::{fs, path::Path};
use studio_sdk::CompatibilityManifest;

pub const SOURCE: &str = include_str!("../../../fixtures/preview-timeline-audio/src/lib.rs");
pub const WAVE_RATE: u32 = 48_000;
pub const WAVE_FRAMES: u32 = 48_000;
#[allow(dead_code)] // Used by Stage 4 parity tests; Stage 3 includes this shared helper independently.
pub const SWATCH_CENTERS: [(u32, u32, [u8; 4]); 4] = [
    (40, 664, [255, 0, 0, 255]),
    (104, 664, [0, 255, 0, 255]),
    (168, 664, [0, 0, 255, 128]),
    (232, 664, [0, 0, 0, 0]),
];

/// Scaffold through the ordinary portable template; every media byte is reproducible.
pub fn create(root: &Path, manifest: &CompatibilityManifest) {
    assert!(
        !root.exists(),
        "fixture cannot overwrite an existing project"
    );
    studio_project::create(
        root,
        "Timeline and audio fixture",
        studio_engine::build_materialization::sdk_pin(manifest),
        &manifest.fframes_version,
        "0.1.0",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), SOURCE).unwrap();
    let count = WAVE_FRAMES;
    let bytes = count * 4;
    let mut wav = Vec::with_capacity(bytes as usize + 44);
    wav.extend(b"RIFF");
    wav.extend((36 + bytes).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16_u32.to_le_bytes());
    wav.extend(1_u16.to_le_bytes());
    wav.extend(2_u16.to_le_bytes());
    wav.extend(WAVE_RATE.to_le_bytes());
    wav.extend(192_000_u32.to_le_bytes());
    wav.extend(4_u16.to_le_bytes());
    wav.extend(16_u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(bytes.to_le_bytes());
    for sample in 0..count {
        let left = ((sample as f64 * 440. * std::f64::consts::TAU / f64::from(WAVE_RATE)).sin()
            * 4000.) as i16;
        let right = ((sample as f64 * 660. * std::f64::consts::TAU / f64::from(WAVE_RATE)).sin()
            * 2000.) as i16;
        wav.extend(left.to_le_bytes());
        wav.extend(right.to_le_bytes());
    }
    fs::write(root.join("media/cue.wav"), wav).unwrap();
}
