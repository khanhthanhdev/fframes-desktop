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

/// Ids of the bundled presets, in registry order.
pub const IDS: [&str; 3] = ["editorial", "pulse", "quiet-motion"];

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
            [EDITORIAL, PULSE, QUIET_MOTION]
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
