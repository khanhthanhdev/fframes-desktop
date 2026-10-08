use crate::manifest::{CompatibilityManifest, SdkArtifact};
use crate::{
    disk::{DiskSpaceError, ensure_available},
    doctor::{Doctor, ProbeStatus},
};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use tar::Archive;
use thiserror::Error;

const MAX_ARCHIVE_ENTRIES: u64 = 500_000;
const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const INSTALL_RECEIPT_SCHEMA_VERSION: u32 = 1;
const BUILD_PROBE_RESERVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const INSTALL_TRANSACTION_FILE: &str = "install-transaction.json";
const ROLLBACK_TRANSACTION_FILE: &str = "rollback-transaction.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallTransaction {
    staging: String,
    active_backup: String,
    previous_backup: String,
    previous_existed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RollbackTransaction {
    active_backup: String,
    active_existed: bool,
    stage: RollbackStage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RollbackStage {
    Prepared,
    Finalizing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkInstallReceipt {
    pub schema_version: u32,
    pub compatibility_digest: String,
    pub artifacts: Vec<SdkArtifactReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkArtifactReceipt {
    pub name: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("io error during installation: {0}")]
    Io(#[from] io::Error),
    #[error("checksum mismatch for artifact '{name}': expected {expected}, actual {actual}")]
    ChecksumMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("artifact '{name}' size mismatch: expected {expected}, actual {actual}")]
    SizeMismatch {
        name: String,
        expected: u64,
        actual: u64,
    },
    #[error("path traversal detected in archive entry: {0}")]
    PathTraversal(String),
    #[error("unsupported file type in archive: {0}")]
    UnsupportedEntryType(String),
    #[error("archive exceeds safe extraction limit: {0}")]
    ArchiveLimitExceeded(String),
    #[error("candidate sdk failed verification: {reason}")]
    CandidateVerificationFailed { reason: String },
    #[error("installation error: {0}")]
    Other(String),
    #[error(transparent)]
    DiskSpace(#[from] DiskSpaceError),
}

pub struct SdkInstaller {
    sdk_home: PathBuf,
    processes: studio_bootstrap::ProcessTreeManager,
}

impl SdkInstaller {
    pub fn new(sdk_home: impl Into<PathBuf>) -> Self {
        Self {
            sdk_home: sdk_home.into(),
            processes: studio_bootstrap::ProcessTreeManager::new(),
        }
    }

    /// Use the app's owner so shutdown can cancel verification and reject later compilers.
    pub fn with_process_manager(mut self, processes: studio_bootstrap::ProcessTreeManager) -> Self {
        self.processes = processes;
        self
    }

    pub fn active_sdk_dir(&self) -> PathBuf {
        self.sdk_home.join("active")
    }

    pub fn previous_sdk_dir(&self) -> PathBuf {
        self.sdk_home.join("previous")
    }

    pub fn staging_root_dir(&self) -> PathBuf {
        self.sdk_home.join("staging")
    }

    pub fn quarantine_dir(&self) -> PathBuf {
        self.sdk_home.join("quarantine")
    }

    fn acquire_install_lock(&self) -> Result<File, InstallError> {
        fs::create_dir_all(&self.sdk_home)?;
        let lock_path = self.sdk_home.join(".install.lock");
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        lock.try_lock().map_err(|error| {
            InstallError::Other(format!(
                "another SDK install or recovery is in progress: {error}"
            ))
        })?;
        Ok(lock)
    }

    /// Completes or rolls back a promotion interrupted by process exit. It never
    /// replaces an unknown active SDK or deletes the last known-good copy.
    pub fn recover_interrupted_install(&self) -> Result<bool, InstallError> {
        let _lock = self.acquire_install_lock()?;
        self.recover_interrupted_transactions_locked()
    }

    fn recover_interrupted_transactions_locked(&self) -> Result<bool, InstallError> {
        let install_recovered = self.recover_interrupted_install_locked()?;
        let rollback_recovered = self.recover_interrupted_rollback_locked()?;
        Ok(install_recovered || rollback_recovered)
    }

    fn recover_interrupted_install_locked(&self) -> Result<bool, InstallError> {
        let journal = self.sdk_home.join(INSTALL_TRANSACTION_FILE);
        let bytes = match fs::read(&journal) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let transaction: InstallTransaction = serde_json::from_slice(&bytes)
            .map_err(|error| InstallError::Other(format!("SDK recovery journal: {error}")))?;
        let path = |name: &str| -> Result<PathBuf, InstallError> {
            let path = Path::new(name);
            if path.as_os_str().is_empty()
                || path.is_absolute()
                || path
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                return Err(InstallError::Other(
                    "SDK recovery journal contains an unsafe path".into(),
                ));
            }
            Ok(self.sdk_home.join(path))
        };
        let staging = path(&transaction.staging)?;
        let active = self.active_sdk_dir();
        let previous = self.previous_sdk_dir();
        let active_backup = path(&transaction.active_backup)?;
        let previous_backup = path(&transaction.previous_backup)?;

        if staging.exists() {
            // Activation did not happen. Restore active if it had already been moved.
            if active_backup.exists() {
                if active.exists() {
                    return Err(InstallError::Other(
                        "cannot recover SDK install: both active and its backup exist before candidate activation".into(),
                    ));
                }
                fs::rename(&active_backup, &active)?;
            }
            if previous_backup.exists() && !previous.exists() {
                fs::rename(&previous_backup, &previous)?;
            }
            fs::remove_dir_all(&staging)?;
        } else {
            // The verified candidate moved into active; finish rotating the old SDK.
            if !active.exists() {
                if active_backup.exists() {
                    fs::rename(&active_backup, &active)?;
                } else {
                    return Err(InstallError::Other(
                        "cannot recover SDK install: no active or backup SDK exists".into(),
                    ));
                }
            } else if active_backup.exists() {
                if transaction.previous_existed && previous.exists() && !previous_backup.exists() {
                    fs::rename(&previous, &previous_backup)?;
                }
                if !previous.exists() {
                    fs::rename(&active_backup, &previous)?;
                }
            }
            if previous_backup.exists() {
                fs::remove_dir_all(&previous_backup)?;
            }
        }
        if active_backup.exists() {
            fs::remove_dir_all(active_backup)?;
        }
        if previous_backup.exists() {
            fs::remove_dir_all(previous_backup)?;
        }
        fs::remove_file(journal)?;
        sync_directory(&self.sdk_home)?;
        Ok(true)
    }

    fn recover_interrupted_rollback_locked(&self) -> Result<bool, InstallError> {
        let journal = self.sdk_home.join(ROLLBACK_TRANSACTION_FILE);
        let bytes = match fs::read(&journal) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let transaction: RollbackTransaction = serde_json::from_slice(&bytes)
            .map_err(|error| InstallError::Other(format!("SDK rollback journal: {error}")))?;
        let backup_name = Path::new(&transaction.active_backup);
        if backup_name.components().count() != 1
            || !matches!(
                backup_name.components().next(),
                Some(std::path::Component::Normal(_))
            )
            || !transaction.active_backup.starts_with("rollback-active.")
        {
            return Err(InstallError::Other(
                "SDK rollback journal contains an unsafe backup path".into(),
            ));
        }

        let active = self.active_sdk_dir();
        let previous = self.previous_sdk_dir();
        let active_backup = self.sdk_home.join(backup_name);

        if transaction.stage == RollbackStage::Prepared {
            if transaction.active_existed && !active_backup.exists() && active.exists() {
                fs::rename(&active, &active_backup)?;
                sync_directory(&self.sdk_home)?;
            }
            if !active.exists() && previous.exists() {
                fs::rename(&previous, &active)?;
                sync_directory(&self.sdk_home)?;
            }
            if transaction.active_existed {
                if !active.exists() || previous.exists() || !active_backup.exists() {
                    return Err(InstallError::Other(
                        "SDK rollback recovery found an unexpected prepared directory state".into(),
                    ));
                }
            } else if !active.exists() || previous.exists() || active_backup.exists() {
                return Err(InstallError::Other(
                    "SDK rollback recovery found an unexpected prepared directory state".into(),
                ));
            }
            let finalizing = RollbackTransaction {
                stage: RollbackStage::Finalizing,
                ..transaction
            };
            write_json_atomic(&journal, &finalizing)?;
        }

        if transaction.active_existed && active_backup.exists() && !previous.exists() {
            fs::rename(&active_backup, &previous)?;
            sync_directory(&self.sdk_home)?;
        }
        if !active.exists()
            || (transaction.active_existed && (!previous.exists() || active_backup.exists()))
            || (!transaction.active_existed && (previous.exists() || active_backup.exists()))
        {
            return Err(InstallError::Other(
                "SDK rollback recovery could not restore a consistent active/previous pair".into(),
            ));
        }

        fs::remove_file(journal)?;
        sync_directory(&self.sdk_home)?;
        Ok(true)
    }

    /// Verifies the SHA-256 hash of a file against expected hex string.
    pub fn verify_file_checksum(
        file_path: &Path,
        expected_sha256: &str,
    ) -> Result<String, InstallError> {
        let mut file = File::open(file_path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        let actual_hash = format!("{:x}", hasher.finalize());
        if !actual_hash.eq_ignore_ascii_case(expected_sha256) {
            return Err(InstallError::ChecksumMismatch {
                name: file_path.to_string_lossy().to_string(),
                expected: expected_sha256.to_string(),
                actual: actual_hash,
            });
        }

        Ok(actual_hash)
    }

    /// Reuse an already installed SDK only when its immutable receipt exactly matches
    /// the requested compatibility manifest and the installed toolchain still probes.
    /// This path deliberately does not inspect or require the original archives.
    pub fn reusable_active_for_app(
        &self,
        manifest: &CompatibilityManifest,
        app_version: &str,
        processes: Option<&studio_bootstrap::ProcessTreeManager>,
    ) -> Result<Option<PathBuf>, InstallError> {
        manifest
            .validate_for_current_app_version(app_version)
            .map_err(|error| InstallError::Other(error.to_string()))?;
        let active = self.active_sdk_dir();
        let installed_manifest_path = active.join("compatibility.json");
        let installed_manifest = match fs::read_to_string(&installed_manifest_path) {
            Ok(json) => CompatibilityManifest::from_json_str(&json)
                .map_err(|error| InstallError::Other(format!("installed SDK manifest: {error}")))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if installed_manifest.digest() != manifest.digest() {
            return Ok(None);
        }

        let receipt_path = active.join("install-receipt.json");
        let receipt: SdkInstallReceipt = match fs::read(&receipt_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| InstallError::Other(format!("installed SDK receipt: {error}")))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let expected_artifacts: Vec<_> = manifest
            .artifacts
            .iter()
            .map(|artifact| SdkArtifactReceipt {
                name: artifact.name.clone(),
                sha256: artifact.sha256.to_ascii_lowercase(),
                size_bytes: artifact.size_bytes,
            })
            .collect();
        if receipt.schema_version != INSTALL_RECEIPT_SCHEMA_VERSION
            || receipt.compatibility_digest != manifest.digest()
            || receipt.artifacts != expected_artifacts
        {
            return Ok(None);
        }

        if Doctor::verify_candidate_sdk_with_processes(&active, manifest, processes).is_ready() {
            Ok(Some(active))
        } else {
            Ok(None)
        }
    }

    /// Safely extracts a .tar.gz archive into target directory, validating against path traversal.
    pub fn safe_extract_tar_gz(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        fs::create_dir_all(target_dir)?;
        let file = File::open(archive_path)?;
        let tar = GzDecoder::new(file);
        let mut archive = Archive::new(tar);

        let mut entry_count = 0u64;
        let mut expanded_bytes = 0u64;
        for entry_res in archive.entries()? {
            let mut entry = entry_res?;
            let path = entry.path()?.to_path_buf();
            entry_count = entry_count.saturating_add(1);
            expanded_bytes = expanded_bytes.saturating_add(entry.size());
            if entry_count > MAX_ARCHIVE_ENTRIES {
                return Err(InstallError::ArchiveLimitExceeded(format!(
                    "more than {MAX_ARCHIVE_ENTRIES} entries"
                )));
            }
            if expanded_bytes > MAX_ARCHIVE_EXPANDED_BYTES {
                return Err(InstallError::ArchiveLimitExceeded(format!(
                    "expanded size exceeds {MAX_ARCHIVE_EXPANDED_BYTES} bytes"
                )));
            }

            // Validate against directory traversal
            let path_str = path.to_string_lossy();
            if path_str.starts_with('/') || path_str.contains("..") {
                return Err(InstallError::PathTraversal(path_str.to_string()));
            }

            let entry_type = entry.header().entry_type();
            if entry_type.is_file() || entry_type.is_dir() {
                entry.unpack_in(target_dir)?;
            } else {
                return Err(InstallError::UnsupportedEntryType(path_str.to_string()));
            }
        }

        Ok(())
    }

    /// Transactionally stages, verifies, and promotes an SDK bundle.
    pub fn install_from_local_artifacts<P: AsRef<Path>>(
        &self,
        manifest: &CompatibilityManifest,
        artifact_files: &[(SdkArtifact, P)],
    ) -> Result<PathBuf, InstallError> {
        self.install_from_local_artifacts_for_app(
            manifest,
            artifact_files,
            env!("CARGO_PKG_VERSION"),
        )
    }

    /// Transactionally installs an SDK after checking it against the owning Studio app version.
    pub fn install_from_local_artifacts_for_app<P: AsRef<Path>>(
        &self,
        manifest: &CompatibilityManifest,
        artifact_files: &[(SdkArtifact, P)],
        app_version: &str,
    ) -> Result<PathBuf, InstallError> {
        manifest
            .validate_for_current_app_version(app_version)
            .map_err(|error| InstallError::Other(error.to_string()))?;
        let _lock = self.acquire_install_lock()?;
        self.recover_interrupted_transactions_locked()?;

        // Bind every supplied archive to the validated manifest before reading or
        // extracting it. The artifact path and digest are part of this identity.
        validate_artifact_set(manifest, artifact_files)?;

        // Archives are retained in the acquisition cache. Reserve room for a
        // conservative 4x expansion estimate plus the independent Cargo build/render
        // probes; extraction also enforces hard entry and expanded-byte limits.
        let artifact_bytes = manifest.artifacts.iter().fold(0u64, |total, artifact| {
            total.saturating_add(artifact.size_bytes)
        });
        let required_space = artifact_bytes
            .saturating_mul(4)
            .saturating_add(BUILD_PROBE_RESERVE_BYTES);
        ensure_available(&self.sdk_home, required_space)?;

        let tx_id = format!(
            "tx_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        let staging_dir = self.staging_root_dir().join(&tx_id);
        fs::create_dir_all(&staging_dir)?;

        // Clean up staging on any failure
        let result = (|| -> Result<(), InstallError> {
            for (artifact, file_path) in artifact_files {
                if self.processes.is_shutdown() {
                    return Err(InstallError::Other("SDK installation cancelled".into()));
                }
                let file_path = file_path.as_ref();

                // 1. Verify the signed size and checksum before extraction.
                let actual_size = fs::metadata(file_path)?.len();
                if actual_size != artifact.size_bytes {
                    return Err(InstallError::SizeMismatch {
                        name: artifact.name.clone(),
                        expected: artifact.size_bytes,
                        actual: actual_size,
                    });
                }
                if let Err(err) = Self::verify_file_checksum(file_path, &artifact.sha256) {
                    // Quarantine corrupt artifact
                    fs::create_dir_all(self.quarantine_dir())?;
                    let quarantined = self.quarantine_dir().join(format!(
                        "{}.corrupt.{}",
                        artifact.name,
                        artifact.sha256.get(..8).unwrap_or("invalid")
                    ));
                    let _ = fs::copy(file_path, &quarantined);
                    return Err(err);
                }

                // 2. Extract into destination subdirectory in staging
                let target_subdir = staging_dir.join(&artifact.destination_subdir);
                Self::safe_extract_tar_gz(file_path, &target_subdir)?;
            }

            if self.processes.is_shutdown() {
                return Err(InstallError::Other("SDK installation cancelled".into()));
            }
            // 3. Write embedded manifest into staging root
            let manifest_path = staging_dir.join("compatibility.json");
            let manifest_json = serde_json::to_string_pretty(manifest)
                .map_err(|e| InstallError::Other(e.to_string()))?;
            fs::write(manifest_path, manifest_json)?;

            // 4. Verify candidate SDK via Doctor before promotion
            let candidate_report = Doctor::verify_candidate_sdk_with_processes(
                &staging_dir,
                manifest,
                Some(&self.processes),
            );
            if candidate_report.overall_status == ProbeStatus::Fail {
                return Err(InstallError::CandidateVerificationFailed {
                    reason: candidate_report.format_summary(),
                });
            }

            // A checksummed archive and directory layout do not establish buildability.
            let probe_root = staging_dir.join("verification");
            let manager = self.processes.sub_manager();
            let environment = crate::SdkEnvironment::new(
                &staging_dir,
                probe_root.join("target"),
                manifest.clone(),
                true,
            );
            let verification = (|| -> Result<(), crate::ProjectError> {
                let template = probe_root.join("template");
                crate::ProjectManager::generate_cpu_project_with_sdk(
                    "sdk-template-probe",
                    &template,
                    &manifest.fframes_version,
                    Some(&staging_dir),
                )?;
                crate::ProjectManager::build_project(&template, &environment, &manager)?;
                let (_, pixels) = crate::ProjectManager::render_frame(
                    &template,
                    &environment,
                    0,
                    Path::new("frames"),
                    &manager,
                )?;
                if !pixels.starts_with(b"\x89PNG\r\n\x1a\n") {
                    return Err(crate::ProjectError::BuildError(
                        "Template did not render a PNG".into(),
                    ));
                }
                let worker = probe_root.join("worker");
                crate::ProjectManager::generate_annotated_worker_project(&worker, &staging_dir)?;
                crate::ProjectManager::build_project(&worker, &environment, &manager)?;
                let (_, pixels) = crate::ProjectManager::render_frame(
                    &worker,
                    &environment,
                    0,
                    Path::new("frames"),
                    &manager,
                )?;
                if !pixels.starts_with(b"\x89PNG\r\n\x1a\n") {
                    return Err(crate::ProjectError::BuildError(
                        "Worker did not render a PNG".into(),
                    ));
                }
                Ok(())
            })();
            manager.terminate_all(std::time::Duration::from_millis(300));
            verification.map_err(|error| InstallError::CandidateVerificationFailed {
                reason: error.to_string(),
            })?;
            let receipt = SdkInstallReceipt {
                schema_version: INSTALL_RECEIPT_SCHEMA_VERSION,
                compatibility_digest: manifest.digest(),
                artifacts: manifest
                    .artifacts
                    .iter()
                    .map(|artifact| SdkArtifactReceipt {
                        name: artifact.name.clone(),
                        sha256: artifact.sha256.to_ascii_lowercase(),
                        size_bytes: artifact.size_bytes,
                    })
                    .collect(),
            };
            let receipt_path = staging_dir.join("install-receipt.json");
            let mut receipt_file = File::create(&receipt_path)?;
            serde_json::to_writer_pretty(&mut receipt_file, &receipt)
                .map_err(|error| InstallError::Other(error.to_string()))?;
            use std::io::Write;
            receipt_file.write_all(b"\n")?;
            receipt_file.sync_all()?;
            fs::remove_dir_all(&probe_root)?;
            if self.processes.is_shutdown() {
                return Err(InstallError::Other("SDK installation cancelled".into()));
            }

            Ok(())
        })();

        if let Err(err) = result {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(err);
        }

        // 5. Atomic and recoverable promotion
        self.promote_staging(&staging_dir)?;

        Ok(self.active_sdk_dir())
    }

    /// Atomically and recoverably promotes staging directory to active SDK.
    /// Preserves existing active SDK as previous SDK on success, and restores active on failure.
    fn promote_staging(&self, staging_dir: &Path) -> Result<(), InstallError> {
        let active = self.active_sdk_dir();
        let previous = self.previous_sdk_dir();
        let transaction_id = format!(
            "{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let backup_name = format!("active.backup.{transaction_id}");
        let previous_backup_name = format!("previous.backup.{transaction_id}");
        let staging_name = staging_dir
            .strip_prefix(&self.sdk_home)
            .map_err(|_| InstallError::Other("staging path is outside SDK storage".into()))?
            .to_string_lossy()
            .into_owned();
        let transaction = InstallTransaction {
            staging: staging_name,
            active_backup: backup_name.clone(),
            previous_backup: previous_backup_name.clone(),
            previous_existed: previous.exists(),
        };
        let journal = self.sdk_home.join(INSTALL_TRANSACTION_FILE);
        write_json_atomic(&journal, &transaction)?;
        let backup_active = self.sdk_home.join(&backup_name);
        let previous_backup = self.sdk_home.join(&previous_backup_name);

        if active.exists() {
            fs::rename(&active, &backup_active)?;
            sync_directory(&self.sdk_home)?;
        }
        if let Err(error) = fs::rename(staging_dir, &active) {
            self.recover_interrupted_transactions_locked()?;
            return Err(InstallError::Io(error));
        }
        sync_directory(&self.sdk_home)?;

        if backup_active.exists() {
            if previous.exists() {
                fs::rename(&previous, &previous_backup)?;
                sync_directory(&self.sdk_home)?;
            }
            if let Err(error) = fs::rename(&backup_active, &previous) {
                self.recover_interrupted_transactions_locked()?;
                return Err(InstallError::Io(error));
            }
        }
        if previous_backup.exists() {
            fs::remove_dir_all(&previous_backup)?;
        }
        fs::remove_file(journal)?;
        sync_directory(&self.sdk_home)?;
        Ok(())
    }
    /// Roll back active SDK to previous SDK.
    pub fn rollback(&self) -> Result<bool, InstallError> {
        let _lock = self.acquire_install_lock()?;
        self.recover_interrupted_transactions_locked()?;
        let active = self.active_sdk_dir();
        let previous = self.previous_sdk_dir();

        if !previous.exists() {
            return Ok(false);
        }

        let transaction_id = format!(
            "{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let journal = self.sdk_home.join(ROLLBACK_TRANSACTION_FILE);
        write_json_atomic(
            &journal,
            &RollbackTransaction {
                active_backup: format!("rollback-active.{transaction_id}"),
                active_existed: active.exists(),
                stage: RollbackStage::Prepared,
            },
        )?;
        self.recover_interrupted_rollback_locked()?;
        Ok(true)
    }
}

fn validate_artifact_set<P>(
    manifest: &CompatibilityManifest,
    artifact_files: &[(SdkArtifact, P)],
) -> Result<(), InstallError> {
    if artifact_files.len() != manifest.artifacts.len() {
        return Err(InstallError::Other(
            "provided SDK artifacts do not exactly match the compatibility manifest".into(),
        ));
    }
    let mut matched = vec![false; manifest.artifacts.len()];
    for (provided, _) in artifact_files {
        let Some(index) = manifest
            .artifacts
            .iter()
            .enumerate()
            .position(|(index, expected)| !matched[index] && expected == provided)
        else {
            return Err(InstallError::Other(format!(
                "provided SDK artifact '{}' does not exactly match the compatibility manifest",
                provided.name
            )));
        };
        matched[index] = true;
    }
    if matched.iter().any(|matched| !matched) {
        return Err(InstallError::Other(
            "provided SDK artifacts are missing a manifest artifact".into(),
        ));
    }
    Ok(())
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("transaction journal has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    sync_directory(parent)
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    #[test]
    fn supplied_install_archives_must_exactly_match_manifest_identity() {
        let manifest = CompatibilityManifest::default_linux_x64();
        let mut artifacts: Vec<_> = manifest
            .artifacts
            .iter()
            .cloned()
            .map(|artifact| (artifact, PathBuf::from("unused")))
            .collect();
        validate_artifact_set(&manifest, &artifacts).unwrap();

        artifacts[0].0.destination_subdir = "/tmp/outside-sdk".into();
        assert!(matches!(
            validate_artifact_set(&manifest, &artifacts),
            Err(InstallError::Other(message)) if message.contains("does not exactly match")
        ));

        let mut extra = artifacts.clone();
        extra.push((manifest.artifacts[0].clone(), PathBuf::from("unused")));
        assert!(matches!(
            validate_artifact_set(&manifest, &extra),
            Err(InstallError::Other(message)) if message.contains("exactly match")
        ));
    }

    fn create_test_tar_gz(dest: &Path, files: &[(&str, &[u8], u32)]) -> String {
        let file = File::create(dest).unwrap();
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = tar::Builder::new(enc);

        for (name, content, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            tar.append(&header, *content).unwrap();
        }

        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();

        // Calculate sha256
        let mut f = File::open(dest).unwrap();
        let mut hasher = Sha256::new();
        io::copy(&mut f, &mut hasher).unwrap();
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn test_installer_lifecycle_and_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let installer = SdkInstaller::new(tmp.path().join("sdk_home"));

        let first = installer.staging_root_dir().join("first");
        fs::create_dir_all(&first).unwrap();
        fs::write(first.join("generation.txt"), "first").unwrap();
        installer.promote_staging(&first).unwrap();
        assert_eq!(
            fs::read_to_string(installer.active_sdk_dir().join("generation.txt")).unwrap(),
            "first"
        );
        assert!(!installer.rollback().unwrap());

        let second = installer.staging_root_dir().join("second");
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("generation.txt"), "second").unwrap();
        installer.promote_staging(&second).unwrap();
        assert_eq!(
            fs::read_to_string(installer.previous_sdk_dir().join("generation.txt")).unwrap(),
            "first"
        );
        assert!(installer.rollback().unwrap());
        assert_eq!(
            fs::read_to_string(installer.active_sdk_dir().join("generation.txt")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(installer.previous_sdk_dir().join("generation.txt")).unwrap(),
            "second"
        );
    }

    #[test]
    fn interrupted_sdk_rollback_finishes_a_prepared_swap() {
        let tmp = tempfile::tempdir().unwrap();
        let installer = SdkInstaller::new(tmp.path().join("sdk_home"));
        let active = installer.active_sdk_dir();
        let previous = installer.previous_sdk_dir();
        let backup = installer.sdk_home.join("rollback-active.crash");
        fs::create_dir_all(&previous).unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(previous.join("generation.txt"), "known-good").unwrap();
        fs::write(backup.join("generation.txt"), "candidate").unwrap();
        write_json_atomic(
            &installer.sdk_home.join(ROLLBACK_TRANSACTION_FILE),
            &RollbackTransaction {
                active_backup: "rollback-active.crash".into(),
                active_existed: true,
                stage: RollbackStage::Prepared,
            },
        )
        .unwrap();

        assert!(installer.recover_interrupted_install().unwrap());
        assert_eq!(
            fs::read_to_string(active.join("generation.txt")).unwrap(),
            "known-good"
        );
        assert_eq!(
            fs::read_to_string(previous.join("generation.txt")).unwrap(),
            "candidate"
        );
        assert!(!backup.exists());
        assert!(!installer.sdk_home.join(ROLLBACK_TRANSACTION_FILE).exists());
    }

    #[test]
    fn completed_sdk_rollback_is_not_replayed_after_crash_before_journal_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let installer = SdkInstaller::new(tmp.path().join("sdk_home"));
        let active = installer.active_sdk_dir();
        let previous = installer.previous_sdk_dir();
        fs::create_dir_all(&active).unwrap();
        fs::create_dir_all(&previous).unwrap();
        fs::write(active.join("generation.txt"), "known-good").unwrap();
        fs::write(previous.join("generation.txt"), "candidate").unwrap();
        write_json_atomic(
            &installer.sdk_home.join(ROLLBACK_TRANSACTION_FILE),
            &RollbackTransaction {
                active_backup: "rollback-active.crash".into(),
                active_existed: true,
                stage: RollbackStage::Finalizing,
            },
        )
        .unwrap();

        assert!(installer.recover_interrupted_install().unwrap());
        assert_eq!(
            fs::read_to_string(active.join("generation.txt")).unwrap(),
            "known-good"
        );
        assert_eq!(
            fs::read_to_string(previous.join("generation.txt")).unwrap(),
            "candidate"
        );
        assert!(!installer.sdk_home.join(ROLLBACK_TRANSACTION_FILE).exists());
    }

    #[test]
    fn interrupted_sdk_promotion_restores_the_active_sdk_before_activation() {
        let tmp = tempfile::tempdir().unwrap();
        let installer = SdkInstaller::new(tmp.path().join("sdk_home"));
        fs::create_dir_all(installer.active_sdk_dir()).unwrap();
        fs::write(
            installer.active_sdk_dir().join("generation.txt"),
            "known-good",
        )
        .unwrap();
        let staging = installer.staging_root_dir().join("tx-interrupted");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("generation.txt"), "candidate").unwrap();
        let active_backup = installer.sdk_home.join("active.backup.crash");
        fs::rename(installer.active_sdk_dir(), &active_backup).unwrap();
        let transaction = InstallTransaction {
            staging: "staging/tx-interrupted".into(),
            active_backup: "active.backup.crash".into(),
            previous_backup: "previous.backup.crash".into(),
            previous_existed: false,
        };
        write_json_atomic(
            &installer.sdk_home.join(INSTALL_TRANSACTION_FILE),
            &transaction,
        )
        .unwrap();

        assert!(installer.recover_interrupted_install().unwrap());
        assert_eq!(
            fs::read_to_string(installer.active_sdk_dir().join("generation.txt")).unwrap(),
            "known-good"
        );
        assert!(!staging.exists());
        assert!(!installer.sdk_home.join(INSTALL_TRANSACTION_FILE).exists());
    }

    #[test]
    fn interrupted_sdk_promotion_finishes_rotation_after_candidate_activation() {
        let tmp = tempfile::tempdir().unwrap();
        let installer = SdkInstaller::new(tmp.path().join("sdk_home"));
        let previous = installer.previous_sdk_dir();
        fs::create_dir_all(&previous).unwrap();
        fs::write(previous.join("generation.txt"), "older").unwrap();
        let active_backup = installer.sdk_home.join("active.backup.crash");
        fs::create_dir_all(&active_backup).unwrap();
        fs::write(active_backup.join("generation.txt"), "known-good").unwrap();
        let active = installer.active_sdk_dir();
        fs::create_dir_all(&active).unwrap();
        fs::write(active.join("generation.txt"), "candidate").unwrap();
        let transaction = InstallTransaction {
            staging: "staging/tx-already-moved".into(),
            active_backup: "active.backup.crash".into(),
            previous_backup: "previous.backup.crash".into(),
            previous_existed: true,
        };
        write_json_atomic(
            &installer.sdk_home.join(INSTALL_TRANSACTION_FILE),
            &transaction,
        )
        .unwrap();

        assert!(installer.recover_interrupted_install().unwrap());
        assert_eq!(
            fs::read_to_string(active.join("generation.txt")).unwrap(),
            "candidate"
        );
        assert_eq!(
            fs::read_to_string(previous.join("generation.txt")).unwrap(),
            "known-good"
        );
        assert!(!installer.sdk_home.join(INSTALL_TRANSACTION_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn receipt_backed_active_sdk_is_reused_without_original_archives() {
        let tmp = tempfile::tempdir().unwrap();
        let installer = SdkInstaller::new(tmp.path().join("sdk_home"));
        let active = installer.active_sdk_dir();
        let manifest = CompatibilityManifest::default_linux_x64();
        fs::create_dir_all(active.join("toolchain/bin")).unwrap();
        fs::create_dir_all(active.join("ffmpeg/include")).unwrap();
        fs::create_dir_all(active.join("ffmpeg/lib")).unwrap();
        fs::write(
            active.join("compatibility.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let target = &manifest.target_triple;
        let rustc = active.join("toolchain/bin/rustc");
        fs::write(
            &rustc,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo 'rustc {} (receipt fixture)'\nelse\n  echo 'host: {}'\nfi\n",
                manifest.rust_toolchain.channel, target
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755)).unwrap();
        let receipt = SdkInstallReceipt {
            schema_version: INSTALL_RECEIPT_SCHEMA_VERSION,
            compatibility_digest: manifest.digest(),
            artifacts: manifest
                .artifacts
                .iter()
                .map(|artifact| SdkArtifactReceipt {
                    name: artifact.name.clone(),
                    sha256: artifact.sha256.to_ascii_lowercase(),
                    size_bytes: artifact.size_bytes,
                })
                .collect(),
        };
        fs::write(
            active.join("install-receipt.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();

        assert_eq!(
            installer
                .reusable_active_for_app(&manifest, env!("CARGO_PKG_VERSION"), None)
                .unwrap(),
            Some(active.clone())
        );

        // A receipt is only reusable while all declared SDK identity fields still agree.
        let mut invalid = receipt;
        invalid.compatibility_digest = "0".repeat(64);
        fs::write(
            active.join("install-receipt.json"),
            serde_json::to_vec(&invalid).unwrap(),
        )
        .unwrap();
        assert!(
            installer
                .reusable_active_for_app(&manifest, env!("CARGO_PKG_VERSION"), None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn app_version_incompatibility_fails_before_staging_any_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let installer = SdkInstaller::new(tmp.path().join("sdk_home"));
        let mut manifest = CompatibilityManifest::default_linux_x64();
        manifest.target_triple = if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "gnu"
        )) {
            "x86_64-unknown-linux-gnu"
        } else if cfg!(all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc"
        )) {
            "x86_64-pc-windows-msvc"
        } else {
            "aarch64-apple-darwin"
        }
        .into();

        let artifacts: Vec<(SdkArtifact, PathBuf)> = Vec::new();
        let result = installer.install_from_local_artifacts_for_app(&manifest, &artifacts, "9.0.0");
        assert!(matches!(
            result,
            Err(InstallError::Other(message)) if message.contains("supports app versions")
        ));
        assert!(!installer.staging_root_dir().exists());
    }

    #[test]
    fn test_installer_rejects_incomplete_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let installer = SdkInstaller::new(tmp.path().join("sdk_home"));

        // Missing rustc executable in toolchain archive
        let toolchain_tar = tmp.path().join("toolchain_broken.tar.gz");
        let toolchain_hash =
            create_test_tar_gz(&toolchain_tar, &[("lib/test.txt", b"no compiler", 0o644)]);

        let ffmpeg_tar = tmp.path().join("ffmpeg.tar.gz");
        let ffmpeg_hash = create_test_tar_gz(
            &ffmpeg_tar,
            &[
                ("include/libavcodec/avcodec.h", b"// avcodec", 0o644),
                ("lib/libavcodec.so", b"dummy lib", 0o644),
            ],
        );

        let mut manifest = CompatibilityManifest::default_linux_x64();
        manifest.artifacts[0].sha256 = toolchain_hash;
        manifest.artifacts[0].size_bytes = fs::metadata(&toolchain_tar).unwrap().len();
        manifest.artifacts[1].sha256 = ffmpeg_hash;
        manifest.artifacts[1].size_bytes = fs::metadata(&ffmpeg_tar).unwrap().len();
        let framework_tar = tmp.path().join("framework.tar.gz");
        manifest.artifacts[2].sha256 = create_test_tar_gz(
            &framework_tar,
            &[("unrelated.txt", b"incomplete framework", 0o644)],
        );
        manifest.artifacts[2].size_bytes = fs::metadata(&framework_tar).unwrap().len();

        let res = installer.install_from_local_artifacts(
            &manifest,
            &[
                (manifest.artifacts[0].clone(), &toolchain_tar),
                (manifest.artifacts[1].clone(), &ffmpeg_tar),
                (manifest.artifacts[2].clone(), &framework_tar),
            ],
        );

        assert!(matches!(
            res,
            Err(InstallError::CandidateVerificationFailed { .. })
        ));
        assert!(!installer.active_sdk_dir().exists());
    }
}
