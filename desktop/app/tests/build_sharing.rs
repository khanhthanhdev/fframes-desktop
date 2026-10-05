use fframes_studio::{
    build_service::{
        BuildError, BuildKey, BuildLimits, BuildService, BuildState, CompileEnvironment,
        CompileRequest, Subscriber, SubscriberKind,
    },
    preview_coordinator::{BuildSpec, PreviewCoordinator},
    worker_project::shared_build_service,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use studio_bootstrap::{ChildEnvironment, ProcessTreeManager};
use studio_engine::{Controller, JobKind, JobResult, app_paths::AppPaths};

#[path = "support/build_fixture.rs"]
mod fixture;
use fixture::*;

struct Env {
    temp: tempfile::TempDir,
    sdk: std::path::PathBuf,
    builds: std::path::PathBuf,
}

fn env() -> Env {
    let temp = tempfile::tempdir().unwrap();
    let sdk = fake_sdk(temp.path());
    let builds = temp.path().join("builds");
    Env { temp, sdk, builds }
}

fn service(compiler: Arc<FakeCompiler>, limits: BuildLimits) -> BuildService {
    BuildService::new(ProcessTreeManager::new(), compiler, limits)
}

fn who(kind: SubscriberKind, name: &str) -> Subscriber {
    Subscriber::new(kind, name)
}

fn never() -> bool {
    false
}

#[test]
fn build_key_separates_every_input_that_changes_the_bytes() {
    let e = env();
    let root = e.temp.path().join("video");
    let project = create_project(&root);
    let m = manifest();
    let base_env = environment(&e.sdk, &e.builds);
    let env_of = |m: &studio_sdk::CompatibilityManifest| environment_for(&e.sdk, &e.builds, m);
    let base = BuildKey::worker(&project, &base_env);
    assert_eq!(base, BuildKey::worker(&project, &base_env), "deterministic");
    assert_eq!(
        base.digest(),
        BuildKey::worker(&project, &base_env).digest()
    );

    let mut digests = std::collections::HashSet::new();
    digests.insert(base.digest());
    let mut distinct = |key: BuildKey| {
        assert_ne!(key, base);
        assert!(digests.insert(key.digest()), "digest collision for {key:?}");
    };
    // Source revision.
    distinct(BuildKey::worker(
        &with_revision(&root, "other source"),
        &base_env,
    ));
    // SDK identity, toolchain, target.
    let mut sdk = m.clone();
    sdk.sdk_id = "another-sdk".into();
    distinct(BuildKey::worker(&project, &env_of(&sdk)));
    let mut toolchain = m.clone();
    toolchain.rust_toolchain.channel = "nightly".into();
    distinct(BuildKey::worker(&project, &env_of(&toolchain)));
    let mut target = m.clone();
    target.target_triple = "aarch64-apple-darwin".into();
    distinct(BuildKey::worker(&project, &env_of(&target)));
    // Package / worker entry.
    let mut package = project.clone();
    package.manifest.entry.package = "other-package".into();
    distinct(BuildKey::worker(&package, &base_env));
    let mut worker = project.clone();
    worker.manifest.entry.worker_target = "other-worker".into();
    distinct(BuildKey::worker(&worker, &base_env));
    // Features, backend, options.
    distinct(base.clone().with_features(["gpu".to_owned()]));
    distinct(base.clone().with_backend("gpu"));
    distinct(base.clone().with_options(["--locked".to_owned()]));
    // Project identity.
    let other_root = e.temp.path().join("other");
    let other = create_project(&other_root);
    distinct(BuildKey::worker(&other, &base_env));
    // Feature order is not identity.
    assert_eq!(
        base.clone().with_features(["b".to_owned(), "a".to_owned()]),
        base.clone().with_features(["a".to_owned(), "b".to_owned()])
    );
    // Each key field is individually part of identity (not only via the digest).
    let field = |key: &BuildKey| {
        (
            key.sdk_id.clone(),
            key.toolchain.clone(),
            key.target_triple.clone(),
            key.compatibility_digest.clone(),
            key.entry_manifest.clone(),
            key.package.clone(),
            key.worker_target.clone(),
            key.features.clone(),
            key.backend.clone(),
            key.options.clone(),
            key.project_id.clone(),
            key.source_revision.clone(),
        )
    };
    let other_sdk = BuildKey::worker(&project, &env_of(&sdk));
    assert_ne!(other_sdk.sdk_id, base.sdk_id);
    assert_eq!(other_sdk.toolchain, base.toolchain);
    let other_toolchain = BuildKey::worker(&project, &env_of(&toolchain));
    assert_ne!(other_toolchain.toolchain, base.toolchain);
    assert_eq!(other_toolchain.sdk_id, base.sdk_id);
    let other_target = BuildKey::worker(&project, &env_of(&target));
    assert_ne!(other_target.target_triple, base.target_triple);
    assert_eq!(other_target.toolchain, base.toolchain);
    let mut entry = project.clone();
    entry.manifest.entry.manifest =
        studio_project::ProjectPath::try_from("other/Cargo.toml".to_owned()).unwrap();
    let other_entry = BuildKey::worker(&entry, &base_env);
    assert_ne!(other_entry.entry_manifest, base.entry_manifest);
    assert_ne!(field(&other_entry), field(&base));
    assert_eq!(other_entry.package, base.package);
    distinct(other_entry);
    // Session, task and generation are subscriber identity, never key fields.
    let json = serde_json::to_value(&base).unwrap();
    let fields: std::collections::BTreeSet<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for forbidden in ["session", "task", "generation", "subscriber", "tag"] {
        assert!(!fields.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn concurrent_equal_key_requests_compile_once_and_share_one_build() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let compiler = FakeCompiler::blocked(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let key = build_key(&project, &e.sdk, &e.builds);
    let mut subscriptions = Vec::new();
    for (i, kind) in [
        SubscriberKind::Ui,
        SubscriberKind::Tool,
        SubscriberKind::Validation,
        SubscriberKind::Tool,
    ]
    .into_iter()
    .enumerate()
    {
        subscriptions.push(
            svc.subscribe(
                key.clone(),
                request(&project, &e.sdk, &e.builds),
                who(kind, &format!("subscriber-{i}")),
            )
            .unwrap(),
        );
    }
    compiler.wait_started(1);
    assert_eq!(svc.status(&key), BuildState::Compiling { subscribers: 4 });
    compiler.release.store(true, Ordering::SeqCst);
    let leases: Vec<_> = subscriptions
        .into_iter()
        .map(|s| std::thread::spawn(move || s.wait(&never).unwrap()))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    assert_eq!(
        compiler.started.load(Ordering::SeqCst),
        1,
        "exactly one compile"
    );
    assert!(
        leases
            .windows(2)
            .all(|w| Arc::ptr_eq(w[0].build(), w[1].build()))
    );
    // Each completion is bound to its own subscriber; none can claim another's authority.
    for (i, lease) in leases.iter().enumerate() {
        assert_eq!(lease.subscriber().identity, format!("subscriber-{i}"));
        assert_eq!(lease.key(), &key);
    }
    assert_eq!(leases.iter().filter(|l| !l.shared()).count(), 1);
    // A later equal request is a cache hit, not a compile.
    let hit = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, "later"),
        )
        .unwrap();
    assert!(hit.is_ready());
    assert!(Arc::ptr_eq(
        hit.wait(&never).unwrap().build(),
        leases[0].build()
    ));
    let stats = svc.stats();
    assert_eq!(
        (stats.compiles_started, stats.cache_hits, stats.joins),
        (1, 1, 3)
    );
    assert_eq!(stats.leased_entries, 1);
}

#[test]
fn different_keys_never_reuse_a_build() {
    let e = env();
    let root = e.temp.path().join("video");
    let first = create_project(&root);
    let original = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    let compiler = FakeCompiler::new(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let a = svc
        .subscribe(
            build_key(&first, &e.sdk, &e.builds),
            request(&first, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "a"),
        )
        .unwrap()
        .wait(&never)
        .unwrap();
    let second = with_revision(&root, "edited");
    let b = svc
        .subscribe(
            build_key(&second, &e.sdk, &e.builds),
            request(&second, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "b"),
        )
        .unwrap()
        .wait(&never)
        .unwrap();
    assert_eq!(compiler.started.load(Ordering::SeqCst), 2);
    assert!(!Arc::ptr_eq(a.build(), b.build()));
    assert_ne!(a.build().root, b.build().root);
    assert_eq!(
        std::fs::read_to_string(a.build().root.join("src/lib.rs")).unwrap(),
        original,
        "the first build keeps the first source bytes"
    );
    assert!(
        std::fs::read_to_string(b.build().root.join("src/lib.rs"))
            .unwrap()
            .contains("// edited")
    );
}

#[test]
fn cancelling_one_subscriber_keeps_the_others_and_the_last_cancel_kills_the_compile() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let compiler = FakeCompiler::blocked(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let key = build_key(&project, &e.sdk, &e.builds);
    let sub = |name: &str| {
        svc.subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, name),
        )
        .unwrap()
    };
    let (a, b, c) = (sub("a"), sub("b"), sub("c"));
    compiler.wait_started(1);
    let cancel_a = Arc::new(AtomicBool::new(false));
    let flag = cancel_a.clone();
    let a = std::thread::spawn(move || a.wait(&|| flag.load(Ordering::SeqCst)));
    let b = std::thread::spawn(move || b.wait(&never));
    std::thread::sleep(Duration::from_millis(50));
    cancel_a.store(true, Ordering::SeqCst);
    assert_eq!(a.join().unwrap().err(), Some(BuildError::Cancelled));
    assert_eq!(
        compiler.killed.load(Ordering::SeqCst),
        0,
        "others still need it"
    );
    // Dropping an unwaited subscription also detaches only that subscriber.
    drop(c);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(svc.status(&key), BuildState::Compiling { subscribers: 1 });
    compiler.release.store(true, Ordering::SeqCst);
    let lease = b.join().unwrap().unwrap();
    assert_eq!(compiler.started.load(Ordering::SeqCst), 1);
    assert_eq!(compiler.killed.load(Ordering::SeqCst), 0);
    assert!(lease.build().manifest.is_file());

    // The last subscriber leaving kills the underlying compile.
    let solo = create_project(&e.temp.path().join("solo"));
    let key = build_key(&with_revision(&solo.root, "solo"), &e.sdk, &e.builds);
    let compiler2 = FakeCompiler::blocked(false);
    let svc2 = service(compiler2.clone(), BuildLimits::default());
    let project = studio_project::open(&solo.root).unwrap();
    let only = svc2
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "only"),
        )
        .unwrap();
    compiler2.wait_started(1);
    assert_eq!(only.wait(&|| true).err(), Some(BuildError::Cancelled));
    let deadline = Instant::now() + Duration::from_secs(5);
    while compiler2.killed.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "compile was not killed");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(svc2.status(&key), BuildState::Idle);
    // A later request starts a fresh compile instead of joining the killed one.
    compiler2.release.store(true, Ordering::SeqCst);
    let again = svc2
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "again"),
        )
        .unwrap()
        .wait(&never)
        .unwrap();
    assert_eq!(compiler2.started.load(Ordering::SeqCst), 2);
    assert!(again.build().manifest.is_file());
}

