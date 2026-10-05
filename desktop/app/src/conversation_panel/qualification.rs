//! Writer-containment qualification: the only way production code may call an adapter's
//! writer processes `ProcessGroupContained`.
//!
//! A settings value never qualifies anything. Ownership is derived from the installed
//! qualification ledger (`<data>/qualification/m3-results.json` with its `evidence/`
//! directory, the bundle `desktop/qualification/` of a harness run) and only when every one
//! of these holds, otherwise it stays [`WriterOwnership::Unknown`] with a stated reason:
//!
//! * the ledger is an M3 ledger whose adapter identity is a real (non-fixture) adapter
//!   that negotiated protocol v1;
//! * its `auth_writer_process_group` gate is an AUTHENTIC gate with status `pass`, and it
//!   is not backed by evidence a development gate also cites;
//! * every cited evidence file lies inside the ledger's `evidence/` directory and hashes
//!   to the recorded SHA-256;
//! * at least one cited file is an authentic, non-fixture writer-containment record of this
//!   exact platform whose adapter identity equals the configured adapter's launch identity
//!   (executable bytes, arguments that name files, argument words, sign-in variable names)
//!   and which measured no escaped writer and an empty group after an edit, a Stop and a
//!   provider crash.
//!
//! The ledger is a local file: this binds a qualification to the adapter and platform it
//! was measured for and rejects stale, partial, fixture and mismatched claims. It does not
//! defend against someone who can forge both the ledger and its evidence on this
//! computer (the same trust level as the settings file itself).
//!
//! Everything here reads files and hashes executables: call it from a background thread.
use super::host::AdapterFile;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use studio_agent_spike::{ExecutableSearch, resolve_executable};
use studio_bootstrap::WriterOwnership;
use studio_engine::app_paths::AppPaths;

/// Directory of the installed ledger bundle inside the app data directory.
pub const LEDGER_DIR: &str = "qualification";
pub const LEDGER_FILE: &str = "m3-results.json";
pub const WRITER_GATE: &str = "auth_writer_process_group";
/// `schema` of every authentic evidence record.
pub const EVIDENCE_SCHEMA: &str = "m3-authentic/1";
/// Names of the repository's scripted fixtures: never an authentic adapter.
pub const FIXTURE_AGENT_NAMES: [&str; 5] = [
    "scripted-agent",
    "protocol-peer",
    "acp-peer",
    "fixture",
    "test-agent",
];
const MAX_LEDGER_BYTES: u64 = 4 * 1024 * 1024;
const MAX_EVIDENCE_BYTES: u64 = 8 * 1024 * 1024;
/// Scenarios the writer-containment record must have measured.
const REQUIRED_SCENARIOS: [&str; 3] = ["edit", "stop", "provider_crash"];

/// The platform a measurement was made on (Python's `platform.system()` / `.machine()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub system: String,
    pub machine: String,
}

impl Platform {
    pub fn current() -> Self {
        let system = match std::env::consts::OS {
            "linux" => "Linux",
            "macos" => "Darwin",
            "windows" => "Windows",
            other => other,
        };
        let machine = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => "AMD64".to_owned(),
            ("windows", "aarch64") => "ARM64".to_owned(),
            ("macos", "aarch64") => "arm64".to_owned(),
            (_, arch) => arch.to_owned(),
        };
        Self {
            system: system.to_owned(),
            machine,
        }
    }
}

/// What a configured adapter launches, as bytes: the digest is what an evidence record
/// names. Paths are hashed, never recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchIdentity {
    pub executable_sha256: String,
    pub args: Vec<String>,
    pub auth_env_names: Vec<String>,
    /// `(argument index, sha256)` of every argument that names an existing absolute file
    /// (an interpreter's script, a bundle): changing it changes the identity.
    pub arg_files: Vec<(usize, String)>,
}

impl LaunchIdentity {
    /// The binding digest (`launch_identity` in evidence): SHA-256 over the NUL-joined
    /// parts `fframes-launch/1`, executable digest, argument count + arguments, variable
    /// name count + sorted names, file-argument count + `index:digest` entries. The Python
    /// harness computes the identical value.
    pub fn digest(&self) -> String {
        let mut names = self.auth_env_names.clone();
        names.sort();
        let mut parts: Vec<String> = vec![
            "fframes-launch/1".into(),
            self.executable_sha256.clone(),
            self.args.len().to_string(),
        ];
        parts.extend(self.args.iter().cloned());
        parts.push(names.len().to_string());
        parts.extend(names);
        parts.push(self.arg_files.len().to_string());
        parts.extend(self.arg_files.iter().map(|(i, d)| format!("{i}:{d}")));
        let mut hasher = Sha256::new();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                hasher.update([0u8]);
            }
            hasher.update(part.as_bytes());
        }
        hex(&hasher.finalize())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// The launch identity of `file`: resolves the executable exactly as the workflow does
