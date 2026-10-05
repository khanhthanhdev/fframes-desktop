//! Shared compile service: one compilation per distinct build key, any number of
//! subscribers.
//!
//! The key names everything that can change the produced bytes: project id,
//! immutable source revision, exact SDK compatibility digest/toolchain/target,
//! package/worker entry, features, profile and backend/options. Session, task and
//! generation are *subscriber* identity ([`Subscriber`]); they are never part of the
//! key and a shared artifact never carries install authority.
//!
//! The *effective compile environment* is an input too. [`CompileEnvironment`] resolves
//! the SDK installation (canonical directory plus a receipt-backed identity) and the exact
//! child environment (`SdkEnvironment::build_child_environment`: PATH, LIBCLANG_PATH,
//! SDKROOT/DEVELOPER_DIR, Windows INCLUDE/LIB, cargo/rustup homes, ...) once, *before* the
//! key exists. The key carries digests of both and the compile runs with exactly that
//! frozen environment, so key and inputs cannot diverge.
//!
//! * Equal keys share one in-flight compilation (above the Cargo target lock and the
//!   isolated binary copy performed by the [`Compiler`]).
//! * Cancelling a subscriber detaches only that subscriber. The underlying compile is
//!   killed when no subscriber remains or its project/service closes.
//! * Completed builds stay cached as `Arc<MaterializedBuild>`: at most
//!   [`BuildLimits::max_entries`] entries and an eviction byte target. An entry with a
//!   live lease (any other `Arc` holder: worker, prepared audio, artifact) is never
//!   evicted; a request that would need that room (by count or by bytes) is refused
//!   instead. A cached entry whose artifacts vanished ([`Compiler::verify`]) is dropped.
//! * Failures and cancellations are never cached.
//!
//! Bytes are accounted twice: the isolated materialization measured when the compile
//! settles, plus artifacts allocated inside it later (prepared PCM). Artifact holders
//! register them with [`BuildService::account_artifact_bytes`] and keep the returned
//! [`ArtifactBytes`] guard for as long as the artifact (or an open handle to it) exists.
//! Admission, eviction and stats use the sum.
use parking_lot::{Condvar, Mutex};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    any::Any,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use studio_bootstrap::{ChildEnvironment, ProcessTreeManager};
use studio_engine::build_materialization::MaterializedBuild;
use studio_project::OpenProject;
use studio_sdk::{CompatibilityManifest, environment::SdkEnvironment};

pub const MAX_CACHED_BUILDS: usize = 8;
pub const BUILD_EVICTION_TARGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildProfile {
    Debug,
}

impl BuildProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
        }
    }
}

/// Everything that determines the compiled worker bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct BuildKey {
    pub project_id: String,
    pub source_revision: String,
    pub sdk_id: String,
    pub compatibility_digest: String,
    /// Identity of the selected SDK installation (see [`CompileEnvironment`]).
    pub sdk_installation: String,
    /// Fingerprint of the frozen child environment the compile runs with.
    pub environment_digest: String,
    pub toolchain: String,
    pub target_triple: String,
    pub entry_manifest: String,
    pub package: String,
    pub worker_target: String,
    pub features: Vec<String>,
    pub profile: BuildProfile,
    pub backend: String,
    pub options: Vec<String>,
}

impl BuildKey {
    /// The CPU preview worker with default features, as built by the UI.
    pub fn worker(project: &OpenProject, environment: &CompileEnvironment) -> Self {
        let compatibility = environment.compatibility();
        Self {
            project_id: String::from(project.manifest.project_id.clone()),
            source_revision: project.inventory.revision.as_str().to_owned(),
            sdk_id: compatibility.sdk_id.clone(),
            compatibility_digest: compatibility.digest(),
            sdk_installation: environment.installation().to_owned(),
            environment_digest: environment.environment_digest().to_owned(),
            toolchain: compatibility.rust_toolchain.channel.clone(),
            target_triple: compatibility.target_triple.clone(),
            entry_manifest: project.manifest.entry.manifest.as_str().to_owned(),
            package: project.manifest.entry.package.clone(),
            worker_target: project.manifest.entry.worker_target.clone(),
            features: Vec::new(),
            profile: BuildProfile::Debug,
            backend: "cpu".into(),
            options: Vec::new(),
        }
    }