#[test]
fn failures_are_delivered_to_everyone_and_never_cached() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let compiler = FakeCompiler::blocked(false);
    compiler.fail.store(true, Ordering::SeqCst);
    let svc = service(compiler.clone(), BuildLimits::default());
    let key = build_key(&project, &e.sdk, &e.builds);
    let a = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "a"),
        )
        .unwrap();
    let b = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, "b"),
        )
        .unwrap();
    compiler.release.store(true, Ordering::SeqCst);
    for s in [a, b] {
        match s.wait(&never).err().unwrap() {
            BuildError::Failed(message) => assert!(message.contains("E0425")),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(svc.status(&key), BuildState::Idle);
    compiler.fail.store(false, Ordering::SeqCst);
    svc.subscribe(
        key,
        request(&project, &e.sdk, &e.builds),
        who(SubscriberKind::Ui, "retry"),
    )
    .unwrap()
    .wait(&never)
    .unwrap();
    assert_eq!(compiler.started.load(Ordering::SeqCst), 2);
}

#[test]
fn live_leases_are_never_evicted_and_a_full_cache_refuses_instead() {
    let e = env();
    let root = e.temp.path().join("video");
    create_project(&root);
    let compiler = FakeCompiler::new(false);
    let svc = service(
        compiler.clone(),
        BuildLimits {
            max_entries: 2,
            eviction_target_bytes: u64::MAX,
        },
    );
    let build_of = |marker: &str| {
        let project = with_revision(&root, marker);
        let key = build_key(&project, &e.sdk, &e.builds);
        (
            key.clone(),
            svc.subscribe(
                key,
                request(&project, &e.sdk, &e.builds),
                who(SubscriberKind::Tool, marker),
            ),
        )
    };
    let (_, one) = build_of("one");
    let one = one.unwrap().wait(&never).unwrap();
    let (key_two, two) = build_of("two");
    let two = two.unwrap().wait(&never).unwrap();
    // Both entries are leased by live consumers: refuse, do not evict.
    let (_, three) = build_of("three");
    match three.err().unwrap() {
        BuildError::CacheFull {
            entries: 2,
            leased: 2,
        } => {}
        other => panic!("{other:?}"),
    }
    assert!(one.build().manifest.is_file() && two.build().manifest.is_file());
    assert_eq!(svc.stats().evictions, 0);
    // Releasing one lease makes room: that entry (and only it) is evicted.
    let two_root = two.build().root.clone();
    drop(two);
    let (key_three, three) = build_of("three");
    let three = three.unwrap().wait(&never).unwrap();
    assert_eq!(svc.stats().evictions, 1);
    assert_eq!(svc.status(&key_two), BuildState::Idle);
    assert!(!two_root.exists(), "the evicted unleased tree is removed");
    assert!(
        one.build().manifest.is_file(),
        "the live lease survived eviction"
    );
    assert!(matches!(
        svc.status(&key_three),
        BuildState::Ready { leases: 1, .. }
    ));
    drop((one, three));
}

#[test]
fn the_byte_target_evicts_only_unleased_entries() {
    let e = env();
    let root = e.temp.path().join("video");
    create_project(&root);
    let compiler = FakeCompiler::new(false);
    let svc = service(
        compiler.clone(),
        BuildLimits {
            max_entries: 8,
            eviction_target_bytes: 1,
        },
    );
    let attempt = |marker: &str| {
        let project = with_revision(&root, marker);
        svc.subscribe(
            build_key(&project, &e.sdk, &e.builds),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, marker),
        )
    };
    let get = |marker: &str| attempt(marker).unwrap().wait(&never).unwrap();
    let leased = get("leased");
    // Over the 1-byte target with the only entry leased: refuse, never evict it.
    match attempt("refused").err().unwrap() {
        BuildError::CacheFull { leased: 1, .. } => {}
        other => panic!("{other:?}"),
    }
    assert!(leased.build().manifest.is_file());
    assert_eq!(svc.stats().evictions, 0);
    let leased_root = leased.build().root.clone();
    drop(leased);
    // Unleased and over target: evicted to make room for the newest build.
    let newest = get("newest");
    assert!(!leased_root.exists());
    assert_eq!(svc.stats().evictions, 1);
    assert!(newest.build().manifest.is_file());
    assert_eq!(svc.stats().cached_entries, 1);
}

