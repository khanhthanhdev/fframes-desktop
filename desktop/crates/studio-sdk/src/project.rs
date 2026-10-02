use crate::environment::SdkEnvironment;
use parking_lot::Mutex;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use studio_bootstrap::{ProcessError, ProcessTreeManager, SpawnOptions, TrackedChild};
use thiserror::Error;
#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("destination '{0}' already exists and is not empty")]
    DestinationAlreadyExists(PathBuf),
    #[error("process error during {operation}: {source}")]
    ProcessFailed {
        operation: String,
        #[source]
        source: ProcessError,
    },
    #[error("command '{command}' failed with exit code: {exit_code:?}, stderr: {stderr}")]
    CommandExecutionFailed {
        command: String,
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("project build error: {0}")]
    BuildError(String),
}

pub struct ProjectManager;

impl ProjectManager {
    pub fn generate_annotated_worker_project(root: &Path, sdk: &Path) -> Result<(), ProjectError> {
        if root.join("Cargo.toml").exists() {
            return Ok(());
        }
        let framework = sdk.join("framework/framework");
        for name in [
            "fframes",
            "fframes-studio-runtime",
            "fframes-studio-protocol",
        ] {
            if !framework.join(name).join("Cargo.toml").is_file() {
                return Err(ProjectError::BuildError(format!(
                    "SDK is missing {name}; install the complete SDK"
                )));
            }
        }
        fs::create_dir_all(root.join("media"))?;
        fs::write(
            root.join("media/DMSans-Medium.ttf"),
            include_bytes!("../../../fixtures/annotated-video-overlay/media/DMSans-Medium.ttf"),
        )?;
        fs::write(
            root.join("media/OFL.txt"),
            include_str!("../../../fixtures/annotated-video-overlay/media/OFL.txt"),
        )?;
        fs::create_dir_all(root.join("src"))?;
        let path =
            |name: &str| serde_json::to_string(&framework.join(name).to_string_lossy()).unwrap();
        let manifest = format!(
            "[package]\nname = \"studio-annotated-video\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nfframes = {{ path = {}, features = [\"cli\", \"compile-time-svgtree\"] }}\nfframes-studio-runtime = {{ path = {} }}\nfframes-studio-protocol = {{ path = {} }}\nsha2 = \"0.10\"\n",
            path("fframes"),
            path("fframes-studio-runtime"),
            path("fframes-studio-protocol")
        );
        fs::write(root.join("Cargo.toml"), manifest)?;
        fs::write(
            root.join("src/main.rs"),
            include_str!("../../../fixtures/annotated-video-overlay/src/main.rs"),
        )?;
        fs::create_dir_all(root.join(".cargo"))?;
        let vendor =
            serde_json::to_string(&sdk.join("framework/vendor").to_string_lossy()).unwrap();
        fs::write(root.join(".cargo/config.toml"),format!("[source.crates-io]\nreplace-with = \"vendored-sources\"\n[source.vendored-sources]\ndirectory = {vendor}\n")).map_err(|e| ProjectError::BuildError(e.to_string()))?;
        Ok(())
    }

