use crate::manifest::CompatibilityManifest;
use std::path::{Path, PathBuf};
use studio_bootstrap::ChildEnvironment;

fn child_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    {
        normalize_windows_verbatim_prefix(&path).into_owned()
    }
    #[cfg(not(windows))]
    {
        path.into_owned()
    }
}

#[cfg(any(windows, test))]
fn normalize_windows_verbatim_prefix(path: &str) -> std::borrow::Cow<'_, str> {
    if let Some(unc_path) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc_path}").into();
    }
    if let Some(drive_path) = path.strip_prefix(r"\\?\")
        && drive_path.as_bytes().get(1) == Some(&b':')
    {
        return drive_path.into();
    }
    path.into()
}

#[derive(Debug, Clone)]
pub struct SdkEnvironment {
    pub sdk_dir: PathBuf,
    pub target_dir: PathBuf,
    pub offline: bool,
    pub manifest: CompatibilityManifest,
    /// When set, the exact environment resolved once; children never re-read the host.
    frozen: Option<ChildEnvironment>,
}

impl SdkEnvironment {
    pub fn new(
        sdk_dir: impl Into<PathBuf>,
        target_dir: impl Into<PathBuf>,
        manifest: CompatibilityManifest,
        offline: bool,
    ) -> Self {
        Self {
            sdk_dir: sdk_dir.into(),
            target_dir: target_dir.into(),
            offline,
            manifest,
            frozen: None,
        }
    }

    /// Resolve the child environment from `host` once and keep it: every later
    /// `build_child_environment` returns exactly these variables, whatever the
    /// process environment becomes.
    pub fn freeze(mut self, host: ChildEnvironment) -> Self {
        self.frozen = Some(self.resolve_child_environment(host));
        self
    }

    /// The frozen environment, if this environment was frozen.
    pub fn frozen_environment(&self) -> Option<&ChildEnvironment> {
        self.frozen.as_ref()
    }

    /// Builds a strictly isolated `ChildEnvironment` for project builds and renders.
    /// A frozen environment returns its snapshot; otherwise the host allowlist is read now.
    pub fn build_child_environment(&self) -> ChildEnvironment {
        match &self.frozen {
            Some(frozen) => frozen.clone(),
            None => self.resolve_child_environment(ChildEnvironment::default_allowlist()),
        }
    }

