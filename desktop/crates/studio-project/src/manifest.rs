use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{ProjectPath, revision::excluded};

pub const SCHEMA_VERSION: u32 = 1;
/// Version of the generated instructions and Studio integration (worker, timing opt-in)
/// a new project gets. A project of another version was made by another Studio and is
/// not opened.
pub const CURRENT_INSTRUCTION_VERSION: u32 = 2;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// Corrective diagnostics retain a native filename and a portable field name.
#[derive(Debug, thiserror::Error)]
#[error("{}: {field}: {reason}; {action}", studio_bootstrap::external_path(.file).display())]
pub struct ProjectError {
    pub file: PathBuf,
    pub field: String,
    pub reason: String,
    pub action: String,
}
impl ProjectError {
    pub fn new(file: impl Into<PathBuf>, field: &str, reason: impl ToString, action: &str) -> Self {
        Self {
            file: file.into(),
            field: field.into(),
            reason: reason.to_string(),
            action: action.into(),
        }
    }
}

/// Stable portable identity; generate once at creation and retain across relocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProjectId(String);
impl TryFrom<String> for ProjectId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        identifier(&value)
            .then_some(Self(value))
            .ok_or_else(|| "invalid project identifier".into())
    }
}
impl From<ProjectId> for String {
    fn from(value: ProjectId) -> Self {
        value.0
    }
}
impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Refuses a `studio.json` whose `field` is not `current`, saying which Studio made it.
fn exact_version(raw: &serde_json::Value, field: &str, current: u32) -> Result<(), ProjectError> {
    let err =
        |reason: String, action: &str| ProjectError::new("studio.json", field, reason, action);
    let version = raw
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| err("missing or invalid version".into(), "Correct this field"))?;
    match version.cmp(&u64::from(current)) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Greater => Err(err(
            format!("made by a newer Studio (version {version}, this Studio reads {current})"),
            "Update Studio",
        )),
        std::cmp::Ordering::Less => Err(err(
            format!("made by an older Studio (version {version}, this Studio reads {current})"),
            "The project is left as it is and cannot be opened",
        )),
    }
}

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        && value != "."
        && value != ".."
}
fn checksum(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
fn cargo_identifier(value: &str) -> bool {
    identifier(value) && !value.contains('.')
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkPin {
    pub release: String,
    pub compatibility_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CargoEntry {
    pub manifest: ProjectPath,
    pub package: String,
    pub worker_target: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayMetadata {
    pub name: String,
    pub description: Option<String>,
}
/// Informational hints only. Compiled Rust remains the timing and canvas authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoHints {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// 1920x1080 at 30 fps, the frame the project templates are written for.
impl Default for VideoHints {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetReference {
    pub id: String,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub display: DisplayMetadata,
    pub sdk: SdkPin,
    pub entry: CargoEntry,
    pub assets: Vec<ProjectPath>,
    pub generated_instruction_version: u32,
    pub video_hints: Option<VideoHints>,
    pub preset: Option<PresetReference>,
}
impl Manifest {
    /// Read-only parsing. A manifest of another schema or instruction version is refused
    /// before it becomes typed state; the file is left as it is.
    pub fn parse(bytes: &[u8]) -> Result<Self, ProjectError> {
        let err = |field: &str, reason: String, action: &str| {
            ProjectError::new("studio.json", field, reason, action)
        };
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(err(
                "",
                "manifest exceeds 1 MiB".into(),
                "Reduce metadata size",
            ));
        }
        let raw: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| err("", e.to_string(), "Correct the JSON"))?;
        exact_version(&raw, "schema_version", SCHEMA_VERSION)?;
        exact_version(
            &raw,
            "generated_instruction_version",
            CURRENT_INSTRUCTION_VERSION,
        )?;
        let manifest: Self = serde_json::from_value(raw)
            .map_err(|e| err("", e.to_string(), "Correct manifest fields"))?;
        manifest.validate()?;
        Ok(manifest)
    }
    pub fn validate(&self) -> Result<(), ProjectError> {
        let check = |valid: bool, field: &str| {
            if valid {
                Ok(())
            } else {
                Err(ProjectError::new(
                    "studio.json",
                    field,
                    "invalid or out-of-bounds value",
                    "Correct this field",
                ))
            }
        };
        check(self.schema_version == SCHEMA_VERSION, "schema_version")?;
        check(
            !self.display.name.trim().is_empty()
                && self.display.name.len() <= 256
                && !self.display.name.chars().any(char::is_control),
            "display.name",
        )?;
        check(
            self.display
                .description
                .as_ref()
                .is_none_or(|s| s.len() <= 4096 && !s.contains('\0')),
            "display.description",
        )?;
        check(
            identifier(&self.sdk.release) && checksum(&self.sdk.compatibility_sha256),
            "sdk",
        )?;
        check(
            cargo_identifier(&self.entry.package) && cargo_identifier(&self.entry.worker_target),
            "entry",
        )?;
        check(
            self.entry.manifest.as_str().rsplit('/').next() == Some("Cargo.toml")
                && !excluded(self.entry.manifest.as_str()),
            "entry.manifest",
        )?;
        check(self.assets.len() <= 4096, "assets")?;
        check(
            self.assets.iter().all(|path| !excluded(path.as_str())),
            "assets",
        )?;
        let unique: std::collections::HashSet<_> = self.assets.iter().collect();
        check(unique.len() == self.assets.len(), "assets")?;
        check(
            self.generated_instruction_version == CURRENT_INSTRUCTION_VERSION,
            "generated_instruction_version",
        )?;
        if let Some(v) = &self.video_hints {
            check(
                (1..=16384).contains(&v.width)
                    && (1..=16384).contains(&v.height)
                    && (1..=1000).contains(&v.fps),
                "video_hints",
            )?;
        }
        if let Some(p) = &self.preset {
            check(identifier(&p.id) && checksum(&p.sha256), "preset")?;
        }
        Ok(())
    }
}