#[test]
fn closing_a_project_kills_its_compiles_and_drops_the_cache_but_not_live_leases() {
    let e = env();
    let root = e.temp.path().join("video");
    let first = create_project(&root);
    let compiler = FakeCompiler::new(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let key = build_key(&first, &e.sdk, &e.builds);
    let lease = svc
        .subscribe(
            key.clone(),
            request(&first, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "ui"),
        )
        .unwrap()
        .wait(&never)
        .unwrap();
    // An in-flight compile of another revision of the same project.
    let second = with_revision(&root, "second");
    compiler.release.store(false, Ordering::SeqCst);
    let pending_key = build_key(&second, &e.sdk, &e.builds);
    let pending = svc
        .subscribe(
            pending_key.clone(),
            request(&second, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, "tool"),
        )
        .unwrap();
    compiler.wait_started(2);
    svc.close_project(&key.project_id);
    assert_eq!(pending.wait(&never).err(), Some(BuildError::ProjectClosed));
    let deadline = Instant::now() + Duration::from_secs(5);
    while compiler.killed.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "compile survived project close");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(svc.status(&key), BuildState::Idle);
    assert_eq!(svc.status(&pending_key), BuildState::Idle);
    assert!(
        lease.build().manifest.is_file(),
        "a live lease keeps its tree"
    );
    let tree = lease.build().root.clone();
    drop(lease);
    assert!(
        !tree.exists(),
        "the tree goes when the last consumer lets go"
    );
    svc.close();
    assert_eq!(
        svc.subscribe(
            key,
            request(&first, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "late")
        )
        .err(),
        Some(BuildError::ServiceClosed)
    );
}