    /// Applies the SDK-honored rules (toolchain/cargo homes, FFmpeg, libclang, PATH,
    /// offline mode, leak stripping) on top of the allowlisted `host` variables.
    pub fn resolve_child_environment(&self, host: ChildEnvironment) -> ChildEnvironment {
        let mut env = host;

        // 1. App-managed toolchain and cargo homes
        let rustup_home = self.sdk_dir.join("rustup");
        let cargo_home = self.sdk_dir.join("cargo");
        env.set("RUSTUP_HOME", child_path(&rustup_home));
        env.set("CARGO_HOME", child_path(&cargo_home));
        env.set("CARGO_TARGET_DIR", child_path(&self.target_dir));
        env.set("RUSTUP_TOOLCHAIN", &self.manifest.rust_toolchain.channel);

        // 2. FFmpeg configuration
        let ffmpeg_dir = self.sdk_dir.join("ffmpeg");
        let ffmpeg_cache = ffmpeg_dir.join("cache");
        env.set("FFMPEG_DIR", child_path(&ffmpeg_dir));
        env.set("FFMPEG_BINARIES_CACHE", child_path(&ffmpeg_cache));

        // 3. Libclang resolution (the host allowlist already carries LIBCLANG_PATH)
        if env.get("LIBCLANG_PATH").is_none() {
            #[cfg(target_os = "macos")]
            {
                for candidate in [
                    "/opt/homebrew/opt/llvm/lib",
                    "/opt/homebrew/lib",
                    "/Library/Developer/CommandLineTools/usr/lib",
                    "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib",
                    "/usr/local/opt/llvm/lib",
                ] {
                    if Path::new(candidate).exists() {
                        env.set("LIBCLANG_PATH", candidate);
                        break;
                    }
                }
            }

            #[cfg(target_os = "linux")]
            {
                for candidate in [
                    "/usr/lib/llvm-19/lib",
                    "/usr/lib/llvm-18/lib",
                    "/usr/lib/llvm-17/lib",
                    "/usr/lib/llvm-16/lib",
                    "/usr/lib/x86_64-linux-gnu",
                    "/usr/lib/aarch64-linux-gnu",
                    "/usr/local/lib",
                ] {
                    if Path::new(candidate).exists() {
                        env.set("LIBCLANG_PATH", candidate);
                        break;
                    }
                }
            }

            #[cfg(windows)]
            {
                for candidate in [r"C:\Program Files\LLVM\bin", r"C:\Program Files\LLVM\lib"] {
                    if Path::new(candidate).exists() {
                        env.set("LIBCLANG_PATH", candidate);
                        break;
                    }
                }
            }
        }

        // 4. Prepend toolchain bin to PATH
        let toolchain_bin = self.sdk_dir.join("toolchain").join("bin");
        if toolchain_bin.exists() {
            env.prepend_path(child_path(&toolchain_bin));
        }

        // 5. Windows DLL path setup
        #[cfg(windows)]
        {
            if let Some(ref bin_rel) = self.manifest.ffmpeg.bin_rel_path {
                let bin_dir = self.sdk_dir.join(bin_rel);
                if bin_dir.exists() {
                    env.prepend_path(child_path(&bin_dir));
                }
            }
        }

        // 6. Network offline mode
        if self.offline {
            env.set("CARGO_NET_OFFLINE", "true");
        }

        // 7. Strip developer ambient variables that might cause leakage
        for leak_var in [
            "RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
            "RUSTC_WRAPPER",
            "CARGO_CONFIG",
            "CARGO_HOME_OVERRIDE",
        ] {
            env.remove(leak_var);
        }

        env
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_verbatim_paths_are_normalized_for_external_toolchains() {
        assert_eq!(
            normalize_windows_verbatim_prefix(r"\\?\C:\sdk\ffmpeg"),
            r"C:\sdk\ffmpeg"
        );
        assert_eq!(
            normalize_windows_verbatim_prefix(r"\\?\UNC\server\share\sdk"),
            r"\\server\share\sdk"
        );
        assert_eq!(
            normalize_windows_verbatim_prefix(r"C:\sdk\ffmpeg"),
            r"C:\sdk\ffmpeg"
        );
    }

    #[test]
    fn test_sdk_environment_construction() {
        let manifest = CompatibilityManifest::default_linux_x64();
        let sdk_env =
            SdkEnvironment::new("/opt/fframes/sdk", "/tmp/project_target", manifest, true);
        let child_env = sdk_env.build_child_environment();

        assert_eq!(child_env.get("CARGO_NET_OFFLINE"), Some("true"));
        assert_eq!(
            child_env.get("CARGO_TARGET_DIR"),
            Some("/tmp/project_target")
        );
        assert_eq!(child_env.get("RUSTUP_TOOLCHAIN"), Some("1.98.1"));
        assert!(child_env.get("FFMPEG_DIR").is_some());
    }

    #[cfg(windows)]
    #[test]
    fn child_environment_uses_normal_paths_for_verbatim_windows_sdk_roots() {
        let temp = tempfile::tempdir().unwrap();
        let sdk = temp.path().join("sdk");
        std::fs::create_dir_all(sdk.join("toolchain/bin")).unwrap();
        let sdk = std::fs::canonicalize(sdk).unwrap();
        let builds = temp.path().join("builds");
        std::fs::create_dir(&builds).unwrap();
        let builds = std::fs::canonicalize(builds).unwrap();
        let expected_sdk = normalize_windows_verbatim_prefix(&sdk.to_string_lossy()).into_owned();
        let expected_builds =
            normalize_windows_verbatim_prefix(&builds.to_string_lossy()).into_owned();
        let expected_ffmpeg = format!(r"{}\ffmpeg", expected_sdk);
        let expected_target = expected_builds.as_str();
        let expected_toolchain = format!(r"{}\toolchain\bin", expected_sdk);

        let sdk_env = SdkEnvironment::new(
            sdk,
            builds,
            CompatibilityManifest::default_linux_x64(),
            true,
        );
        let child_env = sdk_env.resolve_child_environment(ChildEnvironment::empty());

        assert_eq!(child_env.get("FFMPEG_DIR"), Some(expected_ffmpeg.as_str()));
        assert_eq!(child_env.get("CARGO_TARGET_DIR"), Some(expected_target));
        assert_eq!(child_env.get("PATH"), Some(expected_toolchain.as_str()));
    }
}
