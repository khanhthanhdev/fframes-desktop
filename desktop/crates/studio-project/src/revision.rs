use std::{fs, io::Read, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    ProjectError, ProjectPath,
    paths::{io_error, is_link, link_error},
};

/// Content identity, independent of project location, timestamps and enumeration order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SourceRevision(String);
impl TryFrom<String> for SourceRevision {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(Self(value))
        } else {
            Err("invalid SHA-256 revision".into())
        }
    }
}
impl From<SourceRevision> for String {
    fn from(value: SourceRevision) -> Self {
        value.0
    }
}
impl std::fmt::Display for SourceRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl SourceRevision {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum FileKind {
    Cargo = 0,
    Rust = 1,
    Media = 2,
    Instructions = 3,
    Style = 4,
    Configuration = 5,
    Other = 6,
    /// The timing document (`timeline.json`): play order and trims, which the studio
    /// edits without rebuilding.
    Timing = 7,
}
/// The project's timing document, at the project root.
pub const TIMING_FILE: &str = "timeline.json";
/// The project's props document, at the project root: the values Studio's props panel
/// gives editable components. Like the timing document it is applied at run time, so
/// it stays out of the build; its kind is [`FileKind::Other`].
pub const PROPS_FILE: &str = "props.json";
/// The project's slides document, at the project root: slide stops and speaker notes.
/// Studio applies it at run time like the props document; its kind is
/// [`FileKind::Other`].
pub const SLIDES_FILE: &str = "slides.json";
/// Whether `path` is a document the preview and export apply at run time.
pub fn run_time_document(path: &str) -> bool {
    path == TIMING_FILE || path == PROPS_FILE || path == SLIDES_FILE
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: ProjectPath,
    pub kind: FileKind,
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInventory {
    /// Hash format; only [`CURRENT_INVENTORY_VERSION`] is read.
    pub version: u32,
    pub files: Vec<SourceFile>,
    pub revision: SourceRevision,
}
/// Version 2 hashes the timing document as its own kind.
pub const CURRENT_INVENTORY_VERSION: u32 = 2;
const HASH_PREFIX: &[u8] = b"studio-source-v2\0";
const BUILD_HASH_PREFIX: &[u8] = b"studio-build-v1\0";
pub(crate) fn excluded(path: &str) -> bool {
    crate::paths::PROTECTED_TREES.iter().any(|p| {
        path == *p
            || path
                .strip_prefix(p)
                .is_some_and(|tail| tail.starts_with('/'))
    })
}
fn kind(path: &str) -> FileKind {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name == "Cargo.toml" || name == "Cargo.lock" {
        FileKind::Cargo
    } else if path.starts_with("media/") {
        FileKind::Media
    } else if path.starts_with("style/") {
        FileKind::Style
    } else if name == "AGENTS.md" || name == "CLAUDE.md" {
        FileKind::Instructions
    } else if path.ends_with(".rs") {
        FileKind::Rust
    } else if path == "studio.json" || path.starts_with(".cargo/") {
        FileKind::Configuration
    } else if path == TIMING_FILE {
        FileKind::Timing
    } else {
        FileKind::Other
    }
}
#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}
#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}
impl SourceInventory {
    /// Verify an immutable manifest against the same byte/path identity as a source scan.
    pub fn verify(&self) -> Result<(), ProjectError> {
        let mut previous = None;
        for file in &self.files {
            if previous.is_some_and(|p: &ProjectPath| p >= &file.path)
                || excluded(file.path.as_str())
                || !kind_matches(file)
            {
                return Err(io_error(
                    Path::new(file.path.as_str()),
                    "invalid checkpoint inventory",
                ));
            }
            let _: SourceRevision = file
                .sha256
                .clone()
                .try_into()
                .map_err(|e: String| io_error(Path::new(file.path.as_str()), e))?;
            previous = Some(&file.path);
        }
        self.check_version(Path::new("checkpoint"))?;
        if inventory_revision(&self.files, HASH_PREFIX) != self.revision {
            return Err(io_error(
                Path::new("checkpoint"),
                "inventory revision integrity failure",
            ));
        }
        Ok(())
    }
    /// Refuses an inventory of any format but the current one, naming `file`. An older
    /// one was made by an older Studio and is never read, repaired or rewritten.
    pub fn check_version(&self, file: &Path) -> Result<(), ProjectError> {
        check_inventory_version(self.version, file)
    }
    /// Whether the scanned source has exactly the recorded revision.
    pub fn matches_revision(&self, recorded: &SourceRevision) -> bool {
        inventory_revision(&self.files, HASH_PREFIX) == *recorded
    }
    /// What a build depends on: the revision of every file except the timing and props
    /// documents, which the preview and export apply at run time. A change of only those
    /// keeps it.
    pub fn build_revision(&self) -> SourceRevision {
        let files: Vec<SourceFile> = self
            .files
            .iter()
            .filter(|file| !run_time_document(file.path.as_str()))
            .cloned()
            .collect();
        inventory_revision(&files, BUILD_HASH_PREFIX)
    }
    /// The timing document's entry, when the project has one.
    pub fn timing_file(&self) -> Option<&SourceFile> {
        self.files
            .iter()
            .find(|file| file.path.as_str() == TIMING_FILE)
    }
    /// The props document's entry, when the project has one.
    pub fn props_file(&self) -> Option<&SourceFile> {
        self.files
            .iter()
            .find(|file| file.path.as_str() == PROPS_FILE)
    }
    /// The slides document's entry, when the project has one.
    pub fn slides_file(&self) -> Option<&SourceFile> {
        self.files
            .iter()
            .find(|file| file.path.as_str() == SLIDES_FILE)
    }
    /// Includes all ordinary files except the four exact root-relative exclusions.
    /// Unknown durable files are included to avoid silently dropping imported source.
    /// Caller must reconcile again before installing results; scanning is not a filesystem snapshot.
    pub fn scan(root: &Path) -> Result<Self, ProjectError> {
        Self::scan_with_cancel(root, &|| false)
    }
    /// Cancellation is checked between directory entries and each 64 KiB read.
    pub fn scan_with_cancel(
        root: &Path,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, ProjectError> {
        let root = fs::canonicalize(root).map_err(|e| io_error(root, e))?;
        let mut paths = Vec::new();
        enumerate(&root, &root, &mut paths, cancelled)?;
        paths.sort();
        let mut files = Vec::new();
        let mut buffer = [0; 64 * 1024];
        for path in paths {
            let mut file = path.open_file(&root)?;
            let meta = file
                .metadata()
                .map_err(|e| io_error(&root.join(path.as_str()), e))?;
            let executable = is_executable(&meta);
            let mut digest = Sha256::new();
            let mut size = 0u64;
            loop {
                if cancelled() {
                    return Err(io_error(&root, "Source scan cancelled"));
                }
                let read = file
                    .read(&mut buffer)
                    .map_err(|e| io_error(&root.join(path.as_str()), e))?;
                if read == 0 {
                    break;
                }
                size += read as u64;
                digest.update(&buffer[..read]);
            }
            let digest = digest.finalize();
            let kind = kind(path.as_str());
            files.push(SourceFile {
                path,
                kind,
                size,
                sha256: format!("{digest:x}"),
                executable,
            });
        }
        let revision = inventory_revision(&files, HASH_PREFIX);
        Ok(Self {
            version: CURRENT_INVENTORY_VERSION,
            files,
            revision,
        })
    }
}
/// Refuses an inventory `version` other than the current one, naming `file`.
pub(crate) fn check_inventory_version(version: u32, file: &Path) -> Result<(), ProjectError> {
    match version.cmp(&CURRENT_INVENTORY_VERSION) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Less => Err(ProjectError::new(
            file,
            "inventory",
            format!(
                "made by an older Studio (inventory format {version}, this Studio reads format {CURRENT_INVENTORY_VERSION})"
            ),
            "it is left as it is and cannot be opened by this Studio",
        )),
        std::cmp::Ordering::Greater => Err(ProjectError::new(
            file,
            "inventory",
            format!(
                "made by a newer Studio (inventory format {version}, this Studio reads format {CURRENT_INVENTORY_VERSION})"
            ),
            "update Studio; it was left as it is",
        )),
    }
}
fn kind_matches(file: &SourceFile) -> bool {
    kind(file.path.as_str()) == file.kind
}
fn inventory_revision(files: &[SourceFile], prefix: &[u8]) -> SourceRevision {
    let mut revision = Sha256::new();
    revision.update(prefix);
    for file in files {
        let digest: Vec<u8> = (0..64)
            .step_by(2)
            .map(|i| u8::from_str_radix(&file.sha256[i..i + 2], 16).unwrap())
            .collect();
        revision.update((file.path.as_str().len() as u64).to_le_bytes());
        revision.update(file.path.as_str().as_bytes());
        revision.update([file.kind as u8]);
        revision.update(file.size.to_le_bytes());
        revision.update([u8::from(file.executable)]);
        revision.update(digest);
    }
    SourceRevision(format!("{:x}", revision.finalize()))
}
fn enumerate(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<ProjectPath>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProjectError> {
    for entry in fs::read_dir(directory).map_err(|e| io_error(directory, e))? {
        if cancelled() {
            return Err(io_error(directory, "Source scan cancelled"));
        }
        let entry = entry.map_err(|e| io_error(directory, e))?;
        let native = entry.path();
        let relative = native
            .strip_prefix(root)
            .map_err(|e| io_error(&native, e))?;
        let mut parts = Vec::new();
        for part in relative.components() {
            parts.push(part.as_os_str().to_str().ok_or_else(|| {
                ProjectError::new(
                    &native,
                    "path",
                    "non-UTF-8 portable name",
                    "Rename the file using UTF-8",
                )
            })?);
        }
        let name = parts.join("/");
        if excluded(&name) {
            continue;
        }
        let path = ProjectPath::try_from(name)?;
        let meta = fs::symlink_metadata(&native).map_err(|e| io_error(&native, e))?;
        if is_link(&meta) {
            return Err(link_error(&native));
        }
        path.resolve_existing(root)?;
        if meta.is_dir() {
            enumerate(root, &native, paths, cancelled)?;
        } else if meta.is_file() {
            // Exact app-generated transaction files (staged bytes, displaced originals,
            // retained variants) are bookkeeping, not project source.
            let internal = native
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(crate::paths::is_transaction_internal_name);
            if !internal {
                paths.push(path);
            }
        } else {
            return Err(io_error(&native, "unsupported special file"));
        }
    }
    Ok(())
}