/// (absolute path, or a name inside `<data>/adapters`; never the global `PATH`) and hashes
/// it. `Err` when it cannot be resolved or read (ownership then stays unknown).
pub fn launch_identity(file: &AdapterFile, paths: &AppPaths) -> Result<LaunchIdentity, String> {
    let search = ExecutableSearch {
        managed_dirs: vec![paths.data.join("adapters")],
        gui_path: None,
    };
    let executable = resolve_executable(&file.executable, &search)
        .map_err(|e| format!("the adapter executable cannot be resolved: {e}"))?;
    let executable_sha256 = hash_file(&executable)
        .map_err(|e| format!("the adapter executable cannot be read: {e}"))?;
    let mut arg_files = Vec::new();
    for (index, arg) in file.args.iter().enumerate() {
        let candidate = Path::new(arg);
        if candidate.is_absolute()
            && std::fs::metadata(candidate).is_ok_and(|m| m.is_file())
            && let Ok(digest) = hash_file(candidate)
        {
            arg_files.push((index, digest));
        }
    }
    Ok(LaunchIdentity {
        executable_sha256,
        args: file.args.clone(),
        auth_env_names: file.auth_env_names.clone(),
        arg_files,
    })
}

/// Why ownership is what it is; shown on the Setup tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Containment {
    /// Qualified by validated, hashed authentic evidence for exactly this launch.
    Qualified {
        evidence_sha256: String,
    },
    /// Containment is injected by a test harness without any evidence (never a
    /// qualification of anything).
    TestInjected {
        label: String,
    },
    Unknown {
        reason: String,
    },
}

/// Who may say a writer is contained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipPolicy {
    /// Production: only validated evidence qualifies.
    Validated,
    /// Test-only injection for fixtures (the `qualify-m3` observation entry): the given
    /// ownership is used as is and labelled as injected everywhere it is shown.
    TestInjected(WriterOwnership),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub ownership: WriterOwnership,
    pub containment: Containment,
}

impl Resolution {
    fn unknown(reason: impl Into<String>) -> Self {
        Self {
            ownership: WriterOwnership::Unknown,
            containment: Containment::Unknown {
                reason: reason.into(),
            },
        }
    }

    /// One line for the Setup tab.
    pub fn summary(&self) -> String {
        match &self.containment {
            Containment::Qualified { evidence_sha256 } => format!(
                "Writer containment: qualified for this exact adapter and platform (evidence {}). Candidates may be captured.",
                &evidence_sha256[..12.min(evidence_sha256.len())]
            ),
            Containment::TestInjected { label } => format!(
                "Writer containment: injected by the test harness ({label}); this is not a qualification."
            ),
            Containment::Unknown { reason } => format!(
                "Writer containment: unknown. Candidates from this adapter stay blocked. {reason}"
            ),
        }
    }
}

/// Resolves ownership for the configured adapter under `policy`. Background thread only.
pub fn resolve_ownership(
    file: &AdapterFile,
    paths: &AppPaths,
    policy: &OwnershipPolicy,
) -> Resolution {
    if let OwnershipPolicy::TestInjected(ownership) = policy {
        return Resolution {
            ownership: ownership.clone(),
            containment: Containment::TestInjected {
                label: ownership.label().to_owned(),
            },
        };
    }
    let identity = match launch_identity(file, paths) {
        Ok(identity) => identity,
        Err(reason) => return Resolution::unknown(reason),
    };
    let ledger = paths.data.join(LEDGER_DIR);
    resolve_with(&ledger, &identity.digest(), &Platform::current())
}

