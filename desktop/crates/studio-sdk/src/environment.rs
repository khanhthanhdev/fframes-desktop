use crate::manifest::CompatibilityManifest;
use std::path::{Path, PathBuf};
use studio_bootstrap::ChildEnvironment;

pub struct SdkEnvironment {
    pub sdk_dir: PathBuf,
    pub target_dir: PathBuf,
    pub offline: bool,
    pub manifest: CompatibilityManifest,
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
        }
    }

    /// Builds a strictly isolated `ChildEnvironment` for project builds and renders.
    pub fn build_child_environment(&self) -> ChildEnvironment {
        let mut env = ChildEnvironment::default_allowlist();

        // 1. App-managed toolchain and cargo homes
        let rustup_home = self.sdk_dir.join("rustup");
        let cargo_home = self.sdk_dir.join("cargo");
        env.set("RUSTUP_HOME", rustup_home.to_string_lossy());
        env.set("CARGO_HOME", cargo_home.to_string_lossy());
        env.set("CARGO_TARGET_DIR", self.target_dir.to_string_lossy());
        env.set("RUSTUP_TOOLCHAIN", &self.manifest.rust_toolchain.channel);

        // 2. FFmpeg configuration
        let ffmpeg_dir = self.sdk_dir.join("ffmpeg");
        let ffmpeg_cache = ffmpeg_dir.join("cache");
        env.set("FFMPEG_DIR", ffmpeg_dir.to_string_lossy());
        env.set("FFMPEG_BINARIES_CACHE", ffmpeg_cache.to_string_lossy());

        // 3. Libclang resolution
        if env.get("LIBCLANG_PATH").is_none()
            && let Some(libclang) = std::env::var("LIBCLANG_PATH")
                .ok()
                .filter(|p| Path::new(p).exists())
        {
            env.set("LIBCLANG_PATH", libclang);
        }

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
            env.prepend_path(&toolchain_bin);
        }

        // 5. Windows DLL path setup
        #[cfg(windows)]
        {
            if let Some(ref bin_rel) = self.manifest.ffmpeg.bin_rel_path {
                let bin_dir = self.sdk_dir.join(bin_rel);
                if bin_dir.exists() {
                    env.prepend_path(&bin_dir);
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
}
