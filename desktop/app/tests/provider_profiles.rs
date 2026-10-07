//! Integration tests for provider profiles, migration, and qualification resolution.

use fframes_studio::conversation_panel::{
    host::{self, AdapterFile, McpChoice, REGISTRY_BACKUP_SUFFIX, REGISTRY_FILE, SETTINGS_FILE},
    provider_profiles::{
        BUILTIN_ANTIGRAVITY_ID, BUILTIN_CLAUDE_ID, BUILTIN_CODEX_ID, BUILTIN_PI_ID,
        ProviderProfile, ProviderQualificationStatus, ProviderRegistry, QualificationRankingStatus,
        QualificationSnapshot, REGISTRY_SCHEMA_VERSION,
    },
    qualification::{self, Containment, Platform},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use studio_agent_spike::{
    AdapterConfig,
    discovery::{AdapterStatus, ExecutableSearch, ProbeOptions, probe_adapter},
};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};
use studio_engine::app_paths::AppPaths;
use tempfile::TempDir;

fn setup_paths() -> (TempDir, AppPaths) {
    let temp = TempDir::new().unwrap();
    let paths = AppPaths::new(temp.path().join("data")).unwrap();
    (temp, paths)
}

fn write_evidence_file(
    dir: &std::path::Path,
    name: &str,
    content: &serde_json::Value,
) -> (String, String) {
    let ev_dir = dir.join("evidence");
    std::fs::create_dir_all(&ev_dir).unwrap();
    let file = ev_dir.join(name);
    let bytes = serde_json::to_vec(content).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = format!("{:x}", hasher.finalize());
    (format!("evidence/{name}"), hash)
}

#[test]
fn default_registry_contains_four_profiles_and_starts_unselected() {
    let registry = ProviderRegistry::default();
    assert_eq!(registry.schema_version, REGISTRY_SCHEMA_VERSION);
    assert_eq!(registry.selected_profile_id, None);
    assert_eq!(registry.profiles.len(), 4);

    let ids: Vec<&str> = registry.profiles.iter().map(|p| p.id.as_str()).collect();
    assert!(ids.contains(&BUILTIN_CLAUDE_ID));
    assert!(ids.contains(&BUILTIN_CODEX_ID));
    assert!(ids.contains(&BUILTIN_PI_ID));
    assert!(ids.contains(&BUILTIN_ANTIGRAVITY_ID));

    for profile in &registry.profiles {
        assert!(profile.builtin, "all default profiles must be builtin");
        assert!(
            profile.experimental,
            "all default profiles start experimental"
        );
    }

    let pi = registry
        .profiles
        .iter()
        .find(|p| p.id == BUILTIN_PI_ID)
        .unwrap();
    assert!(
        pi.adapter.auth_env_names.is_empty(),
        "Pi uses CLI login, no env credentials"
    );
    assert_eq!(
        pi.adapter.mcp,
        McpChoice::Unsupported,
        "Pi uses CLI tool route only"
    );
}

#[test]
fn qualification_snapshot_displays_per_provider_ledger_status_and_ranking() {
    let snapshot =
        QualificationSnapshot::parse(include_str!("../../qualification/m6-results.json")).unwrap();

    assert_eq!(
        snapshot.ranking,
        QualificationRankingStatus::InsufficientEvidence
    );
    assert_eq!(
        snapshot.status_for(BUILTIN_CLAUDE_ID),
        ProviderQualificationStatus::NotRun
    );
    assert!(snapshot.recommended.is_empty());
}