/// [`resolve_ownership`] against an explicit ledger bundle, launch digest and platform.
pub fn resolve_with(ledger_dir: &Path, launch_digest: &str, platform: &Platform) -> Resolution {
    match verify(ledger_dir, launch_digest, platform) {
        Ok(evidence_sha256) => Resolution {
            ownership: WriterOwnership::ProcessGroupContained {
                qualification: format!("m3-ledger:{WRITER_GATE}:{}", &evidence_sha256[..12]),
            },
            containment: Containment::Qualified { evidence_sha256 },
        },
        Err(reason) => Resolution::unknown(reason),
    }
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", name(path)))?;
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("cannot read {}: {e}", name(path)))?;
    if data.len() as u64 > limit {
        return Err(format!("{} is too large", name(path)));
    }
    Ok(data)
}

fn name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned())
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Validates the ledger and returns the SHA-256 of the evidence record that qualifies
/// `launch_digest` on `platform`.
fn verify(ledger_dir: &Path, launch_digest: &str, platform: &Platform) -> Result<String, String> {
    let ledger_path = ledger_dir.join(LEDGER_FILE);
    let bytes = match read_bounded(&ledger_path, MAX_LEDGER_BYTES) {
        Ok(bytes) => bytes,
        Err(_) if !ledger_path.exists() => {
            return Err(format!(
                "No installed qualification ledger (<data>/{LEDGER_DIR}/{LEDGER_FILE}); a writer is never contained on a setting alone."
            ));
        }
        Err(error) => return Err(error),
    };
    let ledger: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "the installed qualification ledger is not valid JSON".to_owned())?;
    if text(&ledger, "kind") != Some("m3") || ledger.get("schema_version") != Some(&Value::from(1))
    {
        return Err("the installed qualification ledger is not an M3 ledger".into());
    }
    let adapter = ledger
        .pointer("/environment/adapter")
        .filter(|a| a.is_object())
        .ok_or("the ledger records no authentic adapter (no qualification was run)")?;
    let agent = text(adapter, "agent_name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or("the ledger's adapter identity has no name")?;
    if FIXTURE_AGENT_NAMES.contains(&agent.to_ascii_lowercase().as_str()) {
        return Err("the ledger's adapter is a scripted fixture, which cannot qualify".into());
    }
    if adapter.get("protocol_version") != Some(&Value::from(1))
        || adapter.get("fixture_detected") != Some(&Value::Bool(false))
    {
        return Err("the ledger's adapter identity is not a real protocol-v1 adapter".into());
    }
    if text(adapter, "launch_identity") != Some(launch_digest) {
        return Err(
            "the ledger's qualified adapter is not this launch (executable, arguments or sign-in variable names differ)".into(),
        );
    }
    let gates = ledger
        .get("gates")
        .and_then(Value::as_object)
        .ok_or("the ledger has no gates")?;
    let gate = gates
        .get(WRITER_GATE)
        .ok_or("the ledger has no writer-containment gate")?;
    if text(gate, "kind") != Some("authentic") || text(gate, "status") != Some("pass") {
        return Err(
            "the writer-containment gate has not passed (it is not an authentic pass)".into(),
        );
    }
    let cited = |gate: &Value| -> Vec<(String, String)> {
        gate.get("evidence")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|e| {
                        Some((text(e, "path")?.to_owned(), text(e, "sha256")?.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let evidence = cited(gate);
    if evidence.is_empty() {
        return Err("the writer-containment gate cites no evidence".into());
    }
    let development: Vec<(String, String)> = gates
        .values()
        .filter(|g| text(g, "kind") == Some("development"))
        .flat_map(cited)
        .collect();
    let root = ledger_dir
        .join("evidence")
        .canonicalize()
        .map_err(|_| "the ledger's evidence directory is missing".to_owned())?;
    let mut last_reason = "no cited evidence is a writer-containment record".to_owned();
    for (relative, claimed) in &evidence {
        if !is_sha256(claimed) {
            return Err("the writer-containment gate records an invalid evidence hash".into());
        }
        if development
            .iter()
            .any(|(path, digest)| path == relative || digest.eq_ignore_ascii_case(claimed))
        {
            return Err(
                "the writer-containment evidence is also cited by a development gate".into(),
            );
        }
        let path = contained(ledger_dir, &root, relative)?;
        let data = read_bounded(&path, MAX_EVIDENCE_BYTES)?;
        let actual = hex(&Sha256::digest(&data));
        if !actual.eq_ignore_ascii_case(claimed) {
            return Err(format!(
                "the evidence {} changed since the ledger recorded it",
                name(&path)
            ));
        }
        let Ok(record) = serde_json::from_slice::<Value>(&data) else {
            continue;
        };
        if text(&record, "gate") != Some(WRITER_GATE) {
            continue;
        }
        match check_record(&record, agent, launch_digest, platform) {
            Ok(()) => return Ok(actual),
            Err(reason) => last_reason = reason,
        }
    }
    Err(last_reason)
}

/// `relative` must stay inside the evidence root (no absolute paths, no `..`, no links out).
fn contained(ledger_dir: &Path, root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("a cited evidence path is not a plain relative path".into());
    }
    let path = ledger_dir
        .join(relative)
        .canonicalize()
        .map_err(|_| "a cited evidence file is missing".to_owned())?;
    if !path.starts_with(root) {
        return Err("a cited evidence file escapes the evidence directory".into());
    }
    Ok(path)
}

fn check_record(
    record: &Value,
    agent: &str,
    launch_digest: &str,
    platform: &Platform,
) -> Result<(), String> {
    let fail = |what: &str| Err(format!("the writer-containment record {what}"));
    if text(record, "evidence_kind") != Some("authentic")
        || record.get("fixture_only") != Some(&Value::Bool(false))
        || text(record, "schema") != Some(EVIDENCE_SCHEMA)
    {
        return fail("is not authentic, non-fixture evidence");
    }
    let record_platform = record.get("platform");
    if record_platform.and_then(|p| text(p, "system")) != Some(platform.system.as_str())
        || record_platform.and_then(|p| text(p, "machine")) != Some(platform.machine.as_str())
    {
        return fail("was measured on another platform");
    }
    let adapter = record.get("adapter");
    if adapter.and_then(|a| text(a, "agent_name")) != Some(agent)
        || adapter.and_then(|a| text(a, "launch_identity")) != Some(launch_digest)
        || adapter.and_then(|a| a.get("protocol_version")) != Some(&Value::from(1))
    {
        return fail("belongs to another adapter launch");
    }
    if record.pointer("/cleanup/owned_processes_after") != Some(&Value::from(0)) {
        return fail("did not measure a clean teardown");
    }
    let scenarios = record
        .pointer("/measurements/scenarios")
        .and_then(Value::as_array)
        .ok_or("the writer-containment record has no scenarios")?;
    for required in REQUIRED_SCENARIOS {
        let measured = scenarios.iter().any(|s| {
            text(s, "name") == Some(required)
                && s.get("escaped_descendants") == Some(&Value::from(0))
                && s.get("group_empty_after") == Some(&Value::Bool(true))
        });
        if !measured {
            return Err(format!(
                "the writer-containment record did not measure a contained {required} scenario"
            ));
        }
    }
    Ok(())
}

/// A shareable resolver for launch-time re-validation (see
/// [`crate::agent_workflow::AdapterSettings::ownership_probe`]): every task launch derives
/// ownership again, so a replaced executable or ledger never keeps a stale qualification.
pub fn probe(
    file: AdapterFile,
    paths: AppPaths,
    policy: OwnershipPolicy,
) -> Arc<dyn Fn() -> WriterOwnership + Send + Sync> {
    Arc::new(move || resolve_ownership(&file, &paths, &policy).ownership)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type RecordEdit = fn(&mut Value);
    const DIGEST: &str = "819ef19d8b35aa9874b2025b70414b64535eed3d11429bfaf1f342e5fabbdb65";

    fn identity() -> LaunchIdentity {
        LaunchIdentity {
            executable_sha256: "aa".repeat(32),
            args: vec!["--acp".into(), "x y".into()],
            auth_env_names: vec!["B".into(), "A".into()],
            arg_files: vec![(0, "bb".repeat(32))],
        }
    }

    #[test]
    fn the_launch_digest_matches_the_harness_vector_and_binds_every_part() {
        // The same vector is computed by scripts/qualify-m3-agent.py.
        assert_eq!(identity().digest(), DIGEST);
        let changed = |f: &dyn Fn(&mut LaunchIdentity)| {
            let mut id = identity();
            f(&mut id);
            assert_ne!(id.digest(), DIGEST);
        };
        changed(&|id| id.executable_sha256 = "cc".repeat(32));
        changed(&|id| id.args.push("--more".into()));
        changed(&|id| id.args[1] = "x  y".into());
        changed(&|id| id.auth_env_names.push("C".into()));
        changed(&|id| id.arg_files[0].1 = "dd".repeat(32));
        changed(&|id| id.arg_files.clear());
        // Name order is not part of the identity.
        let mut reordered = identity();
        reordered.auth_env_names.reverse();
        assert_eq!(reordered.digest(), DIGEST);
        // Arguments cannot masquerade as the neighbouring list.
        let mut shifted = identity();
        shifted.args = vec!["--acp".into(), "x y".into(), "B".into()];
        shifted.auth_env_names = vec!["A".into()];
        assert_ne!(shifted.digest(), DIGEST);
    }

    fn linux() -> Platform {
        Platform {
            system: "Linux".into(),
            machine: "x86_64".into(),
        }
    }

    fn record(digest: &str) -> Value {
        json!({
            "evidence_kind": "authentic",
            "schema": EVIDENCE_SCHEMA,
            "gate": WRITER_GATE,
            "fixture_only": false,
            "platform": {"system": "Linux", "machine": "x86_64"},
            "adapter": {"agent_name": "real-adapter", "protocol_version": 1, "launch_identity": digest},
            "cleanup": {"owned_processes_after": 0},
            "measurements": {"scenarios": [
                {"name": "edit", "escaped_descendants": 0, "group_empty_after": true},
                {"name": "stop", "escaped_descendants": 0, "group_empty_after": true},
                {"name": "provider_crash", "escaped_descendants": 0, "group_empty_after": true}
            ]}
        })
    }

    fn ledger(digest: &str, evidence: &[(&str, &str)]) -> Value {
        json!({
            "kind": "m3",
            "schema_version": 1,
            "environment": {"adapter": {
                "agent_name": "real-adapter", "protocol_version": 1,
                "fixture_detected": false, "launch_identity": digest
            }},
            "gates": {
                "dev_workflow_fixture": {"kind": "development", "status": "pass",
                    "evidence": [{"path": "evidence/dev.log", "sha256": "0".repeat(64)}]},
                WRITER_GATE: {"kind": "authentic", "status": "pass",
                    "evidence": evidence.iter()
                        .map(|(p, h)| json!({"path": p, "sha256": h}))
                        .collect::<Vec<_>>()}
            }
        })
    }

    /// Writes a bundle (evidence record + ledger) and returns its directory.
    fn bundle(
        edit_record: impl FnOnce(&mut Value),
        edit_ledger: impl FnOnce(&mut Value),
    ) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("evidence")).unwrap();
        let mut rec = record(DIGEST);
        edit_record(&mut rec);
        let bytes = serde_json::to_vec(&rec).unwrap();
        std::fs::write(temp.path().join("evidence/writer.json"), &bytes).unwrap();
        let hash = hex(&Sha256::digest(&bytes));
        let mut led = ledger(DIGEST, &[("evidence/writer.json", &hash)]);
        edit_ledger(&mut led);
        std::fs::write(
            temp.path().join(LEDGER_FILE),
            serde_json::to_vec(&led).unwrap(),
        )
        .unwrap();
        temp
    }

    fn refused(dir: &tempfile::TempDir, digest: &str, platform: &Platform) -> String {
        let resolution = resolve_with(dir.path(), digest, platform);
        assert_eq!(
            resolution.ownership,
            WriterOwnership::Unknown,
            "{resolution:?}"
        );
        match resolution.containment {
            Containment::Unknown { reason } => reason,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn validated_hashed_authentic_evidence_for_this_launch_and_platform_qualifies() {
        let dir = bundle(|_| (), |_| ());
        let resolution = resolve_with(dir.path(), DIGEST, &linux());
        assert!(
            matches!(
                &resolution.ownership,
                WriterOwnership::ProcessGroupContained { qualification }
                    if qualification.starts_with("m3-ledger:auth_writer_process_group:")
            ),
            "{resolution:?}"
        );
        assert!(
            resolution
                .summary()
                .contains("qualified for this exact adapter")
        );
    }

    #[test]
    fn an_arbitrary_id_a_missing_ledger_and_a_garbage_ledger_never_qualify() {
        let empty = tempfile::tempdir().unwrap();
        assert!(refused(&empty, DIGEST, &linux()).contains("No installed qualification ledger"));
        let garbage = tempfile::tempdir().unwrap();
        std::fs::write(
            garbage.path().join(LEDGER_FILE),
            "{\"writer_qualification\":\"x\"}",
        )
        .unwrap();
        assert!(refused(&garbage, DIGEST, &linux()).contains("not an M3 ledger"));
        std::fs::write(garbage.path().join(LEDGER_FILE), "not json").unwrap();
        assert!(refused(&garbage, DIGEST, &linux()).contains("not valid JSON"));
    }

    #[test]
    fn a_gate_that_did_not_pass_or_is_not_authentic_never_qualifies() {
        for status in ["not_run", "blocked", "fail"] {
            let dir = bundle(
                |_| (),
                |l| l["gates"][WRITER_GATE]["status"] = json!(status),
            );
            assert!(
                refused(&dir, DIGEST, &linux()).contains("has not passed"),
                "{status}"
            );
        }
        let dir = bundle(
            |_| (),
            |l| l["gates"][WRITER_GATE]["kind"] = json!("development"),
        );
        assert!(refused(&dir, DIGEST, &linux()).contains("has not passed"));
        let dir = bundle(|_| (), |l| l["gates"][WRITER_GATE]["evidence"] = json!([]));
        assert!(refused(&dir, DIGEST, &linux()).contains("cites no evidence"));
    }

    #[test]
    fn fixture_and_development_evidence_never_qualifies() {
        let cases: [(&str, RecordEdit); 4] = [
            ("fixture_only", |r| r["fixture_only"] = json!(true)),
            ("marker absent", |r| {
                r.as_object_mut().unwrap().remove("fixture_only");
            }),
            ("development kind", |r| {
                r["evidence_kind"] = json!("development")
            }),
            ("wrong schema", |r| r["schema"] = json!("other/1")),
        ];
        for (label, edit) in cases {
            let dir = bundle(edit, |_| ());
            assert!(
                refused(&dir, DIGEST, &linux()).contains("not authentic"),
                "{label}"
            );
        }
        for (field, value) in [
            ("agent_name", json!("scripted-agent")),
            ("protocol_version", json!(2)),
            ("fixture_detected", json!(true)),
        ] {
            let dir = bundle(
                |_| (),
                |l| l["environment"]["adapter"][field] = value.clone(),
            );
            let reason = refused(&dir, DIGEST, &linux());
            assert!(
                reason.contains("fixture") || reason.contains("protocol-v1"),
                "{field}: {reason}"
            );
        }
        let dir = bundle(|_| (), |l| l["environment"]["adapter"] = Value::Null);
        assert!(refused(&dir, DIGEST, &linux()).contains("no authentic adapter"));
    }

    #[test]
    fn a_changed_executable_or_arguments_or_variables_leave_ownership_unknown() {
        let dir = bundle(|_| (), |_| ());
        for other in [
            LaunchIdentity {
                executable_sha256: "cc".repeat(32),
                ..identity()
            },
            LaunchIdentity {
                args: vec!["--acp".into()],
                ..identity()
            },
            LaunchIdentity {
                auth_env_names: vec!["A".into()],
                ..identity()
            },
        ] {
            let reason = refused(&dir, &other.digest(), &linux());
            assert!(reason.contains("not this launch"), "{reason}");
        }
        // The record itself bound to another launch is refused even if the ledger agrees.
        let dir = bundle(
            |r| r["adapter"]["launch_identity"] = json!("0".repeat(64)),
            |_| (),
        );
        assert!(refused(&dir, DIGEST, &linux()).contains("another adapter launch"));
    }

    #[test]
    fn another_platform_never_qualifies() {
        let dir = bundle(|_| (), |_| ());
        for platform in [
            Platform {
                system: "Windows".into(),
                machine: "AMD64".into(),
            },
            Platform {
                system: "Darwin".into(),
                machine: "arm64".into(),
            },
            Platform {
                system: "Linux".into(),
                machine: "aarch64".into(),
            },
        ] {
            assert!(refused(&dir, DIGEST, &platform).contains("another platform"));
        }
    }

    #[test]
    fn a_hash_mismatch_or_an_escaping_path_never_qualifies() {
        let dir = bundle(|_| (), |_| ());
        // Tamper with the cited file after the ledger recorded its hash.
        let path = dir.path().join("evidence/writer.json");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(b' ');
        std::fs::write(&path, bytes).unwrap();
        assert!(refused(&dir, DIGEST, &linux()).contains("changed since the ledger"));
        for bad in ["../writer.json", "/etc/hostname", "evidence/../writer.json"] {
            let dir = bundle(
                |_| (),
                |l| l["gates"][WRITER_GATE]["evidence"][0]["path"] = json!(bad),
            );
            assert!(
                refused(&dir, DIGEST, &linux()).contains("plain relative path"),
                "{bad}"
            );
        }
        let dir = bundle(
            |_| (),
            |l| l["gates"][WRITER_GATE]["evidence"][0]["sha256"] = json!("zz"),
        );
        assert!(refused(&dir, DIGEST, &linux()).contains("invalid evidence hash"));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_evidence_directory_never_qualifies() {
        let dir = bundle(|_| (), |_| ());
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("writer.json");
        std::fs::copy(dir.path().join("evidence/writer.json"), &target).unwrap();
        std::fs::remove_file(dir.path().join("evidence/writer.json")).unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("evidence/writer.json")).unwrap();
        assert!(refused(&dir, DIGEST, &linux()).contains("escapes the evidence directory"));
    }

    #[test]
    fn evidence_that_a_development_gate_also_cites_never_qualifies() {
        let dir = bundle(
            |_| (),
            |l| {
                let evidence = l["gates"][WRITER_GATE]["evidence"].clone();
                l["gates"]["dev_workflow_fixture"]["evidence"] = evidence;
            },
        );
        assert!(refused(&dir, DIGEST, &linux()).contains("development gate"));
    }

    #[test]
    fn missing_or_unclean_containment_measurements_never_qualify() {
        let cases: [(&str, RecordEdit, &str); 4] = [
            (
                "escaped writer",
                |r| r["measurements"]["scenarios"][1]["escaped_descendants"] = json!(1),
                "stop",
            ),
            (
                "group not empty",
                |r| r["measurements"]["scenarios"][0]["group_empty_after"] = json!(false),
                "edit",
            ),
            (
                "scenario missing",
                |r| {
                    r["measurements"]["scenarios"].as_array_mut().unwrap().pop();
                },
                "provider_crash",
            ),
            (
                "owned processes left",
                |r| r["cleanup"]["owned_processes_after"] = json!(2),
                "clean teardown",
            ),
        ];
        for (label, edit, needle) in cases {
            let dir = bundle(edit, |_| ());
            let reason = refused(&dir, DIGEST, &linux());
            assert!(reason.contains(needle), "{label}: {reason}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_executable_loses_its_qualification_end_to_end() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let executable = temp.path().join("adapter.sh");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = AdapterFile {
            provider: "real".into(),
            executable: executable.to_string_lossy().into_owned(),
            args: vec!["--acp".into()],
            auth_env_names: vec!["PROVIDER_API_KEY".into()],
            auth_method: None,
            mcp: super::super::host::McpChoice::Baseline,
        };
        let digest = launch_identity(&file, &paths).unwrap().digest();
        // Install the bundle where the app looks for it.
        let installed = paths.data.join(LEDGER_DIR);
        let source = bundle(
            |r| r["adapter"]["launch_identity"] = json!(digest),
            |l| {
                l["environment"]["adapter"]["launch_identity"] = json!(digest);
            },
        );
        std::fs::create_dir_all(installed.join("evidence")).unwrap();
        for relative in [LEDGER_FILE, "evidence/writer.json"] {
            std::fs::copy(source.path().join(relative), installed.join(relative)).unwrap();
        }
        let platform = Platform::current();
        // The fixture's platform is Linux/x86_64; qualification is only offered on it.
        if platform == linux() {
            let resolution = resolve_ownership(&file, &paths, &OwnershipPolicy::Validated);
            assert!(resolution.ownership.is_qualified(), "{resolution:?}");
            // The adapter is replaced: the very next derivation (every launch) says unknown.
            std::fs::write(&executable, "#!/bin/sh\necho replaced\n").unwrap();
            let again = resolve_ownership(&file, &paths, &OwnershipPolicy::Validated);
            assert_eq!(again.ownership, WriterOwnership::Unknown);
            let probe = probe(file.clone(), paths.clone(), OwnershipPolicy::Validated);
            assert_eq!(probe(), WriterOwnership::Unknown);
        }
        // Test injection is the only other route, and it is labelled.
        let injected = resolve_ownership(
            &file,
            &paths,
            &OwnershipPolicy::TestInjected(WriterOwnership::ProcessGroupContained {
                qualification: "test".into(),
            }),
        );
        assert!(matches!(
            injected.containment,
            Containment::TestInjected { .. }
        ));
    }
}