    /// Resolve the lockfile before taking the source snapshot used by a worker.
    pub fn ensure_lockfile(
        project_dir: &Path,
        sdk_env: &SdkEnvironment,
        process_tree: &ProcessTreeManager,
    ) -> Result<(), ProjectError> {
        if project_dir.join("Cargo.lock").is_file() {
            return Ok(());
        }
        let mut options = SpawnOptions {
            env: sdk_env.build_child_environment(),
            ..SpawnOptions::new("cargo")
        };
        options.arg("generate-lockfile");
        options.current_dir(project_dir);
        options.stdout = Stdio::piped();
        options.stderr = Stdio::piped();
        let child = process_tree
            .spawn(options)
            .map_err(|source| ProjectError::ProcessFailed {
                operation: "cargo generate-lockfile".into(),
                source,
            })?;
        let (status, _, stderr) = wait_with_output_drained(&child, 64 * 1024)?;
        if !status.success() {
            return Err(ProjectError::CommandExecutionFailed {
                command: "cargo generate-lockfile".into(),
                exit_code: status.code(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            });
        }
        Ok(())
    }
    /// Generates a standalone CPU fframes project in a temporary staging directory,
    /// and then atomically promotes it to `destination`.
    pub fn generate_cpu_project(
        name: &str,
        destination: &Path,
        fframes_version: &str,
    ) -> Result<PathBuf, ProjectError> {
        Self::generate_cpu_project_with_sdk(name, destination, fframes_version, None)
    }

    pub fn generate_cpu_project_with_sdk(
        name: &str,
        destination: &Path,
        fframes_version: &str,
        sdk_root: Option<&Path>,
    ) -> Result<PathBuf, ProjectError> {
        if destination.exists() {
            let is_empty = destination.read_dir()?.next().is_none();
            if !is_empty {
                return Err(ProjectError::DestinationAlreadyExists(
                    destination.to_path_buf(),
                ));
            }
        }

        let parent_dir = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent_dir)?;

        let tx_id = format!(
            "gen_{}_{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        let staging_dir = parent_dir.join(&tx_id);
        fs::create_dir_all(&staging_dir)?;

        // Determine fframes dependency line
        let fframes_dep = if let Some(sdk) = sdk_root {
            let bundled_fframes = sdk.join("framework").join("framework").join("fframes");
            if bundled_fframes.exists() {
                let bundled_path = serde_json::to_string(&bundled_fframes.to_string_lossy())
                    .map_err(|e| ProjectError::BuildError(e.to_string()))?;
                format!(
                    r#"fframes = {{ path = {bundled_path}, version = "{fframes_version}", features = ["cli", "compile-time-svgtree"] }}"#
                )
            } else {
                format!(
                    r#"fframes = {{ version = "{fframes_version}", features = ["cli", "compile-time-svgtree"] }}"#
                )
            }
        } else {
            format!(
                r#"fframes = {{ version = "{fframes_version}", features = ["cli", "compile-time-svgtree"] }}"#
            )
        };

        // Write Cargo.toml
        let cargo_toml_content = format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[workspace]

[dependencies]
{fframes_dep}
"#
        );
        fs::write(staging_dir.join("Cargo.toml"), cargo_toml_content)?;

        // If SDK has vendor, write .cargo/config.toml
        if let Some(sdk) = sdk_root {
            let vendor_dir = sdk.join("framework").join("vendor");
            if vendor_dir.exists() {
                let vendor_path = serde_json::to_string(&vendor_dir.to_string_lossy())
                    .map_err(|e| ProjectError::BuildError(e.to_string()))?;
                let dot_cargo = staging_dir.join(".cargo");
                fs::create_dir_all(&dot_cargo)?;
                let config_content = format!(
                    r#"[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = {vendor_path}
"#
                );
                fs::write(dot_cargo.join("config.toml"), config_content)?;
            }
        }

        // Write src/main.rs
        let src_dir = staging_dir.join("src");
        fs::create_dir_all(&src_dir)?;
        let main_rs_content = r##"use fframes::{cli, Color, Duration, Frame, RenderOptions, Svgr, Video};

pub struct MyVideo;

impl Video for MyVideo {
    const FPS: usize = 30;
    const WIDTH: usize = 1920;
    const HEIGHT: usize = 1080;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(5.0)
    }
    fn audio(&self) -> fframes::AudioMap<'_> {
        fframes::AudioMap::none()
    }

    fn render_frame<'a>(&'a self, _frame: Frame, _ctx: &fframes::FFramesContext<'a, '_>) -> Svgr<'a> {
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1920 1080" width={Self::WIDTH} height={Self::HEIGHT}>
                <rect x="0" y="0" width={Self::WIDTH} height={Self::HEIGHT} fill="#0d1117" />
                <text x="100" y="300" font-size="120" fill="#ffffff">
                    "fframes studio"
                </text>
            </svg>
        )
    }
}

