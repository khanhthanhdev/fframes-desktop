use crate::manifest::CompatibilityManifest;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DoctorStage {
    HostPreflight,
    CandidateSdkVerify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeStatus {
    Pass,
    Warning,
    Installable, // Missing but app-managed (not a host blocker)
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorItem {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: ProbeStatus,
    pub observed_version_or_path: Option<String>,
    pub package_to_install: Option<String>,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub stage: DoctorStage,
    pub target_triple: String,
    pub items: Vec<DoctorItem>,
    pub overall_status: ProbeStatus,
    pub missing_system_packages: Vec<String>,
}

impl DoctorReport {
    pub fn is_ready(&self) -> bool {
        self.overall_status == ProbeStatus::Pass
    }

    pub fn format_summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Doctor Report [{:?}] for {}\n",
            self.stage, self.target_triple
        ));
        out.push_str(&format!("Overall Status: {:?}\n\n", self.overall_status));

        for item in &self.items {
            let symbol = match item.status {
                ProbeStatus::Pass => "[OK]",
                ProbeStatus::Warning => "[WARN]",
                ProbeStatus::Installable => "[INSTALLABLE]",
                ProbeStatus::Fail => "[FAIL]",
            };
            out.push_str(&format!("{} {}: ", symbol, item.name));
            if let Some(ref val) = item.observed_version_or_path {
                out.push_str(val);
            } else if let Some(ref reason) = item.failure_reason {
                out.push_str(&format!("FAILED ({reason})"));
            }
            if let Some(ref pkg) = item.package_to_install {
                out.push_str(&format!(" -> Install: {pkg}"));
            }
            out.push('\n');
        }

        if !self.missing_system_packages.is_empty() {
            out.push_str(&format!(
                "\nRequired system packages to install (reviewed by user):\n  {}\n",
                install_hint(&self.missing_system_packages)
            ));
        }

        out
    }
}

/// How the user installs missing host prerequisites on this platform.
/// A probe process that opens no console window when the doctor runs inside the GUI app.
fn probe_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

pub fn install_hint(packages: &[String]) -> String {
    if cfg!(windows) {
        format!(
            "Install with the Visual Studio Installer or LLVM setup: {}",
            packages.join(", ")
        )
    } else {
        format!("sudo apt-get install -y {}", packages.join(" "))
    }
}

const WINDOWS_MSVC_PACKAGE: &str =
    "Visual Studio Build Tools with the \"Desktop development with C++\" workload";
const WINDOWS_LLVM_PACKAGE: &str =
    "LLVM (libclang) from https://github.com/llvm/llvm-project/releases";

/// The installation path of a Visual Studio instance with the x64 MSVC tools, found the way
/// rustc finds the linker (the Visual Studio setup registry via vswhere), not through PATH:
/// `cl.exe` is only on PATH inside a developer prompt.
fn windows_msvc_installation() -> Option<String> {
    let program_files =
        std::env::var_os("ProgramFiles(x86)").or_else(|| std::env::var_os("ProgramFiles"))?;
    let vswhere = Path::new(&program_files).join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    let output = probe_command(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !path.is_empty()).then_some(path)
}

/// libclang for bindgen, resolved the same way `SdkEnvironment` does on Windows.
fn windows_libclang() -> Option<String> {
    std::env::var("LIBCLANG_PATH")
        .ok()
        .into_iter()
        .chain([r"C:\Program Files\LLVM\bin".to_owned()])
        .find(|dir| Path::new(dir).join("libclang.dll").is_file())
}

fn found_or_missing(
    id: &str,
    name: &str,
    description: &str,
    found: Option<String>,
    package: &str,
) -> DoctorItem {
    DoctorItem {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        status: if found.is_some() {
            ProbeStatus::Pass
        } else {
            ProbeStatus::Fail
        },
        failure_reason: found.is_none().then(|| format!("{name} was not found")),
        package_to_install: found.is_none().then(|| package.into()),
        observed_version_or_path: found,
    }
}

pub struct Doctor;