#[test]
fn observing_status_never_starts_a_compile() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let compiler = FakeCompiler::new(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let key = build_key(&project, &e.sdk, &e.builds);
    assert_eq!(svc.status(&key), BuildState::Idle);
    assert!(svc.project_status(&key.project_id).is_empty());
    let _ = svc.stats();
    assert_eq!(compiler.started.load(Ordering::SeqCst), 0);
}

fn wait_for_ready(p: &PreviewCoordinator, c: &mut Controller) -> Arc<studio_engine::ReadyPreview> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let e = p.events();
        if let Some(tag) = e.compiled {
            c.complete(&tag, JobResult::Built(tag.base_source.clone()))
                .unwrap();
        }
        if let Some((_, error)) = e.error {
            panic!("preview failed: {error}");
        }
        if let Some(r) = e.ready {
            return r;
        }
        assert!(Instant::now() < deadline, "preview readiness deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn ui_preparation_and_a_tool_subscriber_with_the_same_key_compile_once() {
    let e = env();
    let root = e.temp.path().join("video");
    create_project(&root);
    let paths = AppPaths::new(e.temp.path().join("data")).unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let compiler = FakeCompiler::blocked(true);
    let svc = service(compiler.clone(), BuildLimits::default());
    let coordinator = PreviewCoordinator::new(controller.processes.sub_manager());
    let tag = controller.begin_job(JobKind::Build).unwrap();
    let project = controller.project.clone();
    let key = build_key(&project, &e.sdk, &e.builds);
    coordinator.build(BuildSpec {
        project: project.clone(),
        sdk: e.sdk.clone(),
        compatibility: manifest(),
        builds: e.builds.clone(),
        tag: tag.clone(),
        compiler: controller.operation_processes(),
        worker: controller.processes.sub_manager(),
        service: svc.clone(),
    });
    compiler.wait_started(1);
    // The tool joins the UI's in-flight compile.
    let tool = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, "tool"),
        )
        .unwrap();
    compiler.release.store(true, Ordering::SeqCst);
    let ready = wait_for_ready(&coordinator, &mut controller);
    let lease = tool.wait(&never).unwrap();
    assert_eq!(
        compiler.started.load(Ordering::SeqCst),
        1,
        "one compile for UI + tool"
    );
    assert!(lease.shared());
    // The UI candidate is installable only through its own operation tag.
    assert_eq!(ready.tag(), &tag);
    assert_eq!(ready.timeline.total_frames, 90);
    assert!(ready.frame.is_some());
    // Cancelling the UI build detaches only the UI: the tool's lease stays valid.
    coordinator.cancel_build();
    assert!(lease.build().manifest.is_file());
    coordinator.close();
}