fn main() -> std::process::ExitCode {
    let video = MyVideo;
    cli::new(&video, RenderOptions::default()).run()
}
"##;
        fs::write(src_dir.join("main.rs"), main_rs_content)?;

        // Atomic promotion
        if destination.exists() {
            let _ = fs::remove_dir_all(destination);
        }
        fs::rename(&staging_dir, destination)?;

        Ok(destination.to_path_buf())
    }

    /// Builds the project using the isolated SDK environment.
    pub fn build_project(
        project_dir: &Path,
        sdk_env: &SdkEnvironment,
        process_tree: &ProcessTreeManager,
    ) -> Result<Duration, ProjectError> {
        let start = Instant::now();
        let mut opts = SpawnOptions::new("cargo");
        opts.arg("build");
        opts.current_dir(project_dir);
        opts.env = sdk_env.build_child_environment();
        opts.stdout = Stdio::piped();
        opts.stderr = Stdio::piped();

        let child_arc = process_tree
            .spawn(opts)
            .map_err(|source| ProjectError::ProcessFailed {
                operation: "cargo build".into(),
                source,
            })?;

        let (status, _stdout, stderr) = wait_with_output_drained(&child_arc, 64 * 1024)?;

        if !status.success() {
            return Err(ProjectError::CommandExecutionFailed {
                command: "cargo build".into(),
                exit_code: status.code(),
                stderr: String::from_utf8_lossy(&stderr).to_string(),
            });
        }

        Ok(start.elapsed())
    }

    /// Builds an explicit worker in an app-local workspace with an external target directory.
    pub fn build_worker_target(
        root: &Path,
        manifest: &Path,
        package: &str,
        worker: &str,
        sdk_env: &SdkEnvironment,
        process_tree: &ProcessTreeManager,
    ) -> Result<Duration, ProjectError> {
        let start = Instant::now();
        let mut options = SpawnOptions::new("cargo");
        for arg in ["build", "--locked", "--offline", "--manifest-path"] {
            options.arg(arg);
        }
        options.arg(manifest);
        options.arg("--package");
        options.arg(package);
        options.arg("--bin");
        options.arg(worker);
        options.current_dir(root);
        options.env = sdk_env.build_child_environment();
        options.stdout = Stdio::piped();
        options.stderr = Stdio::piped();
        let child = process_tree
            .spawn(options)
            .map_err(|source| ProjectError::ProcessFailed {
                operation: "build portable worker".into(),
                source,
            })?;
        let (status, _, stderr) = wait_with_output_drained(&child, 64 * 1024)?;
        if !status.success() {
            return Err(ProjectError::CommandExecutionFailed {
                command: "build portable worker".into(),
                exit_code: status.code(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            });
        }
        Ok(start.elapsed())
    }

    /// Renders a single frame from the project using the CLI `frame` subcommand.
    pub fn render_frame(
        project_dir: &Path,
        sdk_env: &SdkEnvironment,
        frame_index: usize,
        output_dir: &Path,
        process_tree: &ProcessTreeManager,
    ) -> Result<(Duration, Vec<u8>), ProjectError> {
        let start = Instant::now();
        let abs_output_dir = if output_dir.is_absolute() {
            output_dir.to_path_buf()
        } else {
            project_dir.join(output_dir)
        };
        fs::create_dir_all(&abs_output_dir)?;

        let mut opts = SpawnOptions::new("cargo");
        opts.arg("run");
        opts.arg("--");
        opts.arg("frame");
        opts.arg(format!("{frame_index}"));
        opts.arg("-o");
        opts.arg(&abs_output_dir);
        opts.current_dir(project_dir);
        opts.env = sdk_env.build_child_environment();
        opts.stdout = Stdio::piped();
        opts.stderr = Stdio::piped();

        let child_arc = process_tree
            .spawn(opts)
            .map_err(|source| ProjectError::ProcessFailed {
                operation: "cargo run -- frame".into(),
                source,
            })?;

        let (status, _stdout, stderr) = wait_with_output_drained(&child_arc, 64 * 1024)?;

        if !status.success() {
            return Err(ProjectError::CommandExecutionFailed {
                command: "cargo run -- frame".into(),
                exit_code: status.code(),
                stderr: String::from_utf8_lossy(&stderr).to_string(),
            });
        }

        // Locate the created PNG inside abs_output_dir
        let expected_file = abs_output_dir.join(format!("{frame_index}.png"));
        let png_path = if expected_file.exists() {
            expected_file
        } else {
            let mut found = None;
            for entry in fs::read_dir(&abs_output_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("png") {
                    found = Some(path);
                    break;
                }
            }
            found.ok_or_else(|| {
                ProjectError::BuildError(format!(
                    "no frame PNG found in output directory '{}'",
                    abs_output_dir.display()
                ))
            })?
        };

        let bytes = fs::read(&png_path)?;
        Ok((start.elapsed(), bytes))
    }
}

