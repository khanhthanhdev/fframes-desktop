//! The disk space the managed SDK and the build caches use, and removing the caches no
//! installed SDK needs. Every SDK version compiles into its own Cargo target under
//! `builds/targets/<manifest digest>` (several GB each), so each SDK update would otherwise
//! leave the previous version's target behind. Callers run these off the UI thread.
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use studio_sdk::{install::SdkInstaller, manifest::CompatibilityManifest};

/// A target younger than this is kept even when no installed SDK owns it: an SDK install
/// running in another Studio may have just created it.
const NEW_TARGET_GRACE: Duration = Duration::from_secs(60 * 60);
/// A build folder (`builds/build-*`) normally removes itself when its build is dropped; one
/// older than this was left by a Studio that crashed or was killed.
const STALE_BUILD_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Bytes on disk, measured by [`measure`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StorageUsage {
    /// The active SDK.
    pub sdk: u64,
    /// The SDK the last install replaced, kept for projects pinned to it; `None` when
    /// there is none.
    pub rollback_sdk: Option<u64>,
    /// Build caches an installed SDK (or the open project's SDK) still uses.
    pub build_cache: u64,
    /// What [`clean`] removes.
    pub unused: u64,
}

/// Measures the SDKs under `sdk_home` and the caches under `builds`; `keep` is the open
/// project's SDK when it is not an installed one.
pub fn measure(
    sdk_home: &Path,
    builds: &Path,
    keep: Option<&CompatibilityManifest>,
) -> StorageUsage {
    let installer = SdkInstaller::new(sdk_home);
    let previous = installer.previous_sdk_dir();
    let (mut unused_size, mut unused_builds) = (0, 0);
    for path in unused(sdk_home, builds, keep, SystemTime::now()) {
        let size = size_of(&path);
        unused_size += size;
        if path.starts_with(builds) {
            unused_builds += size;
        }
    }
    let builds_size = size_of(&builds.join("targets"))
        + build_folders(builds)
            .iter()
            .map(|p| size_of(p))
            .sum::<u64>();
    StorageUsage {
        sdk: size_of(&installer.active_sdk_dir()),
        rollback_sdk: previous.is_dir().then(|| size_of(&previous)),
        build_cache: builds_size.saturating_sub(unused_builds),
        unused: unused_size,
    }
}

/// Removes the build caches no installed SDK (nor `keep`) uses, build folders a crashed
/// Studio left, and SDK folders an interrupted removal left. Returns the bytes freed; a
/// folder that cannot be removed now (a file in use) is skipped and tried next time.
pub fn clean(sdk_home: &Path, builds: &Path, keep: Option<&CompatibilityManifest>) -> u64 {
    unused(sdk_home, builds, keep, SystemTime::now())
        .into_iter()
        .map(|path| {
            let size = size_of(&path);
            fs::remove_dir_all(&path).map_or(0, |()| size)
        })
        .sum()
}

/// The folders [`clean`] removes at `now`.
fn unused(
    sdk_home: &Path,
    builds: &Path,
    keep: Option<&CompatibilityManifest>,
    now: SystemTime,
) -> Vec<PathBuf> {
    let older_than = |path: &Path, age: Duration| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|d| d >= age))
    };
    let used: HashSet<String> = crate::sdk_setup::installed_sdks(sdk_home)
        .into_iter()
        .map(|(_, manifest)| manifest.digest())
        .chain(keep.map(CompatibilityManifest::digest))
        .collect();
    let mut unused: Vec<PathBuf> = dirs(&builds.join("targets"))
        .filter(|(path, name)| !used.contains(name) && older_than(path, NEW_TARGET_GRACE))
        .map(|(path, _)| path)
        .collect();
    unused.extend(
        build_folders(builds)
            .into_iter()
            .filter(|path| older_than(path, STALE_BUILD_AGE)),
    );
    unused.extend(
        dirs(sdk_home)
            .filter(|(_, name)| name.starts_with(studio_sdk::install::REMOVED_SDK_PREFIX))
            .map(|(path, _)| path),
    );
    unused
}