    pub fn with_features(mut self, features: impl IntoIterator<Item = String>) -> Self {
        self.features = features.into_iter().collect();
        self.features.sort();
        self.features.dedup();
        self
    }

    pub fn with_backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = backend.into();
        self
    }

    pub fn with_options(mut self, options: impl IntoIterator<Item = String>) -> Self {
        self.options = options.into_iter().collect();
        self
    }

    /// Stable identity of the key; declaration-ordered typed JSON, no map ordering.
    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("typed key serializes");
        format!("{:x}", Sha256::digest(bytes))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriberKind {
    Ui,
    Tool,
    Validation,
}

/// Who asked. Completion authority belongs to this identity, never to the artifact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Subscriber {
    pub kind: SubscriberKind,
    /// Operation tag / task / session description supplied by the caller.
    pub identity: String,
}

impl Subscriber {
    pub fn new(kind: SubscriberKind, identity: impl Into<String>) -> Self {
        Self {
            kind,
            identity: identity.into(),
        }
    }
}

/// The effective compile environment, resolved and frozen before key construction.
///
/// Holds the canonical SDK directory, a deterministic identity of that installation, the
/// frozen child environment ([`SdkEnvironment::freeze`]) and the builds directory (which
/// names the Cargo target dir). Every build compiled from it, and every worker later
/// launched from that build, uses exactly the frozen variables.
#[derive(Debug, Clone)]
pub struct CompileEnvironment {
    environment: SdkEnvironment,
    builds: PathBuf,
    installation: String,
    environment_digest: String,
}

impl CompileEnvironment {
    /// Resolve against the process environment (allowlisted host variables).
    pub fn resolve(
        sdk: &Path,
        compatibility: &CompatibilityManifest,
        builds: &Path,
    ) -> Result<Self, String> {
        Self::resolve_from(
            sdk,
            compatibility,
            builds,
            ChildEnvironment::default_allowlist(),
        )
    }

    /// Resolve against an explicit host environment instead of the process one.
    pub fn resolve_from(
        sdk: &Path,
        compatibility: &CompatibilityManifest,
        builds: &Path,
        host: ChildEnvironment,
    ) -> Result<Self, String> {
        let sdk_dir = std::fs::canonicalize(sdk)
            .map_err(|e| format!("SDK installation {} is unavailable: {e}", sdk.display()))?;
        let installation = installation_identity(&sdk_dir)?;
        let target = studio_engine::build_materialization::target_dir(builds, compatibility);
        let environment =
            SdkEnvironment::new(sdk_dir, target, compatibility.clone(), true).freeze(host);
        let mut hasher = Sha256::new();
        for (name, value) in environment.build_child_environment().iter() {
            for part in [name, value] {
                hasher.update((part.len() as u64).to_le_bytes());
                hasher.update(part.as_bytes());
            }
        }
        Ok(Self {
            environment,
            builds: builds.to_owned(),
            installation,
            environment_digest: format!("{:x}", hasher.finalize()),
        })
    }

    /// Canonical SDK installation directory the compile is bound to.
    pub fn sdk_dir(&self) -> &Path {
        &self.environment.sdk_dir
    }
    pub fn builds(&self) -> &Path {
        &self.builds
    }
    pub fn compatibility(&self) -> &CompatibilityManifest {
        &self.environment.manifest
    }
    /// Digest of the installation identity (path, directory identity, install receipt).
    pub fn installation(&self) -> &str {
        &self.installation
    }
    /// Digest of every variable the frozen child environment carries.
    pub fn environment_digest(&self) -> &str {
        &self.environment_digest
    }
    /// The frozen environment itself, for materialization and worker launches.
    pub fn sdk_environment(&self) -> SdkEnvironment {
        self.environment.clone()
    }
}