impl Doctor {
    pub fn run_host_preflight(manifest: &CompatibilityManifest) -> DoctorReport {
        let mut items = Vec::new();
        let target_triple = manifest.target_triple.clone();

        // 1. Probe Host Architecture & Target
        let host_triple = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
            ("windows", "x86_64") => "x86_64-pc-windows-msvc",
            ("macos", "aarch64") => "aarch64-apple-darwin",
            (_os, _arch) => "unknown",
        };

        if host_triple == target_triple {
            items.push(DoctorItem {
                id: "target_arch".into(),
                name: "Host Architecture & OS Baseline".into(),
                description: format!("Target matches host: {target_triple}"),
                status: ProbeStatus::Pass,
                observed_version_or_path: Some(format!("Host is {host_triple}")),
                package_to_install: None,
                failure_reason: None,
            });
        } else {
            items.push(DoctorItem {
                id: "target_arch".into(),
                name: "Host Architecture & OS Baseline".into(),
                description: format!("Expected {target_triple}, found {host_triple}"),
                status: ProbeStatus::Fail,
                observed_version_or_path: Some(host_triple.into()),
                package_to_install: None,
                failure_reason: Some(format!(
                    "Architecture mismatch: {host_triple} vs {target_triple}"
                )),
            });
        }

        if cfg!(windows) {
            // clang/gcc, CMake and pkg-config are Unix build prerequisites. Windows SDK
            // builds link with MSVC and run bindgen against libclang.
            items.push(found_or_missing(
                "msvc",
                "MSVC compiler",
                "Visual Studio C++ tools found through the Visual Studio setup registry",
                windows_msvc_installation(),
                WINDOWS_MSVC_PACKAGE,
            ));
            items.push(found_or_missing(
                "libclang",
                "libclang",
                "bindgen for the FFmpeg bindings",
                windows_libclang(),
                WINDOWS_LLVM_PACKAGE,
            ));
        } else {
            // 2. Probe Compiler (clang or gcc)
            let compiler_output = probe_command("clang")
                .arg("--version")
                .output()
                .or_else(|_| probe_command("gcc").arg("--version").output());

            match compiler_output {
                Ok(out) if out.status.success() => {
                    let first_line = String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .next()
                        .unwrap_or("compiler found")
                        .to_string();
                    items.push(DoctorItem {
                        id: "c_compiler".into(),
                        name: "C/C++ Compiler".into(),
                        description: "Native build-script and bindgen compiler".into(),
                        status: ProbeStatus::Pass,
                        observed_version_or_path: Some(first_line),
                        package_to_install: None,
                        failure_reason: None,
                    });
                }
                _ => {
                    items.push(DoctorItem {
                        id: "c_compiler".into(),
                        name: "C/C++ Compiler".into(),
                        description: "Native build-script and bindgen compiler".into(),
                        status: ProbeStatus::Fail,
                        observed_version_or_path: None,
                        package_to_install: Some("clang build-essential".into()),
                        failure_reason: Some("No C compiler found on PATH".into()),
                    });
                }
            }

            // 3. Probe CMake
            let cmake_output = probe_command("cmake").arg("--version").output();
            match cmake_output {
                Ok(out) if out.status.success() => {
                    let line = String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .next()
                        .unwrap_or("cmake found")
                        .to_string();
                    items.push(DoctorItem {
                        id: "cmake".into(),
                        name: "CMake Build System".into(),
                        description: "Native build tool".into(),
                        status: ProbeStatus::Pass,
                        observed_version_or_path: Some(line),
                        package_to_install: None,
                        failure_reason: None,
                    });
                }
                _ => {
                    items.push(DoctorItem {
                        id: "cmake".into(),
                        name: "CMake Build System".into(),
                        description: "Native build tool".into(),
                        status: ProbeStatus::Fail,
                        observed_version_or_path: None,
                        package_to_install: Some("cmake".into()),
                        failure_reason: Some("cmake not found on PATH".into()),
                    });
                }
            }

            // 4. Probe Pkg-Config
            let pkg_config_output = probe_command("pkg-config").arg("--version").output();
            match pkg_config_output {
                Ok(out) if out.status.success() => {
                    let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    items.push(DoctorItem {
                        id: "pkg_config".into(),
                        name: "pkg-config".into(),
                        description: "Library locator".into(),
                        status: ProbeStatus::Pass,
                        observed_version_or_path: Some(format!("pkg-config {ver}")),
                        package_to_install: None,
                        failure_reason: None,
                    });
                }
                _ => {
                    items.push(DoctorItem {
                        id: "pkg_config".into(),
                        name: "pkg-config".into(),
                        description: "Library locator".into(),
                        status: ProbeStatus::Fail,
                        observed_version_or_path: None,
                        package_to_install: Some("pkg-config".into()),
                        failure_reason: Some("pkg-config not found on PATH".into()),
                    });
                }
            }
        }

