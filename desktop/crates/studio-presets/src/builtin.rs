//! Compile-time registry of the bundled presets. Read-only: bundles are embedded directory trees
//! that are verified exactly like imported packages the first time they are requested.

use std::{collections::BTreeMap, sync::OnceLock};

use studio_project::ProjectPath;

use crate::{model::PresetError, package::Package};

type Bundle = &'static [(&'static str, &'static [u8])];

macro_rules! bundle {
    ($dir:literal: $($file:literal),+ $(,)?) => {
        &[$(($file, include_bytes!(concat!("../builtins/", $dir, "/", $file)) as &[u8])),+]
    };
}

const EDITORIAL: Bundle = bundle!("editorial":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/DMSans-OFL.txt", "fonts/DMSans-Medium.ttf", "examples/title-card.md");
const PULSE: Bundle = bundle!("pulse":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/DMSans-OFL.txt", "fonts/DMSans-Medium.ttf", "examples/title-card.md");
const QUIET_MOTION: Bundle = bundle!("quiet-motion":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/DMSans-OFL.txt", "fonts/DMSans-Regular.ttf", "examples/title-card.md");
const SUNSET: Bundle = bundle!("sunset":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/DMSans-OFL.txt", "fonts/DMSans-Medium.ttf", "examples/title-card.md");
const SWISS: Bundle = bundle!("swiss":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/DMSans-OFL.txt", "fonts/DMSans-Medium.ttf", "examples/title-card.md");
const TERMINAL: Bundle = bundle!("terminal":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/JetBrainsMono-OFL.txt", "fonts/JetBrainsMono-Regular.ttf", "examples/title-card.md");
const LAUNCH: Bundle = bundle!("launch":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/Geist-OFL.txt", "fonts/Geist-SemiBold.ttf", "examples/title-card.md");
const PROMO: Bundle = bundle!("promo":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/Geist-OFL.txt", "fonts/Geist-SemiBold.ttf", "examples/title-card.md");
const LUXE: Bundle = bundle!("luxe":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/DMSans-OFL.txt", "fonts/DMSans-Regular.ttf", "examples/title-card.md");
const FRESH: Bundle = bundle!("fresh":
    "preset.json", "tokens.json", "guide.md", "LICENSE.txt",
    "licenses/Geist-OFL.txt", "fonts/Geist-Medium.ttf", "examples/title-card.md");

/// Ids of the bundled presets, in registry order.
pub const IDS: [&str; 10] = [
    "editorial",
    "pulse",
    "quiet-motion",
    "sunset",
    "swiss",
    "terminal",
    "launch",
    "promo",
    "luxe",
    "fresh",
];

fn load(bundle: Bundle) -> Result<Package, PresetError> {
    let files: BTreeMap<ProjectPath, Vec<u8>> = bundle
        .iter()
        .map(|(path, bytes)| {
            (
                ProjectPath::try_from((*path).to_owned()).expect("bundled path is valid"),
                bytes.to_vec(),
            )
        })
        .collect();
    Package::from_files(files)
}

/// All bundled presets, verified once. Returns the verification error if a bundle is corrupt.
pub fn packages() -> Result<&'static [Package], &'static PresetError> {
    static CACHE: OnceLock<Result<Vec<Package>, PresetError>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            [
                EDITORIAL,
                PULSE,
                QUIET_MOTION,
                SUNSET,
                SWISS,
                TERMINAL,
                LAUNCH,
                PROMO,
                LUXE,
                FRESH,
            ]
            .into_iter()
            .map(load)
            .collect()
        })
        .as_ref()
        .map(Vec::as_slice)
}

/// One bundled preset by id.
pub fn get(id: &str) -> Option<&'static Package> {
    packages().ok()?.iter().find(|p| p.id() == id)
}