pub(crate) fn wait_with_output_drained(
    child_arc: &Arc<Mutex<TrackedChild>>,
    max_diagnostic_bytes: usize,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), ProjectError> {
    let (stdout_opt, stderr_opt) = {
        let mut child = child_arc.lock();
        (
            child.child_mut().stdout.take(),
            child.child_mut().stderr.take(),
        )
    };

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = stdout_opt {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = r.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if buf.len() < max_diagnostic_bytes {
                    let to_take = n.min(max_diagnostic_bytes - buf.len());
                    buf.extend_from_slice(&chunk[..to_take]);
                }
            }
        }
        buf
    });

    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = stderr_opt {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = r.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if buf.len() < max_diagnostic_bytes {
                    let to_take = n.min(max_diagnostic_bytes - buf.len());
                    buf.extend_from_slice(&chunk[..to_take]);
                }
            }
        }
        buf
    });

    let deadline = Instant::now() + Duration::from_secs(300);
    let status = loop {
        if Instant::now() >= deadline {
            let _ = child_arc.lock().kill_forcefully();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return Err(ProjectError::BuildError(
                "Child exceeded five-minute build/render deadline".into(),
            ));
        }
        let maybe_status = {
            let mut child = child_arc.lock();
            child
                .try_wait()
                .map_err(|source| ProjectError::ProcessFailed {
                    operation: "poll child status".into(),
                    source,
                })?
        };
        if let Some(status) = maybe_status {
            break status;
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    if child_arc.lock().is_alive() {
        let _ = child_arc
            .lock()
            .terminate_gracefully(Duration::from_millis(300));
    }
    let stdout_bytes = stdout_handle.join().unwrap_or_default();
    let stderr_bytes = stderr_handle.join().unwrap_or_default();

    Ok((status, stdout_bytes, stderr_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_cpu_project_creates_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("my-test-video");

        let created_path =
            ProjectManager::generate_cpu_project("my-test-video", &project_dir, "1.1.0")
                .expect("project generation succeeds");

        assert_eq!(created_path, project_dir);
        assert!(project_dir.join("Cargo.toml").exists());
        assert!(project_dir.join("src").join("main.rs").exists());

        let cargo_toml = fs::read_to_string(project_dir.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains("name = \"my-test-video\""));
        assert!(cargo_toml.contains("version = \"1.1.0\""));
        assert!(cargo_toml.contains("features = [\"cli\", \"compile-time-svgtree\"]"));
        assert!(cargo_toml.contains("[workspace]"));
        // Second generation in same non-empty path should be rejected
        let err = ProjectManager::generate_cpu_project("my-test-video", &project_dir, "1.1.0")
            .unwrap_err();
        assert!(matches!(err, ProjectError::DestinationAlreadyExists(_)));
    }
}