        // 5. Run manifest-specified host probes
        for probe in &manifest.host_prerequisites {
            // `cl.exe` is only on PATH inside a developer prompt; the MSVC probe above
            // answers it through the Visual Studio setup registry instead.
            if cfg!(windows) && matches!(probe.command.as_str(), "cl" | "cl.exe") {
                continue;
            }
            let res = probe_command(&probe.command).args(&probe.args).output();
            match res {
                Ok(out) if out.status.success() => {
                    items.push(DoctorItem {
                        id: probe.id.clone(),
                        name: probe.name.clone(),
                        description: probe.description.clone(),
                        status: ProbeStatus::Pass,
                        observed_version_or_path: Some("satisfied".into()),
                        package_to_install: None,
                        failure_reason: None,
                    });
                }
                _ => {
                    let status = if probe.required {
                        ProbeStatus::Fail
                    } else {
                        ProbeStatus::Warning
                    };
                    items.push(DoctorItem {
                        id: probe.id.clone(),
                        name: probe.name.clone(),
                        description: probe.description.clone(),
                        status,
                        observed_version_or_path: None,
                        package_to_install: Some(probe.package_name.clone()),
                        failure_reason: Some(format!("Check failed for '{}'", probe.command)),
                    });
                }
            }
        }

        // 6. Check App-managed Artifacts (classified as Installable, NOT host blockers)
        items.push(DoctorItem {
            id: "app_sdk_artifacts".into(),
            name: "App-Managed SDK Artifacts".into(),
            description: "Managed toolchain and static FFmpeg libraries".into(),
            status: ProbeStatus::Installable,
            observed_version_or_path: Some("Ready for transactional installation".into()),
            package_to_install: None,
            failure_reason: None,
        });

        // Compute overall status and missing package list
        let mut missing_system_packages = Vec::new();
        let mut has_failure = false;
        let mut has_warning = false;

        for item in &items {
            if item.status == ProbeStatus::Fail {
                has_failure = true;
                if let Some(ref pkg) = item.package_to_install {
                    missing_system_packages.push(pkg.clone());
                }
            } else if item.status == ProbeStatus::Warning {
                has_warning = true;
            }
        }

        let overall_status = if has_failure {
            ProbeStatus::Fail
        } else if has_warning {
            ProbeStatus::Warning
        } else {
            ProbeStatus::Pass
        };