#[test]
fn qualification_snapshot_displays_distinct_qualified_and_blocked_provider_states() {
    let mut ledger: serde_json::Value =
        serde_json::from_str(include_str!("../../qualification/m6-results.json")).unwrap();
    ledger["providers"]["claude"]["status"] = json!("qualified");
    ledger["providers"]["codex"]["status"] = json!("qualified");
    ledger["providers"]["pi"]["status"] = json!("blocked");
    ledger["providers"]["antigravity"]["status"] = json!("experimental");
    ledger["ranking"]["status"] = json!("recommended");
    ledger["ranking"]["recommended"] = json!(["claude", "codex"]);

    let snapshot = QualificationSnapshot::parse(&ledger.to_string()).unwrap();

    assert_eq!(snapshot.ranking, QualificationRankingStatus::Recommended);
    assert_eq!(
        snapshot.status_for(BUILTIN_CLAUDE_ID),
        ProviderQualificationStatus::Qualified
    );
    assert_eq!(
        snapshot.status_for(BUILTIN_PI_ID),
        ProviderQualificationStatus::Blocked
    );
    assert_eq!(
        snapshot.status_for(BUILTIN_ANTIGRAVITY_ID),
        ProviderQualificationStatus::Experimental
    );
    assert_eq!(snapshot.recommended, ["claude", "codex"]);
}

#[test]
fn qualification_snapshot_refuses_inconsistent_or_unqualified_recommendations() {
    let mut ledger: serde_json::Value =
        serde_json::from_str(include_str!("../../qualification/m6-results.json")).unwrap();
    ledger["ranking"]["status"] = json!("recommended");
    ledger["ranking"]["recommended"] = json!(["claude", "codex"]);

    assert!(QualificationSnapshot::parse(&ledger.to_string()).is_err());
}

#[test]
fn registry_save_load_round_trip_re_derives_builtin_and_experimental_flags() {
    let (_temp, paths) = setup_paths();
    let mut registry = ProviderRegistry::default();
    registry.select_profile(BUILTIN_CLAUDE_ID).unwrap();
    registry.save_custom_adapter(AdapterFile {
        provider: "My Custom Service".into(),
        executable: "/opt/custom".into(),
        args: vec![],
        auth_env_names: vec![],
        auth_method: None,
        mcp: McpChoice::Baseline,
    });
    host::save_registry(&paths, &registry).unwrap();

    let loaded = host::load_registry(&paths)
        .unwrap()
        .expect("loaded registry");
    let claude = loaded
        .profiles
        .iter()
        .find(|p| p.id == BUILTIN_CLAUDE_ID)
        .unwrap();
    assert!(claude.builtin, "builtin flag must be true after reload");
    assert!(
        claude.experimental,
        "experimental flag must be true after reload"
    );

    let custom = loaded
        .profiles
        .iter()
        .find(|p| p.id == "my-custom-service")
        .unwrap();
    assert!(
        !custom.builtin,
        "custom profile must have builtin: false after reload"
    );
    assert!(
        custom.experimental,
        "custom profile must have experimental: true after reload"
    );
}

#[test]
fn migration_from_custom_adapter_preserves_all_fields_and_selects_it() {
    let (_temp, paths) = setup_paths();

    let legacy = AdapterFile {
        provider: "My Custom Agent".into(),
        executable: "/opt/custom/agent".into(),
        args: vec!["--acp".into(), "--fast".into()],
        auth_env_names: vec!["CUSTOM_TOKEN".into(), "CUSTOM_API_KEY".into()],
        auth_method: Some("custom-method".into()),
        mcp: McpChoice::Unsupported,
    };
    std::fs::write(paths.data.join(SETTINGS_FILE), legacy.to_pretty()).unwrap();

    let registry = host::load_registry(&paths)
        .unwrap()
        .expect("migrated registry");
    assert_eq!(registry.schema_version, 1);
    assert_eq!(
        registry.selected_profile_id,
        Some("my-custom-agent".to_string())
    );

    let profile = registry
        .selected_profile()
        .expect("selected profile exists");
    assert_eq!(profile.label, "My Custom Agent");
    assert_eq!(profile.adapter.executable, "/opt/custom/agent");
    assert_eq!(profile.adapter.args, vec!["--acp", "--fast"]);
    assert_eq!(
        profile.adapter.auth_env_names,
        vec!["CUSTOM_TOKEN", "CUSTOM_API_KEY"]
    );
    assert_eq!(
        profile.adapter.auth_method.as_deref(),
        Some("custom-method")
    );
    assert_eq!(profile.adapter.mcp, McpChoice::Unsupported);

    // Rollback backup copy must exist and old file must remain intact
    let backup_path = paths
        .data
        .join(format!("{SETTINGS_FILE}{REGISTRY_BACKUP_SUFFIX}"));
    assert!(backup_path.exists(), "rollback backup must be created");
    assert!(
        paths.data.join(SETTINGS_FILE).exists(),
        "legacy file remains intact"
    );
    assert!(
        paths.data.join(REGISTRY_FILE).exists(),
        "new registry file created"
    );
}

