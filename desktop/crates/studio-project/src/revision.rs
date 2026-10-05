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
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: ProjectPath,
    pub kind: FileKind,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub executable: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInventory {
    /// Hash format. Zero is reserved for the two historical, unversioned formats.
    #[serde(default)]
    pub version: u32,
    pub files: Vec<SourceFile>,
    pub revision: SourceRevision,
}
const CURRENT_INVENTORY_VERSION: u32 = 1;
const HASH_PREFIX: &[u8] = b"studio-source-v1\0";
pub(crate) fn excluded(path: &str) -> bool {
    [".git", "target", ".fframes/context", ".fframes/cache"]
        .iter()
        .any(|p| {
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
                || kind(file.path.as_str()) != file.kind
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
        let valid = match self.version {
            CURRENT_INVENTORY_VERSION => {
                inventory_revision(&self.files, HASH_PREFIX, true) == self.revision
            }
            0 => {
                inventory_revision(&self.files, HASH_PREFIX, true) == self.revision
                    || (self.files.iter().all(|file| !file.executable)
                        && inventory_revision(&self.files, HASH_PREFIX, false) == self.revision)
            }
            _ => {
                return Err(io_error(
                    Path::new("checkpoint"),
                    format!("unsupported inventory version {}", self.version),
                ));
            }
        };
        if !valid {
            return Err(io_error(
                Path::new("checkpoint"),
                "inventory revision integrity failure",
            ));
        }
        Ok(())
    }
    /// Tests scanned source against current and historical revision formats.
    /// The oldest format is accepted only when every scanned file is non-executable,
    /// because that format did not protect executable metadata.
    pub fn matches_revision(&self, historical: &SourceRevision) -> bool {
        inventory_revision(&self.files, HASH_PREFIX, true) == *historical
            || (self.files.iter().all(|file| !file.executable)
                && inventory_revision(&self.files, HASH_PREFIX, false) == *historical)
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
        let revision = inventory_revision(&files, HASH_PREFIX, true);
        Ok(Self {
            version: CURRENT_INVENTORY_VERSION,
            files,
            revision,
        })
    }
}
fn inventory_revision(
    files: &[SourceFile],
    prefix: &[u8],
    include_executable: bool,
) -> SourceRevision {
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
        if include_executable {
            revision.update([u8::from(file.executable)]);
        }
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