        DoctorReport {
            stage: DoctorStage::HostPreflight,
            target_triple,
            items,
            overall_status,
            missing_system_packages,
        }
    }

    pub fn verify_candidate_sdk(sdk_root: &Path, manifest: &CompatibilityManifest) -> DoctorReport {
        Self::verify_candidate_sdk_with_processes(sdk_root, manifest, None)
    }

    pub fn verify_candidate_sdk_with_processes(
        sdk_root: &Path,
        manifest: &CompatibilityManifest,
        processes: Option<&studio_bootstrap::ProcessTreeManager>,
    ) -> DoctorReport {
        let mut items = Vec::new();
        let target_triple = manifest.target_triple.clone();

        // 1. Verify Rust toolchain executable
        let rustc_path = sdk_root
            .join("toolchain")
            .join("bin")
            .join(if cfg!(windows) { "rustc.exe" } else { "rustc" });

        if !rustc_path.exists() {
            items.push(DoctorItem {
                id: "sdk_rustc".into(),
                name: "SDK Rust Compiler".into(),
                description: "Rust compiler for project building".into(),
                status: ProbeStatus::Fail,
                observed_version_or_path: None,
                package_to_install: None,
                failure_reason: Some(format!(
                    "bundled rustc executable not found inside candidate SDK at {:?}",
                    rustc_path
                )),
            });
        } else {
            let probe_rustc = probe_bundled_compiler(&rustc_path, "--version", processes);
            match probe_rustc {
                Ok(out) if out.status.success() => {
                    let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !compiler_version_matches(&ver, &manifest.rust_toolchain.channel) {
                        items.push(DoctorItem {
                            id: "sdk_rustc".into(),
                            name: "SDK Rust Compiler".into(),
                            description: "Rust compiler for project building".into(),
                            status: ProbeStatus::Fail,
                            observed_version_or_path: Some(ver.clone()),
                            package_to_install: None,
                            failure_reason: Some(format!(
                                "compiler version mismatch: expected channel '{}', observed '{}'",
                                manifest.rust_toolchain.channel, ver
                            )),
                        });
                    } else {
                        let verbose_out = probe_bundled_compiler(&rustc_path, "-vV", processes);
                        let target_ok = match verbose_out {
                            Ok(v_out) if v_out.status.success() => {
                                let v_str = String::from_utf8_lossy(&v_out.stdout);
                                v_str.lines().any(|line| {
                                    line.strip_prefix("host: ")
                                        == Some(manifest.target_triple.as_str())
                                })
                            }
                            _ => false,
                        };
                        if !target_ok {
                            items.push(DoctorItem {
                                id: "sdk_rustc".into(),
                                name: "SDK Rust Compiler".into(),
                                description: "Rust compiler for project building".into(),
                                status: ProbeStatus::Fail,
                                observed_version_or_path: Some(ver),
                                package_to_install: None,
                                failure_reason: Some(format!(
                                    "compiler target mismatch: expected host '{}'",
                                    manifest.target_triple
                                )),
                            });
                        } else {
                            items.push(DoctorItem {
                                id: "sdk_rustc".into(),
                                name: "SDK Rust Compiler".into(),
                                description: "Rust compiler for project building".into(),
                                status: ProbeStatus::Pass,
                                observed_version_or_path: Some(ver),
                                package_to_install: None,
                                failure_reason: None,
                            });
                        }
                    }
                }
                Ok(out) => {
                    items.push(DoctorItem {
                        id: "sdk_rustc".into(),
                        name: "SDK Rust Compiler".into(),
                        description: "Rust compiler for project building".into(),
                        status: ProbeStatus::Fail,
                        observed_version_or_path: None,
                        package_to_install: None,
                        failure_reason: Some(format!(
                            "rustc failed with exit code {:?}: {}",
                            out.status.code(),
                            String::from_utf8_lossy(&out.stderr)
                        )),
                    });
                }
                Err(err) => {
                    items.push(DoctorItem {
                        id: "sdk_rustc".into(),
                        name: "SDK Rust Compiler".into(),
                        description: "Rust compiler for project building".into(),
                        status: ProbeStatus::Fail,
                        observed_version_or_path: None,
                        package_to_install: None,
                        failure_reason: Some(format!("failed to execute candidate rustc: {err}")),
                    });
                }
            }
        }
        // 2. Verify FFmpeg Headers & Libs
        let ffmpeg_headers = sdk_root.join(&manifest.ffmpeg.headers_rel_path);
        let ffmpeg_libs = sdk_root.join(&manifest.ffmpeg.libs_rel_path);

        if ffmpeg_headers.exists() && ffmpeg_libs.exists() {
            items.push(DoctorItem {
                id: "sdk_ffmpeg".into(),
                name: "FFmpeg 9 Headers and Libraries".into(),
                description: format!(
                    "Layout: {} (headers), {} (libs)",
                    manifest.ffmpeg.headers_rel_path, manifest.ffmpeg.libs_rel_path
                ),
                status: ProbeStatus::Pass,
                observed_version_or_path: Some(format!(
                    "Headers: {:?}, Libs: {:?}",
                    ffmpeg_headers, ffmpeg_libs
                )),
                package_to_install: None,
                failure_reason: None,
            });
        } else {
            items.push(DoctorItem {
                id: "sdk_ffmpeg".into(),
                name: "FFmpeg 9 Headers and Libraries".into(),
                description: "FFmpeg development files".into(),
                status: ProbeStatus::Fail,
                observed_version_or_path: None,
                package_to_install: None,
                failure_reason: Some(format!(
                    "Missing headers ({:?}) or libs ({:?})",
                    ffmpeg_headers, ffmpeg_libs
                )),
            });
        }

        let has_failure = items.iter().any(|i| i.status == ProbeStatus::Fail);
        let overall_status = if has_failure {
            ProbeStatus::Fail
        } else {
            ProbeStatus::Pass
        };

        DoctorReport {
            stage: DoctorStage::CandidateSdkVerify,
            target_triple,
            items,
            overall_status,
            missing_system_packages: Vec::new(),
        }
    }
}