#[test]
fn migration_matches_builtin_provider_by_name() {
    let (_temp, paths) = setup_paths();

    let legacy = AdapterFile {
        provider: "Codex".into(),
        executable: "/custom/path/codex-acp".into(),
        args: vec!["--port".into(), "9000".into()],
        auth_env_names: vec!["OPENAI_API_KEY".into()],
        auth_method: None,
        mcp: McpChoice::Baseline,
    };
    std::fs::write(paths.data.join(SETTINGS_FILE), legacy.to_pretty()).unwrap();

    let registry = host::load_registry(&paths)
        .unwrap()
        .expect("migrated registry");
    assert_eq!(
        registry.selected_profile_id,
        Some(BUILTIN_CODEX_ID.to_string())
    );

    let profile = registry.selected_profile().unwrap();
    assert_eq!(profile.id, BUILTIN_CODEX_ID);
    assert_eq!(profile.adapter.executable, "/custom/path/codex-acp");
    assert_eq!(profile.adapter.args, vec!["--port", "9000"]);
}

#[test]
fn validation_rejects_duplicate_ids() {
    let mut registry = ProviderRegistry::default();
    registry.profiles.push(ProviderProfile {
        id: BUILTIN_CLAUDE_ID.into(),
        label: "Duplicate Claude".into(),
        description: "".into(),
        adapter: AdapterFile {
            provider: "Dup".into(),
            executable: "/bin/true".into(),
            args: vec![],
            auth_env_names: vec![],
            auth_method: None,
            mcp: McpChoice::Baseline,
        },
        builtin: false,
        experimental: true,
    });
    assert!(registry.validate().is_err());
}

#[test]
fn validation_rejects_newer_schema_version_as_diagnostic_only() {
    let registry = ProviderRegistry {
        schema_version: 999,
        ..Default::default()
    };
    let err = registry.validate().unwrap_err();
    assert!(err.contains("diagnostic-only"));
}

#[test]
fn validation_rejects_credentials_in_auth_env_names() {
    let mut registry = ProviderRegistry::default();
    registry.profiles[0].adapter.auth_env_names = vec!["sk-ant-api03-secret1234567890".into()];
    assert!(registry.validate().is_err());
}

#[test]
fn save_registry_does_not_modify_legacy_agent_adapter_json() {
    let (_temp, paths) = setup_paths();

    let original_legacy = AdapterFile {
        provider: "Original".into(),
        executable: "/bin/orig".into(),
        args: vec![],
        auth_env_names: vec![],
        auth_method: None,
        mcp: McpChoice::Baseline,
    };
    let legacy_bytes = original_legacy.to_pretty();
    std::fs::write(paths.data.join(SETTINGS_FILE), &legacy_bytes).unwrap();

    let mut registry = ProviderRegistry::default();
    registry.select_profile(BUILTIN_CODEX_ID).unwrap();
    host::save_registry(&paths, &registry).unwrap();

    // Verify agent-adapter.json is strictly untouched!
    let legacy_after = std::fs::read_to_string(paths.data.join(SETTINGS_FILE)).unwrap();
    assert_eq!(
        legacy_after, legacy_bytes,
        "legacy file must remain strictly untouched"
    );
}

