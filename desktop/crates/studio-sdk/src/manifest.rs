use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use thiserror::Error;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("unsupported schema version: expected {expected}, got {actual}")]
    UnsupportedSchemaVersion { expected: u32, actual: u32 },
    #[error(
        "target triple mismatch: manifest targets '{manifest_target}', but host is '{host_target}'"
    )]
    TargetMismatch {
        manifest_target: String,
        host_target: String,
    },
    #[error(
        "invalid artifact path '{path}': directory traversal ('..') or absolute paths are not permitted"
    )]
    InvalidArtifactPath { path: String },
    #[error("artifact '{name}' has empty or invalid SHA-256 checksum")]
    InvalidChecksum { name: String },
    #[error(
        "artifact '{name}' has moving or mutable URL '{url}' (e.g. 'latest') which is prohibited"
    )]
    MovingUrlProhibited { name: String, url: String },
    #[error("json parsing error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("validation error: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustToolchainInfo {
    pub channel: String,
    pub components: Vec<String>,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkArtifact {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub destination_subdir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FfmpegManifestInfo {
    pub tag: String,
    pub link_mode: String,
    pub headers_rel_path: String,
    pub libs_rel_path: String,
    pub bin_rel_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPrerequisiteProbe {
    pub id: String,
    pub name: String,
    pub description: String,
    pub command: String,
    pub args: Vec<String>,
    pub package_name: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityManifest {
    pub schema_version: u32,
    pub sdk_id: String,
    pub compatible_app_range: String,
    pub target_triple: String,
    pub arch: String,
    pub os_baseline: String,
    pub rust_toolchain: RustToolchainInfo,
    pub fframes_version: String,
    pub cargo_fframes_version: String,
    pub artifacts: Vec<SdkArtifact>,
    pub ffmpeg: FfmpegManifestInfo,
    pub host_prerequisites: Vec<HostPrerequisiteProbe>,
}

impl CompatibilityManifest {
    pub fn from_json_str(json: &str) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_str(json)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchemaVersion {
                expected: CURRENT_SCHEMA_VERSION,
                actual: self.schema_version,
            });
        }

        if self.sdk_id.is_empty() {
            return Err(ManifestError::Validation("sdk_id cannot be empty".into()));
        }

        for artifact in &self.artifacts {
            if artifact.sha256.trim().len() != 64 {
                return Err(ManifestError::InvalidChecksum {
                    name: artifact.name.clone(),
                });
            }

            // Prohibit moving 'latest' identifiers
            if artifact.url.to_ascii_lowercase().contains("latest") {
                return Err(ManifestError::MovingUrlProhibited {
                    name: artifact.name.clone(),
                    url: artifact.url.clone(),
                });
            }

            // Prevent path traversal
            let path = Path::new(&artifact.destination_subdir);
            if path.is_absolute()
                || artifact.destination_subdir.contains("..")
                || artifact.destination_subdir.starts_with('/')
            {
                return Err(ManifestError::InvalidArtifactPath {
                    path: artifact.destination_subdir.clone(),
                });
            }
        }

        // Validate ffmpeg relative paths
        for path_str in [&self.ffmpeg.headers_rel_path, &self.ffmpeg.libs_rel_path] {
            if path_str.contains("..") || path_str.starts_with('/') {
                return Err(ManifestError::InvalidArtifactPath {
                    path: path_str.clone(),
                });
            }
        }

        if let Some(bin_path) = &self.ffmpeg.bin_rel_path {
            let is_invalid = bin_path.contains("..") || bin_path.starts_with('/');
            if is_invalid {
                return Err(ManifestError::InvalidArtifactPath {
                    path: bin_path.clone(),
                });
            }
        }

        Ok(())
    }

    pub fn digest(&self) -> String {
        let serialized = serde_json::to_string(self).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn default_linux_x64() -> Self {
        Self::from_json_str(include_str!("../../../packaging/sdk/phase-zero-sdk.json"))
            .expect("bundled Linux compatibility manifest must be valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_manifest_parses_and_validates() {
        let manifest = CompatibilityManifest::default_linux_x64();
        manifest
            .validate()
            .expect("default linux manifest is valid");
        assert!(!manifest.digest().is_empty());
    }

    #[test]
    fn test_unsupported_schema_version_rejected() {
        let mut manifest = CompatibilityManifest::default_linux_x64();
        manifest.schema_version = 999;
        let err = manifest.validate().unwrap_err();
        assert!(matches!(
            err,
            ManifestError::UnsupportedSchemaVersion {
                expected: CURRENT_SCHEMA_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn test_path_traversal_rejected() {
        let mut manifest = CompatibilityManifest::default_linux_x64();
        manifest.artifacts[0].destination_subdir = "../escaping/path".into();
        let err = manifest.validate().unwrap_err();
        assert!(matches!(err, ManifestError::InvalidArtifactPath { .. }));
    }

    #[test]
    fn test_moving_latest_url_rejected() {
        let mut manifest = CompatibilityManifest::default_linux_x64();
        manifest.artifacts[0].url = "https://example.com/downloads/ffmpeg-latest.zip".into();
        let err = manifest.validate().unwrap_err();
        assert!(matches!(err, ManifestError::MovingUrlProhibited { .. }));
    }

    #[test]
    fn test_invalid_sha256_length_rejected() {
        let mut manifest = CompatibilityManifest::default_linux_x64();
        manifest.artifacts[0].sha256 = "tooshort".into();
        let err = manifest.validate().unwrap_err();
        assert!(matches!(err, ManifestError::InvalidChecksum { .. }));
    }
}