/// Deterministic identity of an installed SDK: canonical path, directory identity (unix
/// device/inode) and the install receipt `compatibility.json` (content digest and mtime).
fn installation_identity(dir: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(dir)
        .map_err(|e| format!("SDK installation {} is unreadable: {e}", dir.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "SDK installation {} is not a directory",
            dir.display()
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(b"path\0");
    hasher.update(dir.as_os_str().as_encoded_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hasher.update(b"\0directory\0");
        hasher.update(metadata.dev().to_le_bytes());
        hasher.update(metadata.ino().to_le_bytes());
    }
    let receipt = dir.join("compatibility.json");
    match std::fs::read(&receipt) {
        Ok(bytes) => {
            let modified = std::fs::metadata(&receipt)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            hasher.update(b"\0receipt\0");
            hasher.update(Sha256::digest(&bytes));
            hasher.update(modified.to_le_bytes());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            hasher.update(b"\0no-receipt\0");
        }
        Err(e) => {
            return Err(format!(
                "SDK install receipt {} is unreadable: {e}",
                receipt.display()
            ));
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Input of one compilation. Only the request that starts a compile is used; joiners'
/// requests are dropped (their key is equal by construction). The key must have been
/// built from this very `environment` (`BuildKey::worker(&project, &environment)`).
pub struct CompileRequest {
    pub project: OpenProject,
    pub environment: CompileEnvironment,
    /// Keeps a restored immutable source tree alive while compiling.
    pub retained: Option<Arc<dyn Any + Send + Sync>>,
}

/// The injectable compile step. `scope` is owned by this one compilation; shutting
/// it down must stop every process the compile started.
pub trait Compiler: Send + Sync + 'static {
    fn compile(
        &self,
        request: &CompileRequest,
        scope: &ProcessTreeManager,
    ) -> Result<Arc<MaterializedBuild>, String>;

    /// Refuse a key whose non-default inputs this compiler cannot honor: caching bytes
    /// under a key that names features/backend/options they were not built with would be
    /// a false cache hit.
    fn accepts(&self, _key: &BuildKey) -> Result<(), String> {
        Ok(())
    }

    /// True while every artifact of a completed build is still present. A cached entry
    /// that fails this is uncertain and is dropped, never reused.
    fn verify(&self, build: &MaterializedBuild) -> bool {
        build.manifest.is_file()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("Build cancelled")]
    Cancelled,
    #[error("Build service closed")]
    ServiceClosed,
    #[error("Project closed; build discarded")]
    ProjectClosed,
    #[error(
        "Build cache is full: {leased} of {entries} cached builds are leased by live consumers; close a preview or tool consumer and retry"
    )]
    CacheFull { entries: usize, leased: usize },
    #[error("{0}")]
    Failed(String),
    #[error("Build is no longer cached; its artifact bytes cannot be accounted")]
    NotCached,
}

#[derive(Debug, Clone, Copy)]
pub struct BuildLimits {
    pub max_entries: usize,
    pub eviction_target_bytes: u64,
}

impl Default for BuildLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_CACHED_BUILDS,
            eviction_target_bytes: BUILD_EVICTION_TARGET_BYTES,
        }
    }
}

type Outcome = Result<Arc<MaterializedBuild>, BuildError>;

struct Slot {
    result: Mutex<Option<Outcome>>,
    ready: Condvar,
}

impl Slot {
    /// The outcome if settled, else wait up to `timeout` for it.
    fn poll(&self, timeout: Duration) -> Option<Outcome> {
        let mut guard = self.result.lock();
        if guard.is_none() {
            self.ready.wait_for(&mut guard, timeout);
        }
        guard.clone()
    }
    fn new() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(None),
            ready: Condvar::new(),
        })
    }
    fn settle(&self, outcome: Outcome) {
        let mut guard = self.result.lock();
        if guard.is_none() {
            *guard = Some(outcome);
        }
        self.ready.notify_all();
    }
}

