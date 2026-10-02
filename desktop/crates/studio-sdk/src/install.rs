use crate::doctor::{Doctor, ProbeStatus};
use crate::manifest::{CompatibilityManifest, SdkArtifact};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use tar::Archive;
use thiserror::Error;
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
    #[error("path traversal detected in archive entry: {0}")]
    PathTraversal(String),
    #[error("unsupported file type in archive: {0}")]
    UnsupportedEntryType(String),
    #[error("candidate sdk failed verification: {reason}")]
    CandidateVerificationFailed { reason: String },
    #[error("installation error: {0}")]
    Other(String),
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

    /// Safely extracts a .tar.gz archive into target directory, validating against path traversal.
    pub fn safe_extract_tar_gz(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        fs::create_dir_all(target_dir)?;
        let file = File::open(archive_path)?;
        let tar = GzDecoder::new(file);
        let mut archive = Archive::new(tar);

        for entry_res in archive.entries()? {
            let mut entry = entry_res?;
            let path = entry.path()?.to_path_buf();

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
        // 0. Verify completeness of provided artifacts against manifest
        for required_artifact in &manifest.artifacts {
            if !artifact_files
                .iter()
                .any(|(a, _)| a.name == required_artifact.name)
            {
                return Err(InstallError::Other(format!(
                    "missing required artifact '{}' from install set",
                    required_artifact.name
                )));
            }
        }

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

                // 1. Verify Checksum
                if let Err(err) = Self::verify_file_checksum(file_path, &artifact.sha256) {
                    // Quarantine corrupt artifact
                    fs::create_dir_all(self.quarantine_dir())?;
                    let quarantined = self.quarantine_dir().join(format!(
                        "{}.corrupt.{}",
                        artifact.name,
                        &artifact.sha256[..8]
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
        let backup_active = self.sdk_home.join(format!(
            "active.backup.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));

        let active_existed = if active.exists() {
            fs::rename(&active, &backup_active)?;
            true
        } else {
            false
        };

        if let Err(err) = fs::rename(staging_dir, &active) {
            if active_existed {
                let _ = fs::rename(&backup_active, &active);
            }
            return Err(InstallError::Io(err));
        }

        if active_existed {
            if previous.exists() {
                let _ = fs::remove_dir_all(&previous);
            }
            let _ = fs::rename(&backup_active, &previous);
        }

        Ok(())
    }
    /// Roll back active SDK to previous SDK.
    pub fn rollback(&self) -> Result<bool, InstallError> {
        let active = self.active_sdk_dir();
        let previous = self.previous_sdk_dir();

        if !previous.exists() {
            return Ok(false);
        }

        let temp_old = self.sdk_home.join("temp_rollback");
        if temp_old.exists() {
            fs::remove_dir_all(&temp_old)?;
        }

        if active.exists() {
            fs::rename(&active, &temp_old)?;
        }

        fs::rename(&previous, &active)?;
        if temp_old.exists() {
            fs::remove_dir_all(&temp_old)?;
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

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
        manifest.artifacts[1].sha256 = ffmpeg_hash;
        let framework_tar = tmp.path().join("framework.tar.gz");
        manifest.artifacts[2].sha256 = create_test_tar_gz(
            &framework_tar,
            &[("unrelated.txt", b"incomplete framework", 0o644)],
        );

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