#[test]
fn save_settings_updates_registry_without_clobbering_builtin_defaults() {
    let (_temp, paths) = setup_paths();

    let registry = ProviderRegistry::default();
    host::save_registry(&paths, &registry).unwrap();

    // Saving a custom adapter configuration
    let custom_adapter = AdapterFile {
        provider: "My Custom Service".into(),
        executable: "/opt/custom/exec".into(),
        args: vec!["--flag".into()],
        auth_env_names: vec!["CUSTOM_KEY".into()],
        auth_method: None,
        mcp: McpChoice::Baseline,
    };
    host::save_settings(&paths, &custom_adapter).unwrap();

    let loaded_reg = host::load_registry(&paths).unwrap().unwrap();
    // Builtin Claude defaults must remain untouched!
    let claude = loaded_reg
        .profiles
        .iter()
        .find(|p| p.id == BUILTIN_CLAUDE_ID)
        .unwrap();
    assert_eq!(claude.adapter.executable, "claude-agent-acp");

    // The custom profile was added and selected
    let selected = loaded_reg.selected_profile().unwrap();
    assert_eq!(selected.label, "My Custom Service");
    assert_eq!(selected.adapter.executable, "/opt/custom/exec");
}

#[test]
fn scratch_probe_records_authentic_prerequisites_for_all_four_providers() {
    let (temp, _paths) = setup_paths();
    let scratch_dir = temp.path().join("scratch");
    std::fs::create_dir_all(&scratch_dir).unwrap();

    let search = ExecutableSearch {
        managed_dirs: vec![temp.path().join("adapters")],
        gui_path: None,
    };
    let options = ProbeOptions::new(scratch_dir);
    let processes = ProcessTreeManager::new();

    let providers = [
        (
            "Claude",
            "claude-agent-acp",
            vec!["ANTHROPIC_API_KEY".to_string()],
        ),
        (
            "Codex",
            "codex-acp",
            vec!["OPENAI_API_KEY".to_string(), "CODEX_API_KEY".to_string()],
        ),
        ("Pi", "pi-acp", vec!["PI_API_KEY".to_string()]),
        (
            "Antigravity",
            "antigravity-acp",
            vec!["GEMINI_API_KEY".to_string(), "GOOGLE_API_KEY".to_string()],
        ),
    ];

    for (name, exe, auth) in providers {
        let config = AdapterConfig {
            executable: exe.to_string(),
            args: vec![],
            auth_env_names: auth,
        };
        let report = probe_adapter(&config, &search, &options, &processes);
        match report.status {
            AdapterStatus::MissingExecutable { searched } => {
                assert!(
                    !searched.is_empty(),
                    "{name} probe must record searched paths"
                );
            }
            other => panic!("{name} unexpected probe status: {other:?}"),
        }
    }
}

#[test]
fn qualification_resolution_grants_m6_containment_with_authentic_evidence() {
    let (_temp, paths) = setup_paths();
    let ledger_dir = paths.data.join(qualification::LEDGER_DIR);
    std::fs::create_dir_all(&ledger_dir).unwrap();

    let digest = "44".repeat(32);
    let platform = Platform {
        system: "Linux".into(),
        machine: "x86_64".into(),
    };

    let evidence_record = json!({
        "evidence_kind": "authentic",
        "schema": "m3-authentic/1",
        "gate": "auth_writer_process_group",
        "fixture_only": false,
        "platform": {"system": "Linux", "machine": "x86_64"},
        "adapter": {"agent_name": "claude-agent-acp", "protocol_version": 1, "launch_identity": digest},
        "cleanup": {"owned_processes_after": 0},
        "measurements": {"scenarios": [
            {"name": "edit", "escaped_descendants": 0, "group_empty_after": true},
            {"name": "stop", "escaped_descendants": 0, "group_empty_after": true},
            {"name": "provider_crash", "escaped_descendants": 0, "group_empty_after": true}
        ]}
    });
    let (ev_path, ev_hash) = write_evidence_file(&ledger_dir, "ev_claude.json", &evidence_record);

    let m6_content = json!({
        "kind": "m6",
        "schema_version": 1,
        "timestamp": "2026-10-07T07:12:00Z",
        "environment": {
            "scope": "test",
            "os": "linux",
            "arch": "x86_64",
            "sdk_id": "test",
            "sdk_manifest_sha256": "0".repeat(64)
        },
        "providers": {
            "claude": {
                "id": "claude",
                "label": "Claude",
                "distribution": "claude-agent-acp",
                "launch_identity": digest,
                "status": "qualified",
                "gates": {
                    "interruption_and_cleanup": {
                        "kind": "authentic",
                        "status": "pass",
                        "criteria": "clean teardown",
                        "notes": "passed",
                        "evidence": [{"path": ev_path, "sha256": ev_hash}]
                    }
                }
            }
        },
        "ranking": {
            "status": "insufficient_evidence",
            "recommended": [],
            "experimental": ["claude"],
            "notes": "testing"
        },
        "acceptance": {
            "development": "not_run",
            "authentic": "not_run",
            "full": "not_run",
            "reason": "testing"
        }
    });
    std::fs::write(
        ledger_dir.join(qualification::LEDGER_FILE_M6),
        m6_content.to_string(),
    )
    .unwrap();

    let resolution = qualification::resolve_with(&ledger_dir, &digest, &platform);
    match resolution.ownership {
        WriterOwnership::ProcessGroupContained { qualification } => {
            assert!(qualification.starts_with("m6-ledger:interruption_and_cleanup:"));
        }
        _ => panic!(
            "Expected M6 ProcessGroupContained, got {:?}",
            resolution.ownership
        ),
    }
    match resolution.containment {
        Containment::Qualified { evidence_sha256 } => {
            assert_eq!(evidence_sha256, ev_hash);
        }
        _ => panic!("Expected Containment::Qualified"),
    }
}