struct InFlight {
    id: u64,
    slot: Arc<Slot>,
    scope: ProcessTreeManager,
    subscribers: HashSet<u64>,
}

struct Completed {
    build: Arc<MaterializedBuild>,
    /// The isolated materialization measured when the compile settled.
    bytes: u64,
    last_used: u64,
    /// Artifacts allocated inside the materialization after settlement.
    ledger: Arc<Ledger>,
}

impl Completed {
    fn leased(&self) -> bool {
        Arc::strong_count(&self.build) > 1
    }
    fn total(&self) -> u64 {
        self.bytes.saturating_add(self.ledger.bytes())
    }
}

impl Drop for Completed {
    /// Every removal path (evict, vanished, project/service close) lands here, so no
    /// artifact is ever charged to an entry the cache no longer holds.
    fn drop(&mut self) {
        self.ledger.retire();
    }
}

#[derive(Default)]
struct LedgerState {
    bytes: u64,
    retired: bool,
}

/// Artifact bytes of one cached build. Lock order: service lock, then ledger.
#[derive(Default)]
struct Ledger {
    state: Mutex<LedgerState>,
}

impl Ledger {
    fn bytes(&self) -> u64 {
        self.state.lock().bytes
    }
    /// False when the entry left the cache.
    fn add(&self, bytes: u64) -> bool {
        let mut state = self.state.lock();
        if state.retired {
            return false;
        }
        state.bytes = state.bytes.saturating_add(bytes);
        true
    }
    fn sub(&self, bytes: u64) {
        let mut state = self.state.lock();
        state.bytes = state.bytes.saturating_sub(bytes);
    }
    fn retire(&self) {
        self.state.lock().retired = true;
    }
}

#[derive(Default)]
struct Inner {
    closed: bool,
    next_id: u64,
    tick: u64,
    in_flight: HashMap<BuildKey, InFlight>,
    completed: HashMap<BuildKey, Completed>,
}

#[derive(Default)]
struct Counters {
    compiles_started: AtomicU64,
    cache_hits: AtomicU64,
    joins: AtomicU64,
    evictions: AtomicU64,
}

