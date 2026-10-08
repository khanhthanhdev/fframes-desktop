use std::collections::HashSet;

use ring::signature::{ED25519, UnparsedPublicKey};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const RELEASE_MANIFEST_SCHEMA_VERSION: u32 = 1;
const SIGNING_DOMAIN: &[u8] = b"fframes-studio-release-manifest/v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseArtifact {
    /// HTTPS location. The signed digest, not the location, identifies the bytes.
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFile {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub license_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub sequence: u64,
    pub app_version: String,
    pub source_commit: String,
    pub target_triple: String,
    pub minimum_os: String,
    pub app: ReleaseArtifact,
    pub sdk: ReleaseArtifact,
    pub sdk_id: String,
    pub sdk_version: String,
    pub app_supports_sdk_range: String,
    pub sdk_supports_app_range: String,
    pub studio_protocol_version: u32,
    pub worker_protocol_versions: Vec<u32>,
    pub export_capabilities: Vec<ExportCapability>,
    pub runtime_inventory: Vec<RuntimeFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportCapability {
    pub backend: String,
    pub container: String,
    pub video_codec: String,
    pub audio_codec: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseEnvelope {
    pub payload: ReleaseManifest,
    pub key_id: String,
    /// Lowercase hexadecimal Ed25519 signature over the domain-separated canonical payload.
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedReleaseKey {
    pub key_id: String,
    pub public_key: [u8; 32],
}

/// A release envelope that has passed signature, target, compatibility, and sequence checks.
/// Its private field prevents callers from treating unsigned feed data as trusted download input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedReleaseManifest {
    envelope: ReleaseEnvelope,
}

impl VerifiedReleaseManifest {
    pub fn verify_json(
        json: &str,
        trusted_keys: &[TrustedReleaseKey],
        expected_target: &str,
        highest_accepted_sequence: u64,
        recovery_pair: Option<&ReleaseIdentity>,
    ) -> Result<Self, ReleaseVerificationError> {
        Ok(Self {
            envelope: ReleaseEnvelope::verify_json(
                json,
                trusted_keys,
                expected_target,
                highest_accepted_sequence,
                recovery_pair,
            )?,
        })
    }

    pub fn manifest(&self) -> &ReleaseManifest {
        &self.envelope.payload
    }

    pub fn app_artifact(&self) -> &ReleaseArtifact {
        &self.envelope.payload.app
    }

    pub fn sdk_artifact(&self) -> &ReleaseArtifact {
        &self.envelope.payload.sdk
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseIdentity {
    pub sequence: u64,
    pub app_version: String,
    pub target_triple: String,
    pub app_sha256: String,
    pub sdk_sha256: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReleaseVerificationError {
    #[error("invalid release manifest JSON: {0}")]
    Json(String),
    #[error("unsupported release manifest schema version: {0}")]
    UnsupportedSchema(u32),
    #[error("invalid release manifest: {0}")]
    InvalidManifest(String),
    #[error("release target '{actual}' does not match this app target '{expected}'")]
    TargetMismatch { expected: String, actual: String },
    #[error("release sequence {actual} is not newer than accepted sequence {highest}")]
    ReplayOrDowngrade { highest: u64, actual: u64 },
    #[error("release sequence is not an exact locally verified recovery pair")]
    InvalidRecovery,
    #[error("release signer '{0}' is not trusted")]
    UnknownSigner(String),
    #[error("release signature is malformed")]
    MalformedSignature,
    #[error("release signature verification failed")]
    InvalidSignature,
}

impl ReleaseManifest {
    pub fn validate(&self) -> Result<(), ReleaseVerificationError> {
        if self.schema_version != RELEASE_MANIFEST_SCHEMA_VERSION {
            return Err(ReleaseVerificationError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        let app_version = Version::parse(&self.app_version).map_err(|error| {
            ReleaseVerificationError::InvalidManifest(format!("invalid app version: {error}"))
        })?;
        let sdk_version = Version::parse(&self.sdk_version).map_err(|error| {
            ReleaseVerificationError::InvalidManifest(format!("invalid SDK version: {error}"))
        })?;
        let app_supports_sdk =
            VersionReq::parse(&self.app_supports_sdk_range).map_err(|error| {
                ReleaseVerificationError::InvalidManifest(format!(
                    "invalid app SDK compatibility range: {error}"
                ))
            })?;
        let sdk_supports_app =
            VersionReq::parse(&self.sdk_supports_app_range).map_err(|error| {
                ReleaseVerificationError::InvalidManifest(format!(
                    "invalid SDK app compatibility range: {error}"
                ))
            })?;
        if self.sequence == 0
            || self.source_commit.len() != 40 && self.source_commit.len() != 64
            || !self
                .source_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.target_triple.is_empty()
            || self.minimum_os.trim().is_empty()
            || self.sdk_id.trim().is_empty()
            || self.studio_protocol_version == 0
            || self.worker_protocol_versions.is_empty()
            || self.worker_protocol_versions.contains(&0)
        {
            return Err(ReleaseVerificationError::InvalidManifest(
                "missing release identity, target, OS baseline, or protocol version".into(),
            ));
        }
        validate_artifact("app", &self.app)?;
        validate_artifact("SDK", &self.sdk)?;

        if !sdk_supports_app.matches(&app_version) || !app_supports_sdk.matches(&sdk_version) {
            return Err(ReleaseVerificationError::InvalidManifest(
                "app and SDK versions do not satisfy their declared compatibility ranges".into(),
            ));
        }

        let mut inventory_paths = HashSet::new();
        for file in &self.runtime_inventory {
            if !is_safe_relative_path(&file.path)
                || !inventory_paths.insert(file.path.as_str())
                || !is_sha256(&file.sha256)
                || file.license_id.trim().is_empty()
            {
                return Err(ReleaseVerificationError::InvalidManifest(format!(
                    "invalid runtime inventory entry '{}'",
                    file.path
                )));
            }
        }

        for capability in &self.export_capabilities {
            if capability.backend.trim().is_empty()
                || capability.container.trim().is_empty()
                || capability.video_codec.trim().is_empty()
                || capability.audio_codec.trim().is_empty()
            {
                return Err(ReleaseVerificationError::InvalidManifest(
                    "export capabilities must name backend, container, video and audio codecs"
                        .into(),
                ));
            }
        }

        Ok(())
    }

    pub fn identity(&self) -> ReleaseIdentity {
        ReleaseIdentity {
            sequence: self.sequence,
            app_version: self.app_version.clone(),
            target_triple: self.target_triple.clone(),
            app_sha256: self.app.sha256.clone(),
            sdk_sha256: self.sdk.sha256.clone(),
        }
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, ReleaseVerificationError> {
        serde_json::to_vec(self).map_err(|error| ReleaseVerificationError::Json(error.to_string()))
    }
}

impl ReleaseEnvelope {
    /// Authenticates metadata and checks its target/sequence before trusting artifact URLs.
    /// `recovery_pair` must be the exact pair previously verified and retained locally.
    pub fn verify_json(
        json: &str,
        trusted_keys: &[TrustedReleaseKey],
        expected_target: &str,
        highest_accepted_sequence: u64,
        recovery_pair: Option<&ReleaseIdentity>,
    ) -> Result<Self, ReleaseVerificationError> {
        let envelope: Self = serde_json::from_str(json)
            .map_err(|error| ReleaseVerificationError::Json(error.to_string()))?;
        envelope.payload.validate()?;
        if envelope.payload.target_triple != expected_target {
            return Err(ReleaseVerificationError::TargetMismatch {
                expected: expected_target.to_owned(),
                actual: envelope.payload.target_triple.clone(),
            });
        }

        let key = trusted_keys
            .iter()
            .find(|key| key.key_id == envelope.key_id)
            .ok_or_else(|| ReleaseVerificationError::UnknownSigner(envelope.key_id.clone()))?;
        let signature =
            decode_hex(&envelope.signature).ok_or(ReleaseVerificationError::MalformedSignature)?;
        if signature.len() != 64 {
            return Err(ReleaseVerificationError::MalformedSignature);
        }
        let canonical_payload = envelope.payload.canonical_payload()?;
        let mut signed_bytes = Vec::with_capacity(SIGNING_DOMAIN.len() + canonical_payload.len());
        signed_bytes.extend_from_slice(SIGNING_DOMAIN);
        signed_bytes.extend_from_slice(&canonical_payload);
        UnparsedPublicKey::new(&ED25519, key.public_key)
            .verify(&signed_bytes, &signature)
            .map_err(|_| ReleaseVerificationError::InvalidSignature)?;

        if envelope.payload.sequence <= highest_accepted_sequence {
            match recovery_pair {
                Some(previous) if previous == &envelope.payload.identity() => {}
                Some(_) => return Err(ReleaseVerificationError::InvalidRecovery),
                None => {
                    return Err(ReleaseVerificationError::ReplayOrDowngrade {
                        highest: highest_accepted_sequence,
                        actual: envelope.payload.sequence,
                    });
                }
            }
        }

        Ok(envelope)
    }
}

fn validate_artifact(
    name: &str,
    artifact: &ReleaseArtifact,
) -> Result<(), ReleaseVerificationError> {
    if !artifact.url.starts_with("https://")
        || artifact.url.to_ascii_lowercase().contains("latest")
        || !is_sha256(&artifact.sha256)
        || artifact.size_bytes == 0
    {
        return Err(ReleaseVerificationError::InvalidManifest(format!(
            "{name} artifact must have an immutable HTTPS identity, SHA-256 and non-zero size"
        )));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.as_bytes().get(1).is_some_and(|byte| *byte == b':')
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Some((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::*;

    const TARGET: &str = "x86_64-unknown-linux-gnu";

    fn manifest() -> ReleaseManifest {
        ReleaseManifest {
            schema_version: RELEASE_MANIFEST_SCHEMA_VERSION,
            sequence: 2,
            app_version: "0.2.0".into(),
            source_commit: "a".repeat(40),
            target_triple: TARGET.into(),
            minimum_os: "Ubuntu 24.04".into(),
            app: ReleaseArtifact {
                url: "https://releases.example.test/v0.2.0/studio.tar.zst".into(),
                sha256: "1".repeat(64),
                size_bytes: 10,
            },
            sdk: ReleaseArtifact {
                url: "https://releases.example.test/v0.2.0/sdk.tar.zst".into(),
                sha256: "2".repeat(64),
                size_bytes: 20,
            },
            sdk_id: "studio-sdk-linux-v2".into(),
            sdk_version: "0.2.0".into(),
            app_supports_sdk_range: "^0.2.0".into(),
            sdk_supports_app_range: "^0.2.0".into(),
            studio_protocol_version: 1,
            worker_protocol_versions: vec![1],
            export_capabilities: vec![],
            runtime_inventory: vec![RuntimeFile {
                path: "lib/libcodec.so".into(),
                sha256: "3".repeat(64),
                size_bytes: 30,
                license_id: "ffmpeg-gpl".into(),
            }],
        }
    }

    fn keypair() -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap()
    }

    fn signed_envelope(payload: ReleaseManifest) -> (String, TrustedReleaseKey) {
        let pair = keypair();
        let mut bytes = SIGNING_DOMAIN.to_vec();
        bytes.extend_from_slice(&payload.canonical_payload().unwrap());
        let signature = pair.sign(&bytes);
        let envelope = ReleaseEnvelope {
            payload,
            key_id: "release-2026".into(),
            signature: signature
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        };
        let key = TrustedReleaseKey {
            key_id: "release-2026".into(),
            public_key: pair.public_key().as_ref().try_into().unwrap(),
        };
        (serde_json::to_string(&envelope).unwrap(), key)
    }

    #[test]
    fn verifies_signed_release_and_binds_artifacts_to_payload() {
        let (json, key) = signed_envelope(manifest());
        let envelope = ReleaseEnvelope::verify_json(&json, &[key], TARGET, 1, None).unwrap();
        assert_eq!(envelope.payload.sdk.sha256, "2".repeat(64));
    }

    #[test]
    fn rejects_unknown_signer_tampering_wrong_target_and_replay() {
        let (json, key) = signed_envelope(manifest());
        assert!(matches!(
            ReleaseEnvelope::verify_json(&json, &[], TARGET, 1, None),
            Err(ReleaseVerificationError::UnknownSigner(_))
        ));
        assert!(matches!(
            ReleaseEnvelope::verify_json(
                &json,
                std::slice::from_ref(&key),
                "aarch64-apple-darwin",
                1,
                None
            ),
            Err(ReleaseVerificationError::TargetMismatch { .. })
        ));
        assert!(matches!(
            ReleaseEnvelope::verify_json(&json, std::slice::from_ref(&key), TARGET, 2, None),
            Err(ReleaseVerificationError::ReplayOrDowngrade { .. })
        ));
        let tampered = json.replace("Ubuntu 24.04", "Ubuntu 26.04");
        assert!(matches!(
            ReleaseEnvelope::verify_json(&tampered, &[key], TARGET, 1, None),
            Err(ReleaseVerificationError::InvalidSignature)
        ));
    }

    #[test]
    fn accepts_only_the_exact_locally_verified_previous_pair_for_recovery() {
        let (json, key) = signed_envelope(manifest());
        let identity = manifest().identity();
        assert!(
            ReleaseEnvelope::verify_json(
                &json,
                std::slice::from_ref(&key),
                TARGET,
                2,
                Some(&identity)
            )
            .is_ok()
        );
        let mut other = identity;
        other.sdk_sha256 = "4".repeat(64);
        assert!(matches!(
            ReleaseEnvelope::verify_json(&json, &[key], TARGET, 2, Some(&other)),
            Err(ReleaseVerificationError::InvalidRecovery)
        ));
    }

    #[test]
    fn rejects_malformed_artifact_inventory_and_incompatible_versions() {
        let mut payload = manifest();
        payload.runtime_inventory[0].path = "../../outside".into();
        assert!(matches!(
            payload.validate(),
            Err(ReleaseVerificationError::InvalidManifest(_))
        ));
        let mut payload = manifest();
        payload.sdk_supports_app_range = "^0.1.0".into();
        assert!(matches!(
            payload.validate(),
            Err(ReleaseVerificationError::InvalidManifest(_))
        ));
        let mut payload = manifest();
        payload.app.url = "https://example.test/latest/app.tar".into();
        assert!(matches!(
            payload.validate(),
            Err(ReleaseVerificationError::InvalidManifest(_))
        ));
    }

    #[test]
    fn rejects_missing_protocol_and_incomplete_export_capabilities() {
        let mut payload = manifest();
        payload.studio_protocol_version = 0;
        assert!(matches!(
            payload.validate(),
            Err(ReleaseVerificationError::InvalidManifest(_))
        ));

        let mut payload = manifest();
        payload.export_capabilities.push(ExportCapability {
            backend: "cpu".into(),
            container: "mp4".into(),
            video_codec: "h264".into(),
            audio_codec: " ".into(),
        });
        assert!(matches!(
            payload.validate(),
            Err(ReleaseVerificationError::InvalidManifest(_))
        ));
    }
}
