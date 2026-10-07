//! Runs real scratch probes using Studio's actual `probe_adapter`, `AdapterLaunch::resolve`,
//! and `AppPaths`, writing genuine probe evidence to `desktop/qualification/evidence/m6-probes/`.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use studio_agent_spike::{
    AdapterConfig,
    discovery::{AdapterStatus, ExecutableSearch, ProbeOptions, probe_adapter},
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::app_paths::AppPaths;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parent of desktop/app")
        .parent()
        .expect("workspace root")
        .to_path_buf();
    let qualification_dir = repo_root.join("desktop").join("qualification");
    let evidence_dir = qualification_dir.join("evidence").join("m6-probes");
    std::fs::create_dir_all(&evidence_dir)?;

    let app_paths = AppPaths::system()?;
    let managed_adapters_dir = app_paths.data.join("adapters");
    std::fs::create_dir_all(&managed_adapters_dir)?;

    let temp = tempfile::tempdir()?;
    let scratch_cwd = temp.path().join("scratch");
    std::fs::create_dir_all(&scratch_cwd)?;

    let search = ExecutableSearch {
        managed_dirs: vec![managed_adapters_dir.clone()],
        gui_path: None,
    };
    let options = ProbeOptions::new(scratch_cwd);
    let processes = ProcessTreeManager::new();

    let providers = [
        (
            "claude",
            "Claude",
            "claude-agent-acp",
            vec!["ANTHROPIC_API_KEY".to_string()],
        ),
        (
            "codex",
            "Codex",
            "codex-acp",
            vec!["OPENAI_API_KEY".to_string(), "CODEX_API_KEY".to_string()],
        ),
        ("pi", "Pi", "pi-acp", vec![]),
        (
            "antigravity",
            "Antigravity",
            "antigravity-acp",
            vec!["GEMINI_API_KEY".to_string(), "GOOGLE_API_KEY".to_string()],
        ),
    ];

    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let mut probe_results = Vec::new();

    for (id, label, exe, auth) in providers {
        let config = AdapterConfig {
            executable: exe.to_string(),
            args: vec![],
            auth_env_names: auth,
        };
        let report = probe_adapter(&config, &search, &options, &processes);
        let (status_str, detail_str, runtime_available, searched_paths) = match &report.status {
            AdapterStatus::MissingExecutable { searched } => (
                "missing_executable",
                format!(
                    "Adapter executable '{exe}' not found in managed search directories or configured path."
                ),
                None,
                searched.clone(),
            ),
            AdapterStatus::MissingRuntime { detail } => {
                ("missing_runtime", detail.clone(), Some(false), vec![])
            }
            AdapterStatus::ProtocolMismatch { detail } => {
                ("protocol_mismatch", detail.clone(), Some(true), vec![])
            }
            AdapterStatus::AuthRequired { methods } => (
                "auth_required",
                format!("Authentication required: {}", methods.join(", ")),
                Some(true),
                vec![],
            ),
            AdapterStatus::AuthUnknown { methods } => (
                "auth_unknown",
                format!("Authentication unknown: {}", methods.join(", ")),
                Some(true),
                vec![],
            ),
            AdapterStatus::AuthRejected { detail } => {
                ("auth_rejected", detail.clone(), Some(true), vec![])
            }
            AdapterStatus::Ready { agent, version } => (
                "ready",
                format!("Ready: {agent} {version}"),
                Some(true),
                vec![],
            ),
            AdapterStatus::Failed { failure } => ("failed", failure.message.clone(), None, vec![]),
        };

        // Redact any absolute local home/temporary paths to portable identifiers
        let sanitized_searched: Vec<String> = searched_paths
            .into_iter()
            .map(|p| {
                if p.contains("adapters") {
                    format!("adapters/{exe}")
                } else {
                    exe.to_string()
                }
            })
            .collect();

        let evidence_record = json!({
            "schema": "m6-probe/1",
            "evidence_kind": "probe",
            "provider": id,
            "observed_unix_seconds": unix_secs,
            "status": status_str,
            "detail": detail_str,
            "runtime_available": runtime_available,
            "searched": sanitized_searched,
            "probe_options": {
                "verify_session": true,
                "timeout_ms": 30000
            }
        });

        let file_name = format!("probe-{id}.json");
        let evidence_path = evidence_dir.join(&file_name);
        let raw_bytes = serde_json::to_vec_pretty(&evidence_record)?;
        std::fs::write(&evidence_path, &raw_bytes)?;

        let mut hasher = Sha256::new();
        hasher.update(&raw_bytes);
        let sha256 = format!("{:x}", hasher.finalize());

        println!(
            "Probe {label}: status={status_str}, runtime_available={runtime_available:?} -> evidence/m6-probes/{file_name} ({sha256})"
        );

        probe_results.push((
            id,
            status_str,
            detail_str,
            runtime_available,
            sanitized_searched,
            format!("evidence/m6-probes/{file_name}"),
            sha256,
        ));
    }

    // Update m6-results.json with genuine observed probe observations
    let ledger_path = qualification_dir.join("m6-results.json");
    let mut ledger: Value = serde_json::from_str(&std::fs::read_to_string(&ledger_path)?)?;

    for (id, status_str, detail_str, runtime_avail, searched, ev_rel, sha256) in probe_results {
        if let Some(prov) = ledger.pointer_mut(&format!("/providers/{id}")) {
            prov["probe_observation"] = json!({
                "status": status_str,
                "detail": detail_str,
                "runtime_available": runtime_avail,
                "observed_unix_seconds": unix_secs,
                "searched": searched,
                "evidence": {
                    "path": ev_rel,
                    "sha256": sha256
                }
            });
            prov["capabilities"] = Value::Null;
        }
    }

    std::fs::write(&ledger_path, serde_json::to_string_pretty(&ledger)? + "\n")?;
    println!("Updated m6-results.json with authentic scratch probe evidence.");

    Ok(())
}