#[test]
fn cancelling_the_ui_compile_scope_leaves_a_tool_subscriber_running() {
    let e = env();
    let root = e.temp.path().join("video");
    create_project(&root);
    let paths = AppPaths::new(e.temp.path().join("data")).unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let compiler = FakeCompiler::blocked(true);
    let svc = service(compiler.clone(), BuildLimits::default());
    let coordinator = PreviewCoordinator::new(controller.processes.sub_manager());
    let tag = controller.begin_job(JobKind::Build).unwrap();
    let project = controller.project.clone();
    let key = build_key(&project, &e.sdk, &e.builds);
    let ui_scope = controller.operation_processes();
    coordinator.build(BuildSpec {
        project: project.clone(),
        sdk: e.sdk.clone(),
        compatibility: manifest(),
        builds: e.builds.clone(),
        tag,
        compiler: ui_scope.clone(),
        worker: controller.processes.sub_manager(),
        service: svc.clone(),
    });
    compiler.wait_started(1);
    let tool = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, "tool"),
        )
        .unwrap();
    coordinator.cancel_build();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        compiler.killed.load(Ordering::SeqCst),
        0,
        "the tool still wants this build"
    );
    assert!(matches!(
        svc.status(&key),
        BuildState::Compiling { subscribers: 1 }
    ));
    compiler.release.store(true, Ordering::SeqCst);
    assert!(tool.wait(&never).unwrap().build().manifest.is_file());
    assert_eq!(compiler.started.load(Ordering::SeqCst), 1);
    coordinator.close();
}

#[test]
fn the_process_wide_service_is_one_shared_instance_for_ui_and_tools() {
    // Both facades must reach the same cache: a clone shares state.
    let a = shared_build_service();
    let b = shared_build_service();
    assert_eq!(a.stats(), b.stats());
}

struct PanickingCompiler;
impl fframes_studio::build_service::Compiler for PanickingCompiler {
    fn compile(
        &self,
        _request: &fframes_studio::build_service::CompileRequest,
        _scope: &ProcessTreeManager,
    ) -> Result<Arc<studio_engine::build_materialization::MaterializedBuild>, String> {
        // Long enough for the second subscriber to join before the compile dies.
        std::thread::sleep(Duration::from_millis(200));
        panic!("compiler bug");
    }
}

#[test]
fn a_crashing_compiler_fails_every_subscriber_and_leaves_the_service_usable() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let svc = BuildService::new(
        ProcessTreeManager::new(),
        Arc::new(PanickingCompiler),
        BuildLimits::default(),
    );
    let key = build_key(&project, &e.sdk, &e.builds);
    let a = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "a"),
        )
        .unwrap();
    let b = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, "b"),
        )
        .unwrap();
    for s in [a, b] {
        match s.wait(&never).err().unwrap() {
            BuildError::Failed(message) => assert!(message.contains("panicked")),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(svc.status(&key), BuildState::Idle);
    assert_eq!(svc.stats().in_flight, 0);
    // The failed entry is not cached and its slot is free again.
    let again = svc
        .subscribe(
            key,
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "again"),
        )
        .unwrap();
    assert!(again.wait(&never).is_err());
    assert_eq!(svc.stats().compiles_started, 2);
}

#[test]
fn default_limits_are_eight_entries_and_a_two_gib_target() {
    let limits = BuildLimits::default();
    assert_eq!(limits.max_entries, 8);
    assert_eq!(limits.eviction_target_bytes, 2 * 1024 * 1024 * 1024);
    assert_eq!(fframes_studio::build_service::MAX_CACHED_BUILDS, 8);
}

#[test]
fn a_cached_entry_whose_artifacts_vanished_is_never_reused() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let compiler = FakeCompiler::new(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let key = build_key(&project, &e.sdk, &e.builds);
    let first = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "first"),
        )
        .unwrap()
        .wait(&never)
        .unwrap();
    // The tree is damaged behind the service's back: even a live lease cannot make the
    // entry trustworthy again.
    std::fs::remove_file(&first.build().manifest).unwrap();
    let second = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, "second"),
        )
        .unwrap();
    assert!(
        !second.is_ready(),
        "an uncertain entry must not be a cache hit"
    );
    let second = second.wait(&never).unwrap();
    assert_eq!(compiler.started.load(Ordering::SeqCst), 2);
    assert!(!Arc::ptr_eq(first.build(), second.build()));
    assert!(second.build().manifest.is_file());
    assert_eq!(svc.stats().cache_hits, 0);
}