/// Observed state of one key. Never launches a compile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BuildState {
    Idle,
    Compiling {
        subscribers: usize,
    },
    Ready {
        leases: usize,
        /// Materialization plus accounted artifacts.
        bytes: u64,
        /// The part of `bytes` registered through `account_artifact_bytes`.
        artifact_bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildStatusEntry {
    pub key_digest: String,
    pub source_revision: String,
    pub state: BuildState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BuildServiceStats {
    pub compiles_started: u64,
    pub cache_hits: u64,
    pub joins: u64,
    pub evictions: u64,
    pub cached_entries: usize,
    pub leased_entries: usize,
    pub in_flight: usize,
    /// Materializations plus accounted artifacts: what admission compares to the target.
    pub cached_bytes: u64,
    /// The part of `cached_bytes` registered through `account_artifact_bytes`.
    pub artifact_bytes: u64,
}

#[derive(Clone)]
pub struct BuildService {
    inner: Arc<Mutex<Inner>>,
    counters: Arc<Counters>,
    owner: ProcessTreeManager,
    compiler: Arc<dyn Compiler>,
    limits: BuildLimits,
}

impl BuildService {
    pub fn new(
        owner: ProcessTreeManager,
        compiler: Arc<dyn Compiler>,
        limits: BuildLimits,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            counters: Arc::new(Counters::default()),
            owner,
            compiler,
            limits,
        }
    }

    /// Acquire the completed build or the in-flight compilation for `key`, starting one
    /// only when neither exists.
    pub fn subscribe(
        &self,
        key: BuildKey,
        request: CompileRequest,
        subscriber: Subscriber,
    ) -> Result<Subscription, BuildError> {
        if let Err(reason) = self.compiler.accepts(&key) {
            return Err(BuildError::Failed(format!(
                "Unsupported build key: {reason}"
            )));
        }
        let mut inner = self.inner.lock();
        if inner.closed || self.owner.is_shutdown() {
            return Err(BuildError::ServiceClosed);
        }
        inner.tick += 1;
        let tick = inner.tick;
        if let Some(entry) = inner.completed.get_mut(&key) {
            if self.compiler.verify(&entry.build) {
                entry.last_used = tick;
                self.counters.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(Subscription::ready(BuildLease {
                    build: entry.build.clone(),
                    key,
                    subscriber,
                    service: self.clone(),
                    shared: true,
                }));
            }
            // An artifact vanished: never trust an uncertain entry.
            inner.completed.remove(&key);
        }
        inner.next_id += 1;
        let subscriber_id = inner.next_id;
        if let Some(flight) = inner.in_flight.get_mut(&key) {
            flight.subscribers.insert(subscriber_id);
            self.counters.joins.fetch_add(1, Ordering::Relaxed);
            return Ok(Subscription::waiting(
                self.clone(),
                key,
                flight.id,
                subscriber_id,
                flight.slot.clone(),
                subscriber,
                true,
            ));
        }
        self.make_room(&mut inner)?;
        inner.next_id += 1;
        let entry_id = inner.next_id;
        let slot = Slot::new();
        let scope = self.owner.sub_manager();
        inner.in_flight.insert(
            key.clone(),
            InFlight {
                id: entry_id,
                slot: slot.clone(),
                scope: scope.clone(),
                subscribers: HashSet::from([subscriber_id]),
            },
        );
        drop(inner);
        self.counters
            .compiles_started
            .fetch_add(1, Ordering::Relaxed);
        let service = self.clone();
        let thread_key = key.clone();
        std::thread::Builder::new()
            .name("build-service-compile".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    service.compiler.compile(&request, &scope)
                }))
                .unwrap_or_else(|_| Err("Compiler panicked".into()));
                drop(request);
                service.finish(thread_key, entry_id, result);
            })
            .map_err(|e| {
                let reason = format!("Cannot start compile thread: {e}");
                self.abandon(&key, entry_id, reason.clone());
                BuildError::Failed(reason)
            })?;
        Ok(Subscription::waiting(
            self.clone(),
            key,
            entry_id,
            subscriber_id,
            slot,
            subscriber,
            false,
        ))
    }

    fn abandon(&self, key: &BuildKey, entry_id: u64, reason: String) {
        let flight = {
            let mut inner = self.inner.lock();
            if inner
                .in_flight
                .get(key)
                .is_some_and(|flight| flight.id == entry_id)
            {
                inner.in_flight.remove(key)
            } else {
                None
            }
        };
        if let Some(flight) = flight {
            // Subscribers that joined in the window must not wait forever.
            flight.slot.settle(Err(BuildError::Failed(reason)));
            flight.scope.shutdown(Duration::ZERO);
        }
    }

    /// Admission: refuse rather than evict a live lease, by entry count or by bytes.
    fn make_room(&self, inner: &mut Inner) -> Result<(), BuildError> {
        loop {
            let over_count =
                inner.completed.len() + inner.in_flight.len() >= self.limits.max_entries;
            let over_bytes = cached_bytes(inner) > self.limits.eviction_target_bytes;
            if !over_count && !over_bytes {
                return Ok(());
            }
            if !self.evict_one(inner, None) {
                return Err(BuildError::CacheFull {
                    entries: inner.completed.len() + inner.in_flight.len(),
                    leased: inner.completed.values().filter(|c| c.leased()).count()
                        + inner.in_flight.len(),
                });
            }
        }
    }

    fn evict_one(&self, inner: &mut Inner, keep: Option<&BuildKey>) -> bool {
        let victim = inner
            .completed
            .iter()
            .filter(|(key, entry)| !entry.leased() && Some(*key) != keep)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone());
        match victim {
            Some(key) => {
                inner.completed.remove(&key);
                self.counters.evictions.fetch_add(1, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    fn finish(&self, key: BuildKey, entry_id: u64, result: Result<Arc<MaterializedBuild>, String>) {
        let bytes = result.as_ref().map_or(0, |build| build_bytes(build));
        let mut inner = self.inner.lock();
        if !inner
            .in_flight
            .get(&key)
            .is_some_and(|flight| flight.id == entry_id)
        {
            // Killed, detached or closed while compiling: the result has no owner.
            return;
        }
        let flight = inner.in_flight.remove(&key).expect("checked");
        let outcome = match result {
            Ok(build) => {
                inner.tick += 1;
                let last_used = inner.tick;
                inner.completed.insert(
                    key.clone(),
                    Completed {
                        build: build.clone(),
                        bytes,
                        last_used,
                        ledger: Arc::new(Ledger::default()),
                    },
                );
                while inner.completed.len() > self.limits.max_entries
                    || cached_bytes(&inner) > self.limits.eviction_target_bytes
                {
                    if !self.evict_one(&mut inner, Some(&key)) {
                        break;
                    }
                }
                Ok(build)
            }
            Err(error) => Err(BuildError::Failed(error)),
        };
        drop(inner);
        flight.slot.settle(outcome);
    }

    fn detach(&self, key: &BuildKey, entry_id: u64, subscriber_id: u64) {
        let scope = {
            let mut inner = self.inner.lock();
            let Some(flight) = inner.in_flight.get_mut(key) else {
                return;
            };
            if flight.id != entry_id {
                return;
            }
            flight.subscribers.remove(&subscriber_id);
            if !flight.subscribers.is_empty() {
                return;
            }
            let flight = inner.in_flight.remove(key).expect("present");
            flight.slot.settle(Err(BuildError::Cancelled));
            flight.scope
        };
        // Outside the service lock: terminating processes may take a moment.
        scope.shutdown(Duration::ZERO);
    }

    /// Observation only; never starts Cargo.
    pub fn status(&self, key: &BuildKey) -> BuildState {
        let inner = self.inner.lock();
        state_of(&inner, key)
    }

    pub fn project_status(&self, project_id: &str) -> Vec<BuildStatusEntry> {
        let inner = self.inner.lock();
        let mut keys: Vec<&BuildKey> = inner
            .in_flight
            .keys()
            .chain(inner.completed.keys())
            .filter(|key| key.project_id == project_id)
            .collect();
        keys.sort_by(|a, b| a.source_revision.cmp(&b.source_revision));
        keys.dedup();
        keys.into_iter()
            .map(|key| BuildStatusEntry {
                key_digest: key.digest(),
                source_revision: key.source_revision.clone(),
                state: state_of(&inner, key),
            })
            .collect()
    }

    pub fn stats(&self) -> BuildServiceStats {
        let inner = self.inner.lock();
        BuildServiceStats {
            compiles_started: self.counters.compiles_started.load(Ordering::Relaxed),
            cache_hits: self.counters.cache_hits.load(Ordering::Relaxed),
            joins: self.counters.joins.load(Ordering::Relaxed),
            evictions: self.counters.evictions.load(Ordering::Relaxed),
            cached_entries: inner.completed.len(),
            leased_entries: inner.completed.values().filter(|c| c.leased()).count(),
            in_flight: inner.in_flight.len(),
            cached_bytes: cached_bytes(&inner),
            artifact_bytes: artifact_bytes(&inner),
        }
    }

    /// Charge `bytes` of an artifact (prepared PCM, ...) written inside `build`'s
    /// materialization to the cache budget. Call it once the artifact exists and keep the
    /// guard for as long as the file or any open handle to it exists: admission, eviction
    /// order and `stats` count these bytes, and the guard holds the build lease so the
    /// entry cannot be evicted underneath it. Fails with [`BuildError::NotCached`] when
    /// `build` is not (or no longer) a cached entry of this service (evicted after a
    /// failed verification, or its project/service closed); nothing is charged then.
    pub fn account_artifact_bytes(
        &self,
        build: &Arc<MaterializedBuild>,
        bytes: u64,
    ) -> Result<ArtifactBytes, BuildError> {
        let inner = self.inner.lock();
        let entry = inner
            .completed
            .values()
            .find(|entry| Arc::ptr_eq(&entry.build, build))
            .ok_or(BuildError::NotCached)?;
        if !entry.ledger.add(bytes) {
            return Err(BuildError::NotCached);
        }
        Ok(ArtifactBytes {
            ledger: entry.ledger.clone(),
            bytes,
            _build: build.clone(),
        })
    }

    /// Kills compiles of `project_id` and drops its cached entries. Live leases keep
    /// their trees until the consumers release them.
    pub fn close_project(&self, project_id: &str) {
        let scopes = {
            let mut inner = self.inner.lock();
            inner
                .completed
                .retain(|key, _| key.project_id != project_id);
            let keys: Vec<BuildKey> = inner
                .in_flight
                .keys()
                .filter(|key| key.project_id == project_id)
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| inner.in_flight.remove(&key))
                .map(|flight| {
                    flight.slot.settle(Err(BuildError::ProjectClosed));
                    flight.scope
                })
                .collect::<Vec<_>>()
        };
        for scope in scopes {
            scope.shutdown(Duration::ZERO);
        }
    }

    pub fn close(&self) {
        let scopes = {
            let mut inner = self.inner.lock();
            inner.closed = true;
            inner.completed.clear();
            inner
                .in_flight
                .drain()
                .map(|(_, flight)| {
                    flight.slot.settle(Err(BuildError::ServiceClosed));
                    flight.scope
                })
                .collect::<Vec<_>>()
        };
        for scope in scopes {
            scope.shutdown(Duration::ZERO);
        }
    }
}

fn state_of(inner: &Inner, key: &BuildKey) -> BuildState {
    if let Some(done) = inner.completed.get(key) {
        BuildState::Ready {
            leases: Arc::strong_count(&done.build) - 1,
            bytes: done.total(),
            artifact_bytes: done.ledger.bytes(),
        }
    } else if let Some(flight) = inner.in_flight.get(key) {
        BuildState::Compiling {
            subscribers: flight.subscribers.len(),
        }
    } else {
        BuildState::Idle
    }
}

fn cached_bytes(inner: &Inner) -> u64 {
    inner
        .completed
        .values()
        .fold(0, |sum, entry| sum.saturating_add(entry.total()))
}

fn artifact_bytes(inner: &Inner) -> u64 {
    inner
        .completed
        .values()
        .fold(0, |sum, entry| sum.saturating_add(entry.ledger.bytes()))
}

/// Artifact bytes registered against a cached build, released on drop. Holding it also
/// holds the build lease, so the entry (and these bytes) cannot be evicted while the
/// artifact or an open handle to it exists.
pub struct ArtifactBytes {
    ledger: Arc<Ledger>,
    bytes: u64,
    _build: Arc<MaterializedBuild>,
}

impl ArtifactBytes {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    /// Return the bytes to the budget now (same as dropping the guard).
    pub fn release(self) {}
}

impl Drop for ArtifactBytes {
    fn drop(&mut self) {
        self.ledger.sub(self.bytes);
    }
}

impl std::fmt::Debug for ArtifactBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtifactBytes")
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

/// Size of the isolated materialization (copied source plus isolated binary).
fn build_bytes(build: &MaterializedBuild) -> u64 {
    fn walk(path: &Path, depth: usize) -> u64 {
        if depth > 32 {
            return 0;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&entry.path(), depth + 1),
                Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |m| m.len()),
                _ => 0,
            })
            .sum()
    }
    build
        .isolated_bin_dir
        .parent()
        .map_or(0, |staging| walk(staging, 0))
}

