//! Project-local materialization: the exact file set a preset application writes.
//!
//! The engine owns writing (transactionally, source-fenced); this module only decides *what* the
//! files are and keeps them deterministic.
//!
//! | Project path | Content |
//! |---|---|
//! | `style/tokens.json` | merged runtime snapshot consumed by `fframes::Styles` |
//! | `style/overrides.json` | canonical project/scene overrides (orphans retained) |
//! | `style/guide.md` | the preset guide |
//! | `style/preset.json` | identity: id, name, version, package hash, snapshot hash, design |
//! | `style/preset-tokens.json` | normalized preset defaults, so overrides can be re-resolved offline |
//! | `media/preset-<package path, `/` -> `-`>` | bundled fonts and images, flat: the renderer's `MediaDirectory::read_folder("media")` only reads top-level files |
//! | `style/preset/<package path>` | license notices and examples |

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use studio_project::ProjectPath;

use crate::{
    model::{Code, Design, PresetError, TOKEN_SCHEMA, check_schema, json_guard},
    package::Package,
    resolve::{OverridesFile, ResolvedSnapshot, TokenSet, resolve},
};

pub const TOKENS_PATH: &str = "style/tokens.json";
pub const OVERRIDES_PATH: &str = "style/overrides.json";
pub const GUIDE_PATH: &str = "style/guide.md";
pub const PRESET_PATH: &str = "style/preset.json";
pub const PRESET_TOKENS_PATH: &str = "style/preset-tokens.json";
/// Every preset-owned media file is a top-level `media/` file with this name prefix.
pub const MEDIA_PREFIX: &str = "media/preset-";
pub const NOTICE_PREFIX: &str = "style/preset/";

/// `style/preset.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetIdentity {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    /// Package hash; also the `sha256` of `studio.json`'s `PresetReference`.
    pub hash: String,
    /// Hash of the merged runtime snapshot (`style/tokens.json`).
    pub tokens_hash: String,
    pub design: Design,
    /// The project-relative media files this preset brought (sorted): exactly what a later
    /// application replaces, so user files are never mistaken for preset media.
    #[serde(default)]
    pub media: Vec<String>,
}

/// The flat, collision-free project path of a preset media file.
pub fn media_path(package_path: &str) -> String {
    format!("{MEDIA_PREFIX}{}", package_path.replace('/', "-"))
}

#[derive(Debug, Clone)]
pub struct MaterializedStyle {
    /// Sorted, deterministic, all contained project-relative paths.
    pub files: Vec<(ProjectPath, Vec<u8>)>,
    /// The merged (defaults + project overrides) snapshot; orphaned / type-mismatched /
    /// invalid overrides are in `snapshot.diagnostics()`.
    pub snapshot: ResolvedSnapshot,
    pub identity: PresetIdentity,
}

fn path(value: String) -> ProjectPath {
    ProjectPath::try_from(value).expect("prefix + validated package path is a valid project path")
}

/// Everything a preset application writes into a project, with the existing overrides preserved.
pub fn materialize(
    package: &Package,
    overrides: &OverridesFile,
) -> Result<MaterializedStyle, PresetError> {
    let snapshot = package.snapshot(Some(overrides.project()), None);
    let manifest = package.manifest();
    let mut media: Vec<(String, &Vec<u8>, String)> = Vec::new();
    for resource in manifest
        .fonts
        .iter()
        .map(|f| &f.file)
        .chain(manifest.assets.iter().map(|a| &a.file))
    {
        let original = resource.path.as_str().to_owned();
        media.push((
            media_path(&original),
            &package.files()[&resource.path],
            original,
        ));
    }
    media.sort();
    if let Some(pair) = media.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(PresetError::one(
            Code::DuplicatePath,
            pair[0].2.clone(),
            format!(
                "`{}` and `{}` both map to project file `{}`; rename one",
                pair[0].2, pair[1].2, pair[0].0
            ),
        ));
    }
    let identity = PresetIdentity {
        schema: TOKEN_SCHEMA,
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        hash: package.hash().to_owned(),
        tokens_hash: snapshot.hash(),
        design: manifest.design,
        media: media.iter().map(|(path, _, _)| path.clone()).collect(),
    };
    let mut files: BTreeMap<ProjectPath, Vec<u8>> = BTreeMap::new();
    let mut put = |p: &str, bytes: Vec<u8>| files.insert(path(p.to_owned()), bytes);
    put(TOKENS_PATH, snapshot.runtime_tokens_json().into_bytes());
    put(OVERRIDES_PATH, overrides.to_canonical_bytes());
    put(GUIDE_PATH, package.guide().to_vec());
    put(
        PRESET_PATH,
        serde_json::to_vec(&identity).expect("identity serializes"),
    );
    put(
        PRESET_TOKENS_PATH,
        package.tokens().to_canonical_json().into_bytes(),
    );
    for (path, bytes, _) in &media {
        put(path, (*bytes).clone());
    }
    for notice in &manifest.licenses {
        put(
            &format!("{NOTICE_PREFIX}{}", notice.file.path.as_str()),
            package.files()[&notice.file.path].clone(),
        );
    }
    for example in &manifest.examples {
        put(
            &format!("{NOTICE_PREFIX}{}", example.file.path.as_str()),
            package.files()[&example.file.path].clone(),
        );
    }
    Ok(MaterializedStyle {
        files: files.into_iter().collect(),
        snapshot,
        identity,
    })
}

/// Re-merge a project's stored preset defaults with (possibly edited) overrides, offline.
/// Inputs are the bytes of `style/preset.json`, `style/preset-tokens.json`, `style/overrides.json`.
/// `scene` additionally applies that scene's override layer.
pub fn reresolve_project_style(
    preset_json: &[u8],
    preset_tokens: &[u8],
    overrides: &[u8],
    scene: Option<&str>,
) -> Result<(PresetIdentity, ResolvedSnapshot), PresetError> {
    json_guard(preset_json, PRESET_PATH)?;
    check_schema(preset_json, PRESET_PATH, TOKEN_SCHEMA)?;
    let identity: PresetIdentity = serde_json::from_slice(preset_json).map_err(|e| {
        PresetError::one(
            Code::Malformed,
            PRESET_PATH,
            format!("invalid preset identity: {e}"),
        )
    })?;
    let defaults = TokenSet::parse(preset_tokens, &identity.design)?;
    let overrides = OverridesFile::parse(overrides, &identity.design)?;
    let snapshot = resolve(
        &defaults,
        Some(overrides.project()),
        scene.and_then(|id| overrides.scene(id)),
    );
    Ok((identity, snapshot))
}