#[test]
fn closing_the_whole_service_kills_in_flight_compiles_and_fails_every_waiter() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let compiler = FakeCompiler::blocked(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let key = build_key(&project, &e.sdk, &e.builds);
    let a = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "a"),
        )
        .unwrap();
    let b = svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, "b"),
        )
        .unwrap();
    compiler.wait_started(1);
    let waiters: Vec<_> = [a, b]
        .into_iter()
        .map(|s| std::thread::spawn(move || s.wait(&never).err()))
        .collect();
    std::thread::sleep(Duration::from_millis(50));
    svc.close();
    for waiter in waiters {
        assert_eq!(waiter.join().unwrap(), Some(BuildError::ServiceClosed));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while compiler.killed.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "compile survived service close");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(compiler.finished.load(Ordering::SeqCst), 0);
    assert_eq!(svc.stats().cached_entries, 0);
    assert_eq!(svc.status(&key), BuildState::Idle);
}

#[test]
fn a_key_the_compiler_cannot_honor_is_refused_before_any_compile_or_cache_entry() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let compiler = FakeCompiler::new(false);
    *compiler.refuse.lock() = Some("features are not supported".into());
    let svc = service(compiler.clone(), BuildLimits::default());
    let key = build_key(&project, &e.sdk, &e.builds).with_features(["gpu".to_owned()]);
    match svc
        .subscribe(
            key.clone(),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, "gpu"),
        )
        .err()
        .unwrap()
    {
        BuildError::Failed(message) => assert!(message.contains("Unsupported build key")),
        other => panic!("{other:?}"),
    }
    assert_eq!(compiler.started.load(Ordering::SeqCst), 0);
    assert_eq!(svc.stats().cached_entries + svc.stats().in_flight, 0);
    assert_eq!(svc.status(&key), BuildState::Idle);
}

#[test]
fn the_cargo_compiler_only_accepts_the_default_cpu_debug_worker_key() {
    use fframes_studio::{build_service::Compiler, worker_project::CargoCompiler};
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let base = build_key(&project, &e.sdk, &e.builds);
    assert!(CargoCompiler.accepts(&base).is_ok());
    for key in [
        base.clone().with_features(["gpu".to_owned()]),
        base.clone().with_backend("gpu"),
        base.clone().with_options(["--release".to_owned()]),
    ] {
        assert!(
            CargoCompiler.accepts(&key).is_err(),
            "{key:?} would be cached under bytes it was not built with"
        );
    }
}

fn host(path: &str, libclang: &str) -> ChildEnvironment {
    let mut host = ChildEnvironment::empty();
    host.set("PATH", path).set("LIBCLANG_PATH", libclang);
    host
}

fn request_with(
    project: &studio_project::OpenProject,
    environment: &CompileEnvironment,
) -> CompileRequest {
    CompileRequest {
        project: project.clone(),
        environment: environment.clone(),
        retained: None,
    }
}

#[test]
fn a_different_consumed_environment_input_is_a_different_key_and_never_joins() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let resolve = |host: ChildEnvironment| {
        CompileEnvironment::resolve_from(&e.sdk, &manifest(), &e.builds, host).unwrap()
    };
    let a = resolve(host("/opt/a/bin:/usr/bin", "/opt/a/lib"));
    let same = resolve(host("/opt/a/bin:/usr/bin", "/opt/a/lib"));
    let other_path = resolve(host("/opt/b/bin:/usr/bin", "/opt/a/lib"));
    let other_clang = resolve(host("/opt/a/bin:/usr/bin", "/opt/b/lib"));

    let key_a = BuildKey::worker(&project, &a);
    assert_eq!(key_a, BuildKey::worker(&project, &same), "same inputs");
    assert_eq!(key_a.digest(), BuildKey::worker(&project, &same).digest());
    for other in [&other_path, &other_clang] {
        let key = BuildKey::worker(&project, other);
        assert_ne!(key, key_a);
        assert_ne!(key.digest(), key_a.digest());
        assert_ne!(key.environment_digest, key_a.environment_digest);
        // Everything else the key names is unchanged: only the environment differs.
        assert_eq!(key.source_revision, key_a.source_revision);
        assert_eq!(key.compatibility_digest, key_a.compatibility_digest);
        assert_eq!(key.sdk_installation, key_a.sdk_installation);
    }

    let compiler = FakeCompiler::blocked(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let subscribe = |environment: &CompileEnvironment, name: &str| {
        svc.subscribe(
            BuildKey::worker(&project, environment),
            request_with(&project, environment),
            who(SubscriberKind::Tool, name),
        )
        .unwrap()
    };
    let first = subscribe(&a, "a");
    let second = subscribe(&other_path, "b");
    let third = subscribe(&other_clang, "c");
    let joined = subscribe(&same, "same-inputs");
    compiler.wait_started(3);
    assert_eq!(svc.stats().joins, 1, "only the equal environment joins");
    compiler.release.store(true, Ordering::SeqCst);
    let lease_a = first.wait(&never).unwrap();
    let lease_b = second.wait(&never).unwrap();
    let lease_c = third.wait(&never).unwrap();
    let lease_same = joined.wait(&never).unwrap();
    assert_eq!(compiler.started.load(Ordering::SeqCst), 3);
    assert!(Arc::ptr_eq(lease_a.build(), lease_same.build()));
    assert!(!Arc::ptr_eq(lease_a.build(), lease_b.build()));
    assert!(!Arc::ptr_eq(lease_a.build(), lease_c.build()));

    // The compile used exactly the frozen inputs the key digested.
    let frozen = |lease: &fframes_studio::build_service::BuildLease, name: &str| {
        lease
            .build()
            .environment
            .build_child_environment()
            .get(name)
            .map(str::to_owned)
    };
    assert_eq!(
        frozen(&lease_a, "PATH").as_deref(),
        Some("/opt/a/bin:/usr/bin")
    );
    assert_eq!(
        frozen(&lease_a, "LIBCLANG_PATH").as_deref(),
        Some("/opt/a/lib")
    );
    assert_eq!(
        frozen(&lease_b, "PATH").as_deref(),
        Some("/opt/b/bin:/usr/bin")
    );
    assert_eq!(
        frozen(&lease_c, "LIBCLANG_PATH").as_deref(),
        Some("/opt/b/lib")
    );

    // Equal environment later: a cache hit, no fourth compile.
    let again = subscribe(&same, "later");
    assert!(again.is_ready());
    drop(again.wait(&never).unwrap());
    assert_eq!(compiler.started.load(Ordering::SeqCst), 3);
    assert_eq!(svc.stats().cache_hits, 1);
}