#[test]
fn qualification_resolution_strictly_refuses_unqualified_m6_launch_without_m3_fallback() {
    let (_temp, paths) = setup_paths();
    let ledger_dir = paths.data.join(qualification::LEDGER_DIR);
    std::fs::create_dir_all(&ledger_dir).unwrap();

    let digest = "55".repeat(32);
    let platform = Platform {
        system: "Linux".into(),
        machine: "x86_64".into(),
    };

    // 1. Create a passing legacy M3 ledger for this exact digest
    let m3_evidence = json!({
        "evidence_kind": "authentic",
        "schema": "m3-authentic/1",
        "gate": "auth_writer_process_group",
        "fixture_only": false,
        "platform": {"system": "Linux", "machine": "x86_64"},
        "adapter": {"agent_name": "claude-legacy", "protocol_version": 1, "launch_identity": digest, "fixture_detected": false},
        "cleanup": {"owned_processes_after": 0},
        "measurements": {"scenarios": [
            {"name": "edit", "escaped_descendants": 0, "group_empty_after": true},
            {"name": "stop", "escaped_descendants": 0, "group_empty_after": true},
            {"name": "provider_crash", "escaped_descendants": 0, "group_empty_after": true}
        ]}
    });
    let (ev_m3_path, ev_m3_hash) = write_evidence_file(&ledger_dir, "ev_m3.json", &m3_evidence);
    let m3_content = json!({
        "kind": "m3",
        "schema_version": 1,
        "environment": {
            "adapter": {
                "agent_name": "claude-legacy",
                "protocol_version": 1,
                "fixture_detected": false,
                "launch_identity": digest
            }
        },
        "gates": {
            "auth_writer_process_group": {
                "kind": "authentic",
                "status": "pass",
                "evidence": [{"path": ev_m3_path, "sha256": ev_m3_hash}]
            }
        }
    });
    std::fs::write(
        ledger_dir.join(qualification::LEDGER_FILE),
        m3_content.to_string(),
    )
    .unwrap();

    // 2. Create M6 ledger where this launch is present but UNQUALIFIED (status: not_run)
    let m6_content = json!({
        "kind": "m6",
        "schema_version": 1,
        "timestamp": "2026-10-07T07:12:00Z",
        "environment": {
            "scope": "test",
            "os": "linux",
            "arch": "x86_64",
            "sdk_id": "test",
            "sdk_manifest_sha256": "0".repeat(64)
        },
        "providers": {
            "claude": {
                "id": "claude",
                "label": "Claude",
                "distribution": "claude-agent-acp",
                "launch_identity": digest,
                "status": "not_run",
                "gates": {
                    "interruption_and_cleanup": {
                        "kind": "authentic",
                        "status": "not_run",
                        "criteria": "clean teardown",
                        "notes": "not run",
                        "prerequisite": "requires credentials"
                    }
                }
            }
        },
        "ranking": {
            "status": "insufficient_evidence",
            "recommended": [],
            "experimental": ["claude"],
            "notes": "none"
        },
        "acceptance": {
            "development": "not_run",
            "authentic": "not_run",
            "full": "not_run",
            "reason": "none"
        }
    });
    std::fs::write(
        ledger_dir.join(qualification::LEDGER_FILE_M6),
        m6_content.to_string(),
    )
    .unwrap();

    // The resolution MUST be Unknown: M6 strictly refuses and MUST NEVER fall back to the passing M3 ledger!
    let resolution = qualification::resolve_with(&ledger_dir, &digest, &platform);
    assert_eq!(
        resolution.ownership,
        WriterOwnership::Unknown,
        "Unqualified M6 provider must be refused, not fall back to M3!"
    );
    match resolution.containment {
        Containment::Unknown { reason } => {
            assert!(reason.contains("has not passed qualification"));
        }
        _ => panic!("Expected Containment::Unknown"),
    }
}