fn compiler_version_matches(version: &str, channel: &str) -> bool {
    let mut fields = version.split_whitespace();
    fields.next() == Some("rustc") && fields.next() == Some(channel)
}

fn probe_bundled_compiler(
    path: &Path,
    argument: &str,
    processes: Option<&studio_bootstrap::ProcessTreeManager>,
) -> std::io::Result<std::process::Output> {
    let fallback;
    let manager = match processes {
        Some(p) => p.sub_manager(),
        None => {
            fallback = studio_bootstrap::ProcessTreeManager::new();
            fallback.sub_manager()
        }
    };
    let mut options = studio_bootstrap::SpawnOptions::new(path);
    options.arg(argument);
    options.stdout = std::process::Stdio::piped();
    options.stderr = std::process::Stdio::piped();
    let child = manager.spawn(options).map_err(std::io::Error::other)?;
    let result = crate::project::wait_with_output_drained(&child, 64 * 1024)
        .map(|(status, stdout, stderr)| std::process::Output {
            status,
            stdout,
            stderr,
        })
        .map_err(std::io::Error::other);
    manager.terminate_all(std::time::Duration::from_millis(300));
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn compiler_version_pin_does_not_accept_prefix_matches() {
        assert!(super::compiler_version_matches(
            "rustc 1.98.1 (hash date)",
            "1.98.1"
        ));
        assert!(!super::compiler_version_matches(
            "rustc 1.98.10 (hash date)",
            "1.98.1"
        ));
        assert!(!super::compiler_version_matches(
            "rustc 1.98.1-beta (hash date)",
            "1.98.1"
        ));
    }
    use super::*;

    #[test]
    fn install_hints_name_this_platforms_installer() {
        let hint = install_hint(&["a".into(), "b".into()]);
        if cfg!(windows) {
            assert!(!hint.contains("apt-get"), "{hint}");
            assert!(hint.contains("Visual Studio Installer"), "{hint}");
        } else {
            assert_eq!(hint, "sudo apt-get install -y a b");
        }
    }

    /// Windows never probes Unix-only tools, and the SDK manifest's `cl.exe` probe is
    /// answered by MSVC discovery instead of PATH.
    #[cfg(windows)]
    #[test]
    fn windows_preflight_skips_unix_tools_and_path_bound_cl() {
        let mut manifest = CompatibilityManifest::default_linux_x64();
        manifest.target_triple = "x86_64-pc-windows-msvc".into();
        manifest.host_prerequisites = vec![crate::manifest::HostPrerequisiteProbe {
            id: "msvc".into(),
            name: "MSVC compiler".into(),
            description: "Visual Studio native build environment".into(),
            command: "cl.exe".into(),
            args: vec![],
            package_name: "Visual Studio C++ Build Tools".into(),
            required: true,
        }];
        let report = Doctor::run_host_preflight(&manifest);
        let ids: Vec<_> = report.items.iter().map(|item| item.id.as_str()).collect();
        for unix_only in ["c_compiler", "cmake", "pkg_config"] {
            assert!(!ids.contains(&unix_only), "{ids:?}");
        }
        assert_eq!(ids.iter().filter(|id| **id == "msvc").count(), 1, "{ids:?}");
    }

    #[test]
    fn test_host_preflight_on_current_machine() {
        let manifest = CompatibilityManifest::default_linux_x64();
        let report = Doctor::run_host_preflight(&manifest);
        assert_eq!(report.stage, DoctorStage::HostPreflight);
        assert!(!report.items.is_empty());
        println!("{}", report.format_summary());
    }
}