#[test]
fn a_different_sdk_installation_is_a_different_key_and_binds_its_own_directory() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let second_sdk = fake_sdk(&e.temp.path().join("second"));
    let resolve = |sdk: &std::path::Path| {
        CompileEnvironment::resolve_from(sdk, &manifest(), &e.builds, host("/bin", "/lib")).unwrap()
    };
    let first = resolve(&e.sdk);
    let key_first = BuildKey::worker(&project, &first);
    assert_eq!(key_first, BuildKey::worker(&project, &resolve(&e.sdk)));
    let second = resolve(&second_sdk);
    let key_second = BuildKey::worker(&project, &second);
    assert_ne!(key_second, key_first);
    assert_ne!(key_second.sdk_installation, key_first.sdk_installation);
    assert_eq!(
        key_second.compatibility_digest,
        key_first.compatibility_digest
    );

    // The same directory with a replaced install receipt is another installation, though
    // the child environment it produces is identical.
    std::fs::write(e.sdk.join("compatibility.json"), "{\"receipt\":1}").unwrap();
    let receipted = resolve(&e.sdk);
    let key_receipted = BuildKey::worker(&project, &receipted);
    assert_ne!(key_receipted.sdk_installation, key_first.sdk_installation);
    assert_eq!(
        key_receipted.environment_digest,
        key_first.environment_digest
    );
    assert_ne!(key_receipted, key_first);
    std::fs::write(e.sdk.join("compatibility.json"), "{\"receipt\":2}").unwrap();
    let replaced = BuildKey::worker(&project, &resolve(&e.sdk));
    assert_ne!(replaced.sdk_installation, key_receipted.sdk_installation);

    // Each compile binds its own installation; the second never joins or reuses the first.
    let compiler = FakeCompiler::new(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let build_with = |environment: &CompileEnvironment, name: &str| {
        svc.subscribe(
            BuildKey::worker(&project, environment),
            request_with(&project, environment),
            who(SubscriberKind::Tool, name),
        )
        .unwrap()
        .wait(&never)
        .unwrap()
    };
    let one = build_with(&second, "second");
    let two = build_with(&receipted, "first-receipted");
    assert_eq!(compiler.started.load(Ordering::SeqCst), 2);
    assert_eq!(
        one.build().environment.sdk_dir,
        std::fs::canonicalize(&second_sdk).unwrap()
    );
    assert_eq!(
        two.build().environment.sdk_dir,
        std::fs::canonicalize(&e.sdk).unwrap()
    );
    assert!(!Arc::ptr_eq(one.build(), two.build()));
}