#[test]
fn qualification_resolution_falls_back_to_m3_when_launch_not_in_m6() {
    let (_temp, paths) = setup_paths();
    let ledger_dir = paths.data.join(qualification::LEDGER_DIR);
    std::fs::create_dir_all(&ledger_dir).unwrap();

    let digest = "66".repeat(32);
    let platform = Platform {
        system: "Linux".into(),
        machine: "x86_64".into(),
    };

    // 1. Create a passing legacy M3 ledger for this exact digest
    let m3_evidence = json!({
        "evidence_kind": "authentic",
        "schema": "m3-authentic/1",
        "gate": "auth_writer_process_group",
        "fixture_only": false,
        "platform": {"system": "Linux", "machine": "x86_64"},
        "adapter": {"agent_name": "legacy-agent", "protocol_version": 1, "launch_identity": digest, "fixture_detected": false},
        "cleanup": {"owned_processes_after": 0},
        "measurements": {"scenarios": [
            {"name": "edit", "escaped_descendants": 0, "group_empty_after": true},
            {"name": "stop", "escaped_descendants": 0, "group_empty_after": true},
            {"name": "provider_crash", "escaped_descendants": 0, "group_empty_after": true}
        ]}
    });
    let (ev_m3_path, ev_m3_hash) =
        write_evidence_file(&ledger_dir, "ev_m3_legacy.json", &m3_evidence);
    let m3_content = json!({
        "kind": "m3",
        "schema_version": 1,
        "environment": {
            "adapter": {
                "agent_name": "legacy-agent",
                "protocol_version": 1,
                "fixture_detected": false,
                "launch_identity": digest
            }
        },
        "gates": {
            "auth_writer_process_group": {
                "kind": "authentic",
                "status": "pass",
                "evidence": [{"path": ev_m3_path, "sha256": ev_m3_hash}]
            }
        }
    });
    std::fs::write(
        ledger_dir.join(qualification::LEDGER_FILE),
        m3_content.to_string(),
    )
    .unwrap();

    // 2. Create M6 ledger that DOES NOT have this digest at all (only has claude with different digest)
    let m6_content = json!({
        "kind": "m6",
        "schema_version": 1,
        "timestamp": "2026-10-07T07:12:00Z",
        "environment": {
            "scope": "test",
            "os": "linux",
            "arch": "x86_64",
            "sdk_id": "test",
            "sdk_manifest_sha256": "0".repeat(64)
        },
        "providers": {
            "claude": {
                "id": "claude",
                "label": "Claude",
                "distribution": "claude-agent-acp",
                "launch_identity": "77".repeat(32),
                "status": "not_run",
                "gates": {}
            }
        },
        "ranking": {
            "status": "insufficient_evidence",
            "recommended": [],
            "experimental": ["claude"],
            "notes": "none"
        },
        "acceptance": {
            "development": "not_run",
            "authentic": "not_run",
            "full": "not_run",
            "reason": "none"
        }
    });
    std::fs::write(
        ledger_dir.join(qualification::LEDGER_FILE_M6),
        m6_content.to_string(),
    )
    .unwrap();

    // Because this launch is NOT in M6, it legitimately falls back to M3 and qualifies via M3!
    let resolution = qualification::resolve_with(&ledger_dir, &digest, &platform);
    match resolution.ownership {
        WriterOwnership::ProcessGroupContained { qualification } => {
            assert!(qualification.starts_with("m3-ledger:auth_writer_process_group:"));
        }
        _ => panic!(
            "Expected legacy M3 ProcessGroupContained, got {:?}",
            resolution.ownership
        ),
    }
}