/// A completed build held by one subscriber. Holding the `Arc` is the lease: the cache
/// cannot evict the tree while any clone is alive. The lease names its subscriber and
/// grants no preview-install authority: that comes only from the caller's own tag.
#[derive(Clone)]
pub struct BuildLease {
    build: Arc<MaterializedBuild>,
    key: BuildKey,
    subscriber: Subscriber,
    shared: bool,
    service: BuildService,
}

impl BuildLease {
    pub fn build(&self) -> &Arc<MaterializedBuild> {
        &self.build
    }
    pub fn into_build(self) -> Arc<MaterializedBuild> {
        self.build
    }
    pub fn key(&self) -> &BuildKey {
        &self.key
    }
    /// The identity this completion was delivered to.
    pub fn subscriber(&self) -> &Subscriber {
        &self.subscriber
    }
    /// True when another subscriber started the compilation or it was cached.
    pub fn shared(&self) -> bool {
        self.shared
    }
    /// Account `bytes` of an artifact allocated inside this build's materialization; see
    /// [`BuildService::account_artifact_bytes`].
    pub fn account_artifact_bytes(&self, bytes: u64) -> Result<ArtifactBytes, BuildError> {
        self.service.account_artifact_bytes(&self.build, bytes)
    }
}

enum Pending {
    Ready(Option<BuildLease>),
    Waiting {
        service: BuildService,
        key: BuildKey,
        entry_id: u64,
        subscriber_id: u64,
        slot: Arc<Slot>,
        subscriber: Subscriber,
        shared: bool,
    },
    Done,
}