#[test]
fn artifact_bytes_count_against_the_budget_after_the_compile_settled() {
    const MIB: u64 = 1024 * 1024;
    let e = env();
    let root = e.temp.path().join("video");
    create_project(&root);
    let compiler = FakeCompiler::new(false);
    let svc = service(
        compiler.clone(),
        BuildLimits {
            max_entries: 8,
            eviction_target_bytes: MIB,
        },
    );
    let get = |marker: &str| {
        let project = with_revision(&root, marker);
        svc.subscribe(
            build_key(&project, &e.sdk, &e.builds),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Tool, marker),
        )
        .and_then(|subscription| subscription.wait(&never))
    };
    let idle = get("idle").unwrap();
    let idle_root = idle.build().root.clone();
    let idle_key = idle.key().clone();
    drop(idle);
    let live = get("live").unwrap();
    let live_key = live.key().clone();
    let settled = svc.stats();
    assert_eq!(settled.artifact_bytes, 0);
    assert!(settled.cached_bytes > 0 && settled.cached_bytes < MIB);

    // PCM allocated inside the leased materialization after the compile settled.
    let pcm = live.account_artifact_bytes(2 * MIB).unwrap();
    assert_eq!(pcm.bytes(), 2 * MIB);
    let stats = svc.stats();
    assert_eq!(stats.artifact_bytes, 2 * MIB);
    assert_eq!(stats.cached_bytes, settled.cached_bytes + 2 * MIB);
    match svc.status(&live_key) {
        BuildState::Ready {
            leases: 2,
            bytes,
            artifact_bytes,
        } => {
            assert_eq!(artifact_bytes, 2 * MIB);
            assert!(bytes > 2 * MIB);
        }
        other => panic!("{other:?}"),
    }

    // Over the target: the idle entry is evicted first, then the live consumption
    // refuses new work rather than touching the leased tree.
    match get("refused").err().unwrap() {
        BuildError::CacheFull { leased: 1, .. } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(svc.stats().evictions, 1);
    assert_eq!(svc.status(&idle_key), BuildState::Idle);
    assert!(!idle_root.exists());
    assert!(live.build().manifest.is_file());

    // The guard alone keeps the entry leased (open-file retention): still refused.
    drop(live);
    match get("still refused").err().unwrap() {
        BuildError::CacheFull { leased: 1, .. } => {}
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        svc.status(&live_key),
        BuildState::Ready { leases: 1, artifact_bytes, .. } if artifact_bytes == 2 * MIB
    ));
    assert_eq!(svc.stats().evictions, 1, "a leased entry is never evicted");

    // Releasing returns the bytes: admission works again without evicting anything.
    pcm.release();
    let after = svc.stats();
    assert_eq!(after.artifact_bytes, 0);
    assert!(
        after.cached_bytes < settled.cached_bytes,
        "only the live materialization is left"
    );
    drop(get("admitted").unwrap());
    assert_eq!(svc.stats().evictions, 1);
    assert_eq!(svc.stats().cached_entries, 2);
}

#[test]
fn artifact_allocations_add_up_and_each_release_returns_only_its_own_bytes() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let compiler = FakeCompiler::new(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let lease = svc
        .subscribe(
            build_key(&project, &e.sdk, &e.builds),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "ui"),
        )
        .unwrap()
        .wait(&never)
        .unwrap();
    let base = svc.stats().cached_bytes;
    let a = lease.account_artifact_bytes(1_000).unwrap();
    let b = svc.account_artifact_bytes(lease.build(), 250).unwrap();
    assert_eq!(svc.stats().artifact_bytes, 1_250);
    assert_eq!(svc.stats().cached_bytes, base + 1_250);
    drop(a);
    assert_eq!(svc.stats().artifact_bytes, 250);
    drop(b);
    assert_eq!(svc.stats().artifact_bytes, 0);
    assert_eq!(svc.stats().cached_bytes, base);
}

#[test]
fn accounting_against_an_entry_the_cache_dropped_is_refused_without_panicking() {
    let e = env();
    let project = create_project(&e.temp.path().join("video"));
    let compiler = FakeCompiler::new(false);
    let svc = service(compiler.clone(), BuildLimits::default());
    let get = || {
        svc.subscribe(
            build_key(&project, &e.sdk, &e.builds),
            request(&project, &e.sdk, &e.builds),
            who(SubscriberKind::Ui, "ui"),
        )
        .unwrap()
        .wait(&never)
        .unwrap()
    };
    let stale = get();
    let before_close = stale.account_artifact_bytes(4_096).unwrap();
    svc.close_project(&stale.key().project_id);
    // The entry is gone from the cache although the lease keeps the tree alive.
    assert_eq!(svc.stats().cached_entries, 0);
    assert_eq!(svc.stats().artifact_bytes, 0);
    assert_eq!(
        stale.account_artifact_bytes(8).err(),
        Some(BuildError::NotCached)
    );
    assert_eq!(
        svc.account_artifact_bytes(stale.build(), 8).err(),
        Some(BuildError::NotCached)
    );
    drop(before_close);
    assert_eq!(svc.stats().artifact_bytes, 0);

    // A rebuilt entry for the same key is a different tree: the stale one stays refused.
    let fresh = get();
    assert!(!Arc::ptr_eq(stale.build(), fresh.build()));
    assert_eq!(
        stale.account_artifact_bytes(8).err(),
        Some(BuildError::NotCached)
    );
    let guard = fresh.account_artifact_bytes(64).unwrap();
    assert_eq!(svc.stats().artifact_bytes, 64);
    // Closing the whole service retires every entry; releasing afterwards is harmless.
    svc.close();
    assert_eq!(svc.stats().cached_entries, 0);
    assert_eq!(
        fresh.account_artifact_bytes(8).err(),
        Some(BuildError::NotCached)
    );
    drop(guard);
    assert_eq!(svc.stats().artifact_bytes, 0);
}