#[test]
fn qualification_resolution_refuses_when_second_cited_file_is_tampered() {
    let (_temp, paths) = setup_paths();
    let ledger_dir = paths.data.join(qualification::LEDGER_DIR);
    std::fs::create_dir_all(&ledger_dir).unwrap();

    let digest = "88".repeat(32);
    let platform = Platform {
        system: "Linux".into(),
        machine: "x86_64".into(),
    };

    let good_evidence = json!({
        "evidence_kind": "authentic",
        "schema": "m6-authentic/1",
        "gate": "interruption_and_cleanup",
        "fixture_only": false,
        "platform": {"system": "Linux", "arch": "x86_64"},
        "adapter": {"name": "claude-agent-acp", "protocol_version": 1, "launch_identity": digest},
        "cleanup": {"owned_processes_after": 0},
        "measurements": {"scenarios": [
            {"name": "edit", "escaped_descendants": 0, "group_empty_after": true},
            {"name": "stop", "escaped_descendants": 0, "group_empty_after": true},
            {"name": "provider_crash", "escaped_descendants": 0, "group_empty_after": true}
        ]}
    });
    let (ev_good_path, ev_good_hash) =
        write_evidence_file(&ledger_dir, "ev_good.json", &good_evidence);

    let second_evidence = json!({
        "evidence_kind": "authentic",
        "schema": "m6-authentic/1",
        "gate": "interruption_and_cleanup",
        "fixture_only": false,
        "platform": {"system": "Linux", "arch": "x86_64"},
        "adapter": {"name": "claude-agent-acp", "protocol_version": 1, "launch_identity": digest},
        "cleanup": {"owned_processes_after": 0},
        "measurements": {"scenarios": []}
    });
    let (ev_tampered_path, _real_hash) =
        write_evidence_file(&ledger_dir, "ev_tampered.json", &second_evidence);
    // Claim a different/tampered hash in the ledger!
    let tampered_claim = "99".repeat(32);

    let m6_content = json!({
        "kind": "m6",
        "schema_version": 1,
        "timestamp": "2026-10-07T07:12:00Z",
        "environment": {
            "scope": "test",
            "os": "linux",
            "arch": "x86_64",
            "sdk_id": "test",
            "sdk_manifest_sha256": "0".repeat(64)
        },
        "providers": {
            "claude": {
                "id": "claude",
                "label": "Claude",
                "distribution": "claude-agent-acp",
                "launch_identity": digest,
                "status": "qualified",
                "gates": {
                    "interruption_and_cleanup": {
                        "kind": "authentic",
                        "status": "pass",
                        "criteria": "clean teardown",
                        "notes": "passed",
                        "evidence": [
                            {"path": ev_good_path, "sha256": ev_good_hash},
                            {"path": ev_tampered_path, "sha256": tampered_claim}
                        ]
                    }
                }
            }
        },
        "ranking": {
            "status": "insufficient_evidence",
            "recommended": [],
            "experimental": ["claude"],
            "notes": "testing"
        },
        "acceptance": {
            "development": "not_run",
            "authentic": "not_run",
            "full": "not_run",
            "reason": "testing"
        }
    });
    std::fs::write(
        ledger_dir.join(qualification::LEDGER_FILE_M6),
        m6_content.to_string(),
    )
    .unwrap();

    let resolution = qualification::resolve_with(&ledger_dir, &digest, &platform);
    assert_eq!(
        resolution.ownership,
        WriterOwnership::Unknown,
        "When second cited file is tampered, writer containment must be strictly refused!"
    );
}