/// The build folders `materialize` creates under `builds`.
fn build_folders(builds: &Path) -> Vec<PathBuf> {
    dirs(builds)
        .filter(|(_, name)| name.starts_with("build-"))
        .map(|(path, _)| path)
        .collect()
}

/// The subfolders of `dir` with their names (never following links).
fn dirs(dir: &Path) -> impl Iterator<Item = (PathBuf, String)> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir()).then(|| {
                (
                    entry.path(),
                    entry.file_name().to_string_lossy().into_owned(),
                )
            })
        })
}

/// The bytes of the files under `path`, not following links.
fn size_of(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| size_of(&entry.path()))
        .sum()
}

/// `bytes` for people: "1.4 GB", "820 MB".
pub fn format_size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(sdk_id: &str) -> CompatibilityManifest {
        let mut manifest = CompatibilityManifest::default_linux_x64();
        manifest.sdk_id = sdk_id.into();
        manifest.target_triple = CompatibilityManifest::host_target()
            .expect("supported host")
            .into();
        manifest
    }

    fn install(sdk_home: &Path, slot: &str, manifest: &CompatibilityManifest) {
        let dir = sdk_home.join(slot);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("compatibility.json"),
            serde_json::to_string(manifest).unwrap(),
        )
        .unwrap();
    }

    fn cache(builds: &Path, name: &str, bytes: usize) -> PathBuf {
        let dir = builds.join(name);
        fs::create_dir_all(dir.join("debug")).unwrap();
        fs::write(dir.join("debug/artifact"), vec![0; bytes]).unwrap();
        dir
    }

    #[test]
    fn keeps_the_caches_of_installed_and_open_sdks_and_removes_the_rest() {
        let temp = tempfile::tempdir().unwrap();
        let (sdk_home, builds) = (temp.path().join("sdk"), temp.path().join("builds"));
        let (active, previous, open, replaced) = (
            manifest("active"),
            manifest("previous"),
            manifest("open"),
            manifest("replaced"),
        );
        install(&sdk_home, "active", &active);
        install(&sdk_home, "previous", &previous);
        let target = |m: &CompatibilityManifest| format!("targets/{}", m.digest());
        let kept = [
            cache(&builds, &target(&active), 10),
            cache(&builds, &target(&previous), 20),
            cache(&builds, &target(&open), 30),
        ];
        let removed = cache(&builds, &target(&replaced), 400);
        let stale_build = cache(&builds, "build-old", 5000);
        let interrupted = cache(&sdk_home, "previous.removed.1", 60000);

        // Ages are measured against `now`: a day later every folder is old enough.
        let later = SystemTime::now() + STALE_BUILD_AGE;
        let mut found = unused(&sdk_home, &builds, Some(&open), later);
        found.sort();
        let mut expected = vec![removed.clone(), stale_build.clone(), interrupted.clone()];
        expected.sort();
        assert_eq!(found, expected);
        // Just now, the replaced SDK's new-looking target and the fresh build stay.
        assert_eq!(
            unused(&sdk_home, &builds, Some(&open), SystemTime::now()),
            vec![interrupted.clone()]
        );

        let usage = measure(&sdk_home, &builds, Some(&open));
        assert_eq!(usage.build_cache, 10 + 20 + 30 + 400 + 5000);
        assert_eq!(usage.unused, 60000);
        assert!(usage.rollback_sdk.is_some());

        assert_eq!(clean(&sdk_home, &builds, Some(&open)), 60000);
        assert!(!interrupted.exists());
        assert!(
            kept.iter()
                .chain([&removed, &stale_build])
                .all(|p| p.exists())
        );
    }

    #[test]
    fn sizes_read_like_the_file_manager() {
        assert_eq!(format_size(820 * 1024 * 1024), "820 MB");
        assert_eq!(format_size(1536 * 1024 * 1024), "1.5 GB");
    }
}