/// One subscriber's interest in a build. Dropping it detaches the subscriber.
pub struct Subscription {
    pending: Pending,
}

impl Subscription {
    fn ready(lease: BuildLease) -> Self {
        Self {
            pending: Pending::Ready(Some(lease)),
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn waiting(
        service: BuildService,
        key: BuildKey,
        entry_id: u64,
        subscriber_id: u64,
        slot: Arc<Slot>,
        subscriber: Subscriber,
        shared: bool,
    ) -> Self {
        Self {
            pending: Pending::Waiting {
                service,
                key,
                entry_id,
                subscriber_id,
                slot,
                subscriber,
                shared,
            },
        }
    }

    /// True when the result was already available at subscription time.
    pub fn is_ready(&self) -> bool {
        matches!(&self.pending, Pending::Ready(_))
    }

    /// Wait for the build. `cancelled` is polled about every 20 ms; when it returns true
    /// this subscriber detaches (killing the compile only if it was the last one).
    pub fn wait(mut self, cancelled: &dyn Fn() -> bool) -> Result<BuildLease, BuildError> {
        match std::mem::replace(&mut self.pending, Pending::Done) {
            Pending::Ready(lease) => lease.ok_or(BuildError::Cancelled),
            Pending::Done => Err(BuildError::Cancelled),
            Pending::Waiting {
                service,
                key,
                entry_id,
                subscriber_id,
                slot,
                subscriber,
                shared,
            } => loop {
                if let Some(outcome) = slot.poll(POLL) {
                    return outcome.map(|build| BuildLease {
                        build,
                        key,
                        subscriber,
                        shared,
                        service: service.clone(),
                    });
                }
                // The cancel closure runs without the slot lock: it may take other locks.
                if cancelled() {
                    service.detach(&key, entry_id, subscriber_id);
                    return Err(BuildError::Cancelled);
                }
            },
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Pending::Waiting {
            service,
            key,
            entry_id,
            subscriber_id,
            ..
        } = std::mem::replace(&mut self.pending, Pending::Done)
        {
            service.detach(&key, entry_id, subscriber_id);
        }
    }
}
