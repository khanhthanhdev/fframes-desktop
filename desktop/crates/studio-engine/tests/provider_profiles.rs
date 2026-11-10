//! Integration tests for provider profiles and the registry file.

use serde_json::json;
use studio_bootstrap::ProcessTreeManager;
use studio_engine::app_paths::AppPaths;
use studio_engine::providers::{
    host::{self, AdapterFile, McpChoice, REGISTRY_BACKUP_SUFFIX, REGISTRY_FILE},
    provider_profiles::{
        BUILTIN_ANTIGRAVITY_ID, BUILTIN_CLAUDE_ID, BUILTIN_CODEX_ID, BUILTIN_PI_ID,
        ProviderProfile, ProviderRegistry, REGISTRY_SCHEMA_VERSION, check_registry_version,
    },
};
use studio_harness::{
    AdapterConfig,
    discovery::{AdapterStatus, ExecutableSearch, ProbeOptions, probe_adapter},
};
use tempfile::TempDir;

fn setup_paths() -> (TempDir, AppPaths) {
    let temp = TempDir::new().unwrap();
    let paths = AppPaths::new(temp.path().join("data")).unwrap();
    (temp, paths)
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

    for (id, wire, executable) in [
        (
            BUILTIN_CLAUDE_ID,
            studio_harness::AgentWire::ClaudeStreamJson,
            "claude",
        ),
        (
            BUILTIN_CODEX_ID,
            studio_harness::AgentWire::CodexAppServer,
            "codex",
        ),
        (BUILTIN_PI_ID, studio_harness::AgentWire::Acp, "pi-acp"),
        (
            BUILTIN_ANTIGRAVITY_ID,
            studio_harness::AgentWire::Acp,
            "antigravity-acp",
        ),
    ] {
        let profile = registry
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .unwrap();
        assert_eq!(profile.adapter.wire, wire);
        assert_eq!(profile.adapter.executable, executable);
        assert_eq!(
            profile.adapter_identity(),
            studio_engine::providers::provider_profiles::AdapterIdentity::Name(executable.into())
        );
    }

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
        wire: studio_harness::AgentWire::Acp,
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

/// What an `agent-adapter.json` of an earlier build looks like on disk.
const OLD_ADAPTER_FILE: &str = "agent-adapter.json";

fn write_old_adapter_file(paths: &AppPaths) -> String {
    let old = AdapterFile {
        provider: "My Custom Agent".into(),
        executable: "/opt/custom/agent".into(),
        args: vec!["--acp".into()],
        auth_env_names: vec!["CUSTOM_TOKEN".into()],
        auth_method: None,
        mcp: McpChoice::Unsupported,
        wire: studio_harness::AgentWire::Acp,
    }
    .to_pretty();
    std::fs::write(paths.data.join(OLD_ADAPTER_FILE), &old).unwrap();
    old
}

/// A registry file as the first registry version wrote it (no CLI names, schema version 1).
fn write_version_one_registry(paths: &AppPaths) -> String {
    let mut value = serde_json::to_value(ProviderRegistry::default()).unwrap();
    value["schema_version"] = json!(1);
    let text = serde_json::to_string_pretty(&value).unwrap();
    std::fs::write(paths.data.join(REGISTRY_FILE), &text).unwrap();
    text
}

fn adapter(provider: &str) -> AdapterFile {
    AdapterFile {
        provider: provider.into(),
        executable: "/opt/custom/exec".into(),
        args: vec![],
        auth_env_names: vec![],
        auth_method: None,
        mcp: McpChoice::Baseline,
        wire: studio_harness::AgentWire::Acp,
    }
}

#[test]
fn an_adapter_file_of_an_earlier_build_is_ignored_and_setup_is_needed() {
    let (_temp, paths) = setup_paths();
    let old = write_old_adapter_file(&paths);

    assert_eq!(host::load_registry(&paths), Ok(None));
    assert_eq!(host::load_settings(&paths), Ok(None));
    assert_eq!(
        std::fs::read_to_string(paths.data.join(OLD_ADAPTER_FILE)).unwrap(),
        old
    );
    assert!(!paths.data.join(REGISTRY_FILE).exists());
}

#[test]
fn an_adapter_file_of_an_earlier_build_survives_every_registry_write() {
    let (_temp, paths) = setup_paths();
    let old = write_old_adapter_file(&paths);

    let mut registry = ProviderRegistry::default();
    registry.select_profile(BUILTIN_CODEX_ID).unwrap();
    host::save_registry(&paths, &registry).unwrap();
    host::save_settings(&paths, &adapter("Other")).unwrap();
    assert_eq!(
        host::load_settings(&paths).unwrap().unwrap().executable,
        "/opt/custom/exec"
    );
    host::clear_settings(&paths).unwrap();

    assert!(!paths.data.join(REGISTRY_FILE).exists());
    assert_eq!(
        std::fs::read_to_string(paths.data.join(OLD_ADAPTER_FILE)).unwrap(),
        old,
        "the old file is never read, rewritten or removed"
    );
}

#[test]
fn a_version_one_registry_is_refused_and_left_as_it_is() {
    let (_temp, paths) = setup_paths();
    let old = write_version_one_registry(&paths);
    let path = paths.data.join(REGISTRY_FILE);
    let same = || std::fs::read_to_string(&path).unwrap() == old;

    let loaded = host::load_registry(&paths).unwrap_err();
    assert!(loaded.contains("older Studio"), "{loaded}");
    assert_eq!(host::load_settings(&paths).unwrap_err(), loaded);
    assert!(loaded.contains(REGISTRY_FILE), "{loaded}");
    assert!(ProviderRegistry::parse(&old).is_err());
    assert!(same());

    // Nothing that writes may replace or remove it.
    let mut fresh = ProviderRegistry::default();
    fresh.select_profile(BUILTIN_CLAUDE_ID).unwrap();
    for refused in [
        host::save_registry(&paths, &fresh),
        host::save_settings(&paths, &adapter("Other")),
        host::save_profile_settings(&paths, BUILTIN_CLAUDE_ID, &adapter("claude")),
        host::clear_registry(&paths),
        host::clear_settings(&paths),
    ] {
        let error = refused.unwrap_err();
        assert!(error.contains("older Studio"), "{error}");
        assert!(error.contains(REGISTRY_FILE), "{error}");
        assert!(same());
    }
    assert!(
        !paths
            .data
            .join(format!("{REGISTRY_FILE}{REGISTRY_BACKUP_SUFFIX}"))
            .exists(),
        "no rollback copy is made of a file that is not replaced"
    );
}

#[test]
fn a_registry_that_is_replaced_keeps_a_rollback_copy_of_the_previous_one() {
    let (_temp, paths) = setup_paths();
    let mut first = ProviderRegistry::default();
    first.select_profile(BUILTIN_CLAUDE_ID).unwrap();
    host::save_registry(&paths, &first).unwrap();
    let before = std::fs::read_to_string(paths.data.join(REGISTRY_FILE)).unwrap();

    let mut second = first.clone();
    second.select_profile(BUILTIN_CODEX_ID).unwrap();
    host::save_registry(&paths, &second).unwrap();

    let backup = paths
        .data
        .join(format!("{REGISTRY_FILE}{REGISTRY_BACKUP_SUFFIX}"));
    assert_eq!(std::fs::read_to_string(backup).unwrap(), before);
    assert_eq!(
        host::load_registry(&paths)
            .unwrap()
            .unwrap()
            .selected_profile_id,
        Some(BUILTIN_CODEX_ID.to_string())
    );
}
#[test]
fn validation_rejects_duplicate_ids() {
    let mut registry = ProviderRegistry::default();
    registry.profiles.push(ProviderProfile {
        id: BUILTIN_CLAUDE_ID.into(),
        label: "Duplicate Claude".into(),
        description: "".into(),
        cli_executable: String::new(),
        adapter: AdapterFile {
            provider: "Dup".into(),
            executable: "/bin/true".into(),
            args: vec![],
            auth_env_names: vec![],
            auth_method: None,
            mcp: McpChoice::Baseline,
            wire: studio_harness::AgentWire::Acp,
        },
        builtin: false,
        experimental: true,
    });
    assert!(registry.validate().is_err());
}

#[test]
fn validation_rejects_credentials_in_auth_env_names() {
    let mut registry = ProviderRegistry::default();
    registry.profiles[0].adapter.auth_env_names = vec!["sk-ant-api03-secret1234567890".into()];
    assert!(registry.validate().is_err());
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
        wire: studio_harness::AgentWire::Acp,
    };
    host::save_settings(&paths, &custom_adapter).unwrap();

    let loaded_reg = host::load_registry(&paths).unwrap().unwrap();
    // Builtin Claude defaults must remain untouched!
    let claude = loaded_reg
        .profiles
        .iter()
        .find(|p| p.id == BUILTIN_CLAUDE_ID)
        .unwrap();
    assert_eq!(claude.adapter.executable, "claude");

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
        node: None,
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
fn a_registry_of_another_schema_version_is_not_parsed() {
    let mut value = serde_json::to_value(ProviderRegistry::default()).unwrap();
    value["schema_version"] = json!(1);
    for profile in value["profiles"].as_array_mut().unwrap() {
        profile.as_object_mut().unwrap().remove("cli_executable");
    }
    assert!(ProviderRegistry::parse(&value.to_string()).is_err());
    value["schema_version"] = json!(REGISTRY_SCHEMA_VERSION + 1);
    assert!(ProviderRegistry::parse(&value.to_string()).is_err());
    let named =
        check_registry_version(&value.to_string(), "/data/provider-registry.json").unwrap_err();
    assert!(named.contains("/data/provider-registry.json"), "{named}");
    assert!(named.contains("newer"), "{named}");
}

#[test]
fn valid_json_that_is_not_a_registry_is_refused_and_only_other_text_is_replaced() {
    let (_temp, paths) = setup_paths();
    let path = paths.data.join(REGISTRY_FILE);
    let mut fresh = ProviderRegistry::default();
    fresh.select_profile(BUILTIN_CLAUDE_ID).unwrap();

    for shaped in [
        r#"{"unrelated": true}"#,
        r#"[1, 2, 3]"#,
        r#"{"schema_version": 2, "profiles": "none"}"#,
    ] {
        std::fs::write(&path, shaped).unwrap();
        let loaded = host::load_registry(&paths).unwrap_err();
        assert!(loaded.contains(REGISTRY_FILE), "{loaded}");
        for refused in [
            host::save_registry(&paths, &fresh),
            host::save_settings(&paths, &adapter("Other")),
            host::clear_settings(&paths),
        ] {
            let error = refused.unwrap_err();
            assert!(error.contains("not in the format"), "{error}");
            assert!(error.contains(REGISTRY_FILE), "{error}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), shaped);
        }
    }

    // Text that is not JSON is garbage a save may replace.
    std::fs::write(&path, "not json {").unwrap();
    host::save_registry(&paths, &fresh).unwrap();
    assert!(host::load_registry(&paths).unwrap().is_some());
}

#[test]
fn a_registry_must_carry_exactly_the_current_schema_version() {
    for version in [0, REGISTRY_SCHEMA_VERSION + 1] {
        let registry = ProviderRegistry {
            schema_version: version,
            ..Default::default()
        };
        assert!(registry.validate().is_err(), "version {version}");
    }
}

#[test]
fn registry_round_trips_and_derives_detection_specs() {
    let registry = ProviderRegistry::default();
    let parsed = ProviderRegistry::parse(&registry.to_pretty()).unwrap();
    assert_eq!(parsed, registry);
    let specs = registry.detection_specs();
    let claude = specs.iter().find(|s| s.id == BUILTIN_CLAUDE_ID).unwrap();
    assert_eq!(claude.cli_executable, "claude");
    assert_eq!(claude.adapter_executable, "claude");
    let native = specs
        .iter()
        .find(|s| s.id == BUILTIN_ANTIGRAVITY_ID)
        .unwrap();
    assert_eq!(native.cli_executable, native.adapter_executable);
}

#[test]
fn detection_result_maps_to_row_state() {
    use studio_engine::providers::provider_profiles::DetectionState;
    use studio_harness::Detected;
    let found = |cli: bool, adapter: bool| Detected {
        id: "x".into(),
        cli: cli.then(|| "/bin/x".into()),
        adapter: adapter.then(|| "/bin/x-acp".into()),
    };
    assert_eq!(
        DetectionState::of(&found(true, true)),
        DetectionState::Ready
    );
    assert_eq!(
        DetectionState::of(&found(false, true)),
        DetectionState::Ready
    );
    assert_eq!(
        DetectionState::of(&found(true, false)),
        DetectionState::AdapterMissing
    );
    assert_eq!(
        DetectionState::of(&found(false, false)),
        DetectionState::NotFound
    );
}

#[test]
fn node_launch_of_a_selected_adapter_survives_the_setup_fields() {
    // Select puts `node <entry>` into the Setup text fields and Save reads them back.
    let file = AdapterFile {
        provider: "Claude".into(),
        executable: r"C:\Program Files\nodejs\node.exe".into(),
        args: vec![r"C:\Users\Some One\AppData\Roaming\npm\pkg\lib\index.js".into()],
        auth_env_names: vec![],
        auth_method: None,
        mcp: McpChoice::Baseline,
        wire: studio_harness::AgentWire::Acp,
    };
    let rebuilt = AdapterFile::from_fields(None, &file.fields(), McpChoice::Baseline)
        .unwrap()
        .unwrap();
    assert_eq!(rebuilt, file);
}

#[test]
fn detect_rows_offer_the_action_their_state_allows() {
    use studio_engine::providers::provider_profiles::{
        DetectRowAction as A, DetectionState as S, row_action,
    };
    let guide = Some("https://example.com/install");
    for installable in [false, true] {
        for tools in [false, true] {
            for g in [None, guide] {
                assert_eq!(row_action(S::Ready, installable, tools, g), A::TurnOn);
                let missing = row_action(S::AdapterMissing, installable, tools, g);
                let expected = match (installable, tools, g) {
                    (true, true, _) => A::InstallAdapter,
                    (true, false, _) => A::GetNode,
                    (false, _, Some(url)) => A::InstallGuide(url),
                    (false, _, None) => A::None,
                };
                assert_eq!(missing, expected, "{installable} {tools} {g:?}");
                assert_eq!(
                    row_action(S::NotFound, installable, tools, g),
                    g.map_or(A::None, A::InstallGuide)
                );
            }
        }
    }
}

#[test]
fn only_https_install_guides_are_opened() {
    use studio_engine::providers::provider_profiles::{
        NODE_DOWNLOAD_URL, install_guide_url, is_openable,
    };
    for refused in [
        "http://example.com",
        "file:///C:/Windows/system32/calc.exe",
        "ms-settings:privacy",
        "javascript:alert(1)",
        "",
        " https://example.com",
        "https://",
        "HTTPS//example.com",
    ] {
        assert!(!is_openable(refused), "{refused:?}");
    }
    assert!(is_openable("https://example.com/install"));
    assert!(is_openable(NODE_DOWNLOAD_URL));
    for id in [BUILTIN_CLAUDE_ID, BUILTIN_CODEX_ID, BUILTIN_PI_ID] {
        let url = install_guide_url(id).unwrap_or_else(|| panic!("{id} has a guide"));
        assert!(is_openable(url), "{url}");
    }
    assert_eq!(install_guide_url(BUILTIN_ANTIGRAVITY_ID), None);
    assert_eq!(install_guide_url("my-custom-adapter"), None);
}

/// An executable file Detect agents accepts: `<name>.exe` on Windows, mode 755 elsewhere.
fn fake_executable(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    });
    std::fs::write(&path, b"x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// A detection environment whose only search root is `bin`.
fn only_path(bin: &std::path::Path) -> studio_harness::DetectionEnv {
    studio_harness::DetectionEnv {
        path: Some(std::env::join_paths([bin]).unwrap()),
        ..Default::default()
    }
}

fn node_launch(node: &std::path::Path, entry: &std::path::Path) -> AdapterFile {
    AdapterFile {
        provider: "Claude".into(),
        executable: node.to_string_lossy().into_owned(),
        args: vec![entry.to_string_lossy().into_owned()],
        auth_env_names: vec!["ANTHROPIC_API_KEY".into()],
        auth_method: None,
        mcp: McpChoice::Baseline,
        wire: studio_harness::AgentWire::Acp,
    }
}

#[test]
fn a_saved_node_launch_never_makes_a_deleted_builtin_adapter_look_ready() {
    use studio_engine::providers::provider_profiles::{DetectionState, scan_agents};
    let (temp, paths) = setup_paths();
    let bin = temp.path().join("bin");
    let node = fake_executable(&bin, "node");
    let entry = temp.path().join("deleted").join("index.js");
    let env = only_path(&bin);
    let adapters = paths.data.join("adapters");
    let claude_state = |registry: &ProviderRegistry| {
        let scan = scan_agents(registry, &env, &adapters);
        let found = scan
            .found
            .iter()
            .find(|found| found.id == BUILTIN_CLAUDE_ID)
            .unwrap();
        (scan.state(found), scan.is_ready(BUILTIN_CLAUDE_ID))
    };

    // Saved by id (the turn-on path) and by label (Save, or a registry written before).
    host::save_profile_settings(&paths, BUILTIN_CLAUDE_ID, &node_launch(&node, &entry)).unwrap();
    let by_id = host::load_registry(&paths).unwrap().unwrap();
    host::save_settings(&paths, &node_launch(&node, &entry)).unwrap();
    let by_label = host::load_registry(&paths).unwrap().unwrap();
    for registry in [&by_id, &by_label] {
        let claude = registry.selected_profile().unwrap();
        assert_eq!(claude.id, BUILTIN_CLAUDE_ID);
        assert_eq!(claude.adapter.executable, node.to_string_lossy());
        assert_eq!(
            claude_state(registry),
            (DetectionState::NotFound, false),
            "only node is installed, so the adapter is not"
        );
    }

    // The adapter itself on the search path is what makes the row ready.
    fake_executable(&bin, "claude-agent-acp");
    assert_eq!(claude_state(&by_id), (DetectionState::Ready, true));
}

#[test]
fn a_turn_on_save_changes_only_that_profile_and_keeps_its_identity() {
    let (temp, paths) = setup_paths();
    let mut registry = ProviderRegistry::default();
    registry.save_custom_adapter(AdapterFile {
        provider: "Mine".into(),
        executable: "/opt/mine-acp".into(),
        args: vec![],
        auth_env_names: vec![],
        auth_method: None,
        mcp: McpChoice::Baseline,
        wire: studio_harness::AgentWire::Acp,
    });
    host::save_registry(&paths, &registry).unwrap();
    let before = host::load_registry(&paths).unwrap().unwrap();

    // A launch whose label names the built-in Claude, saved for the custom profile.
    let launch = node_launch(&temp.path().join("node"), &temp.path().join("a.js"));
    host::save_profile_settings(&paths, "mine", &launch).unwrap();
    let after = host::load_registry(&paths).unwrap().unwrap();

    assert_eq!(after.selected_profile_id.as_deref(), Some("mine"));
    for (old, new) in before.profiles.iter().zip(&after.profiles) {
        assert_eq!(
            (&old.id, &old.label, &old.description, &old.cli_executable),
            (&new.id, &new.label, &new.description, &new.cli_executable)
        );
        if new.id == "mine" {
            assert_eq!(new.adapter, launch);
        } else {
            assert_eq!(old.adapter, new.adapter, "{} is untouched", new.id);
        }
    }
    assert!(host::save_profile_settings(&paths, "gone", &launch).is_err());
}

#[test]
fn a_turn_on_save_creates_the_registry_when_none_exists() {
    let (temp, paths) = setup_paths();
    let launch = node_launch(&temp.path().join("node"), &temp.path().join("a.js"));
    host::save_profile_settings(&paths, BUILTIN_CODEX_ID, &launch).unwrap();
    let registry = host::load_registry(&paths).unwrap().unwrap();
    let codex = registry.selected_profile().unwrap();
    assert_eq!(codex.id, BUILTIN_CODEX_ID);
    assert_eq!(codex.label, "Codex");
    assert_eq!(codex.adapter, launch);
    assert_eq!(host::load_settings(&paths).unwrap(), Some(launch));
}

#[test]
fn a_custom_node_launch_is_detected_by_its_entry() {
    use studio_engine::providers::provider_profiles::{
        AdapterIdentity, DetectionState, scan_agents,
    };
    let (temp, paths) = setup_paths();
    let bin = temp.path().join("bin");
    let node = fake_executable(&bin, "node");
    let entry = temp.path().join("pkg").join("cli.js");
    let mut launch = node_launch(&node, &entry);
    launch.provider = "Mine".into();
    launch.args = vec![
        "--no-warnings".into(),
        launch.args[0].clone(),
        "--acp".into(),
    ];
    let mut registry = ProviderRegistry::default();
    registry.save_custom_adapter(launch);
    let mine = registry.selected_profile().unwrap().clone();
    assert_eq!(
        mine.adapter_identity(),
        AdapterIdentity::Entry(Some(entry.clone()))
    );
    // Selecting another file never carries the old entry along.
    assert_eq!(mine.launch_template().args, vec!["--acp"]);
    assert!(
        registry
            .detection_specs()
            .iter()
            .all(|spec| spec.id != mine.id),
        "node is never searched for as the adapter"
    );

    let state = || {
        let scan = scan_agents(&registry, &only_path(&bin), &paths.data.join("adapters"));
        let found = scan.found.iter().find(|f| f.id == mine.id).unwrap();
        scan.state(found)
    };
    assert_eq!(state(), DetectionState::NotFound);
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(&entry, "").unwrap();
    assert_eq!(state(), DetectionState::Ready);
}

#[test]
fn builtins_are_detected_by_their_own_adapter_name_whatever_launch_is_saved() {
    use studio_engine::providers::provider_profiles::{AdapterIdentity, builtin_adapter};
    let mut registry = ProviderRegistry::default();
    for profile in &mut registry.profiles {
        profile.adapter.wire = studio_harness::AgentWire::Acp;
        profile.adapter.executable = "/usr/bin/node".into();
        profile.adapter.args = vec!["/x/index.js".into()];
    }
    let specs = registry.detection_specs();
    assert_eq!(specs.len(), registry.profiles.len());
    for spec in specs {
        assert_eq!(
            Some(spec.adapter_executable.as_str()),
            builtin_adapter(&spec.id)
        );
    }
    for profile in &registry.profiles {
        assert!(matches!(
            profile.adapter_identity(),
            AdapterIdentity::Name(_)
        ));
        assert!(profile.launch_template().args.is_empty());
    }
}

#[test]
fn a_missing_install_tool_is_named() {
    use studio_engine::providers::provider_profiles::DetectionScan;
    let scan = |node: bool, npm: bool| DetectionScan {
        node: node.then(|| "/n/node".into()),
        npm: npm.then(|| "/n/npm".into()),
        ..Default::default()
    };
    assert_eq!(scan(true, false).missing_tools(), ["npm"]);
    assert_eq!(scan(false, true).missing_tools(), ["Node.js"]);
    assert_eq!(scan(false, false).missing_tools(), ["Node.js", "npm"]);
    assert!(scan(true, false).tools().is_none());
    assert!(scan(true, true).tools().is_some());
    assert!(scan(true, true).missing_tools().is_empty());
}

#[test]
fn native_defaults_detect_the_cli_without_node_or_an_acp_adapter() {
    use studio_engine::providers::provider_profiles::scan_agents;
    let (temp, paths) = setup_paths();
    let bin = temp.path().join("bin");
    let claude = fake_executable(&bin, "claude");
    let codex = fake_executable(&bin, "codex");
    let registry = ProviderRegistry::default();
    let scan = scan_agents(&registry, &only_path(&bin), &paths.data.join("adapters"));
    for (id, executable) in [(BUILTIN_CLAUDE_ID, claude), (BUILTIN_CODEX_ID, codex)] {
        let found = scan.found.iter().find(|found| found.id == id).unwrap();
        assert_eq!(found.adapter.as_ref(), Some(&executable));
        assert!(scan.is_ready(id));
    }
    // Detection always searches /opt/homebrew/bin and /usr/local/bin, where a CI Mac has Node.
    if studio_harness::find_executable("node", &studio_harness::DetectionEnv::default()).is_none() {
        assert!(scan.node.is_none());
    }
    assert!(scan.installed.is_empty());
}
