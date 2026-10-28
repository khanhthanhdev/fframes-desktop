//! Make editable, standalone Studio projects from real repository examples.
//! Run with `--out <folder outside the checkout> [--sdk <installed-sdk>] [--check]`.
use std::{error::Error, fs, path::Path, path::PathBuf};

use fframes_studio_protocol::PreviewIdentity;
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{build_materialization::sdk_pin, sdk_setup, services::worker_project};
use studio_project::{OpenProject, manifest::CargoEntry};
use studio_sdk::CompatibilityManifest;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const EXAMPLES: [(&str, &str, &str, &str); 5] = [
    (
        "hello-world",
        "HelloWorldMedia",
        "HelloWorldVideo<'_>",
        "example_video::HelloWorldVideo { media: &media, slug: \"Studio!\" }",
    ),
    (
        "motion-graphics",
        "MotionGraphicsMedia",
        "MotionGraphicsVideo<'_>",
        "example_video::MotionGraphicsVideo { media: &media }",
    ),
    (
        "signal-lab",
        "SignalLabMedia",
        "SignalLabVideo",
        "example_video::SignalLabVideo",
    ),
    (
        "fframes-studio-intro",
        "StudioIntroMedia",
        "StudioIntroVideo",
        "example_video::StudioIntroVideo",
    ),
    (
        "projectile-motion",
        "",
        "ProjectileMotionVideo",
        "example_video::ProjectileMotionVideo::new(props)",
    ),
];

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mut out = None;
    let mut sdk = sdk_setup::default_sdk_home().join("active");
    let mut check = false;
    let mut selected = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--out") => out = Some(PathBuf::from(args.next().ok_or("Missing --out folder")?)),
            Some("--sdk") => sdk = PathBuf::from(args.next().ok_or("Missing --sdk folder")?),
            Some("--example") => {
                selected = Some(args.next().ok_or("Missing --example name")?);
            }
            Some("--check") => check = true,
            Some("--help" | "-h") => {
                println!(
                    "prepare_examples --out <folder outside checkout> [--example <name>] [--sdk <installed-sdk>] [--check]"
                );
                println!(
                    "Creates Hello World, Motion Graphics, Signal Lab, fframes-studio-intro and projectile-motion without changing the originals."
                );
                println!(
                    "Projectile Motion requires --example projectile-motion and an SDK with kit 0.3.0."
                );
                println!("--check builds and exercises each project's real Studio preview worker.");
                return Ok(());
            }
            _ => return Err(format!("Unknown argument: {}", arg.to_string_lossy()).into()),
        }
    }
    let examples: Vec<_> = EXAMPLES
        .into_iter()
        .filter(|(name, ..)| {
            selected
                .as_ref()
                .map_or(*name != "projectile-motion", |selected| selected == *name)
        })
        .collect();
    if examples.is_empty() {
        return Err(
            "Unknown example; use hello-world, motion-graphics, signal-lab, fframes-studio-intro or projectile-motion"
                .into(),
        );
    }
    let out =
        out.ok_or("Pass --out <folder outside the repository>, e.g. C:\\Videos\\StudioExamples")?;
    let compatibility = CompatibilityManifest::from_json_str(
        &fs::read_to_string(sdk.join("compatibility.json"))
            .map_err(|e| format!("Install the managed SDK in Studio Setup first: {e}"))?,
    )?;
    if examples
        .iter()
        .any(|(name, ..)| *name == "projectile-motion")
        && compatibility.kit_version.as_deref() != Some("0.3.0")
    {
        return Err("Projectile Motion needs an SDK containing fframes-kit 0.3.0; select that SDK with --sdk".into());
    }
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    fs::create_dir_all(&out)?;
    let out = fs::canonicalize(out)?;
    for ancestor in out.ancestors() {
        if ancestor.join(".cargo/config.toml").exists() || ancestor.join(".cargo/config").exists() {
            return Err("Choose an output folder outside a Cargo-configured checkout; Studio refuses inherited Cargo configuration".into());
        }
    }
    // Refuse the whole batch before creating anything if an earlier copy exists.
    for (name, ..) in &examples {
        if out.join(name).exists() {
            return Err(format!(
                "{} already exists; choose a new output folder to preserve your edits",
                out.join(name).display()
            )
            .into());
        }
    }
    for (name, media, video, constructor) in examples {
        let project = prepare(
            &repository,
            &out,
            name,
            media,
            video,
            constructor,
            &compatibility,
        )?;
        println!(
            "Open project: {}",
            studio_bootstrap::external_path(&project.root).display()
        );
        if check {
            verify(&project, &sdk, &compatibility)?;
        }
    }
    println!("In Studio, choose Open project, select one of these folders, then Build preview.");
    Ok(())
}

fn prepare(
    repository: &Path,
    out: &Path,
    name: &str,
    media_type: &str,
    video_type: &str,
    constructor: &str,
    compatibility: &CompatibilityManifest,
) -> Result<OpenProject> {
    let destination = out.join(name);
    if destination.exists() {
        return Err(format!(
            "{} already exists; original retained",
            destination.display()
        )
        .into());
    }
    let staging = tempfile::Builder::new()
        .prefix(".studio-example-")
        .tempdir_in(out)?;
    let root = staging.path();
    // Keep the workspace-relative layout used by include_media_dir! and include_str!.
    let relative = format!("examples/{name}");
    for directory in ["src", "media"] {
        let source = repository.join(&relative).join(directory);
        copy_directory(&source, &root.join(&relative).join(directory))?;
    }
    fs::create_dir_all(root.join("src/bin"))?;
    let setup = if name == "fframes-studio-intro" {
        prepare_studio_intro(repository, root, compatibility)?
    } else if name == "projectile-motion" {
        fs::copy(
            repository.join(&relative).join("slides.json"),
            root.join("slides.json"),
        )?;
        format!(
            "let directory = fframes::MediaDirectory::read_folder(\"{relative}/media\")?;\n    let media = directory.process_media_source()?;\n    let props = std::sync::Arc::new(fframes_studio_kit::PropsStore::load(\"props.json\")?);\n    props.listen();\n    let hooks = props.hooks();\n    let video = {constructor};\n    let options = fframes::RenderOptions {{ media: Some(&media), ..Default::default() }};\n    "
        )
    } else {
        format!(
            "let media = <example_video::{media_type} as fframes::StaticMediaProvider>::prepare()?;\n    let video = {constructor};\n    let options = fframes::RenderOptions {{ media: Some(&media), ..Default::default() }};\n    "
        )
    };
    // Reuse the product's worker, including timing, export, protocol and audio support.
    let template = if name == "projectile-motion" {
        include_str!("../../studio-project/templates-slides/src/bin/studio_worker.rs")
    } else {
        include_str!("../../studio-project/templates/src/bin/studio_worker.rs")
    };
    let start = template
        .find("let directory =")
        .ok_or("Worker template media setup changed")?;
    let end = template
        .find("// One-shot export:")
        .ok_or("Worker template export setup changed")?;
    let worker = format!("{}{}{}", &template[..start], setup, &template[end..])
        .replace(
            "studio_video::StudioVideo",
            &format!("example_video::{video_type}"),
        )
        .replace("use fframes_studio_kit::PropsStore;\n", "")
        .replace("use std::sync::Arc;\n", "");
    fs::write(root.join("src/bin/studio_worker.rs"), worker)?;
    fs::write(
        root.join("src/main.rs"),
        format!(
            "fn main() -> Result<std::process::ExitCode, Box<dyn std::error::Error>> {{\n    {}Ok(fframes::cli::new(&video, options).timing_file(\"timeline.json\").run())\n}}\n",
            setup.replace("    let hooks = props.hooks();\n", ""),
        ),
    )?;
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"studio-{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\ndefault-run = \"studio-{name}\"\n\n[workspace]\n\n[lib]\nname = \"example_video\"\npath = \"{relative}/src/lib.rs\"\ndoctest = false\n\n[dependencies]\nfframes = {{ version = \"={}\", features = [\"cli\", \"compile-time-svgtree\"] }}\nfframes-studio-runtime = \"={}\"\nfframes-studio-protocol = \"={}\"\n",
            compatibility.fframes_version,
            compatibility.runtime_version,
            compatibility.runtime_version,
        ),
    )?;
    if name == "projectile-motion" {
        use std::io::Write;
        write!(
            fs::OpenOptions::new()
                .append(true)
                .open(root.join("Cargo.toml"))?,
            "fframes-kit = {{ version = \"=0.3.0\", features = [\"studio\"] }}\nfframes-studio-kit = \"={}\"\n",
            compatibility.runtime_version
        )?;
    }
    if name == "fframes-studio-intro" {
        use std::io::Write;
        fs::OpenOptions::new().append(true).open(root.join("Cargo.toml"))?.write_all(
            b"fframes-components = { path = \"components\", default-features = false, features = [\"prompt-zoom\", \"strike-through\", \"tracking-in\", \"typing-code-block\", \"word-flip\"] }\n"
        )?;
    }
    fs::write(root.join(".gitignore"), "/target/\n/out.*\n")?;
    fs::write(
        root.join("AGENTS.md"),
        format!(
            "# Studio example: {name}\n\nEdit the real video source in {relative}/src and its assets in {relative}/media.\nThis is an independent CPU-rendered copy; the repository example is unchanged.\nKeep the embedded media paths relative to this project's workspace root.\nStudio builds src/bin/studio_worker.rs; src/main.rs is the CPU CLI.\nTimeline edits live in timeline.json. Keep scene names/keys stable.\nUse frame, inspect and strip to verify changes; do not remove the worker bridge.\n"
        ),
    )?;
    studio_project::import(
        root,
        CargoEntry {
            manifest: "Cargo.toml".to_owned().try_into()?,
            package: format!("studio-{name}"),
            worker_target: "studio_worker".into(),
        },
        sdk_pin(compatibility),
    )?;
    fs::rename(root, &destination)?;
    Ok(studio_project::open(&destination)?)
}

/// Bundle the intro's non-SDK dependency and shared media inside the portable copy.
fn prepare_studio_intro(
    repository: &Path,
    root: &Path,
    compatibility: &CompatibilityManifest,
) -> Result<String> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|home| PathBuf::from(home).join(".cargo"))
        })
        .ok_or("Cannot locate Cargo home; set CARGO_HOME")?;
    let components = fs::read_dir(home.join("registry/src"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join("fframes-components-0.1.2"))
        .find(|path| path.join("Cargo.toml").is_file())
        .ok_or("fframes-components 0.1.2 is not cached; run cargo fetch --locked in the repository first")?;
    for directory in ["src", "media"] {
        copy_directory(
            &components.join(directory),
            &root.join("components").join(directory),
        )?;
    }
    for file in ["LICENSE", "CREDITS.md", "README.md"] {
        fs::copy(components.join(file), root.join("components").join(file))?;
    }
    // The library only needs fframes and ttf-parser. Exclude its optional native
    // CLI dependencies, which are not part of Studio's offline SDK.
    fs::write(
        root.join("components/Cargo.toml"),
        format!(
            "[package]\nname = \"fframes-components\"\nversion = \"0.1.2\"\nedition = \"2024\"\nlicense = \"MIT\"\nautobins = false\n\n[features]\ndefault = []\nprompt-zoom = []\nstrike-through = []\ntracking-in = []\ntyping-code-block = []\nword-flip = []\n\n[dependencies]\nfframes = {{ version = \"={}\", features = [\"compile-time-svgtree\"] }}\nttf-parser = \"0.25\"\n",
            compatibility.fframes_version,
        ),
    )?;
    // include_media_dir! is workspace-relative; keep the dependency's embedded
    // fonts available at its original `media` path as well as its runtime path.
    copy_directory(&components.join("media"), &root.join("media"))?;
    for relative in [
        "examples/fframes-intro/dynamic_media",
        "examples/fframes-studio-intro/dynamic_media",
    ] {
        copy_directory(&repository.join(relative), &root.join(relative))?;
    }
    let lib = root.join("examples/fframes-studio-intro/src/lib.rs");
    let source = fs::read_to_string(&lib)?;
    if !source.contains("pub mod render_cli;") {
        return Err("Intro CLI module changed; update the Studio adapter".into());
    }
    // Keep the original Skia CLI on disk, but don't compile it into the CPU
    // project. The generated main and worker own the Studio entry points.
    fs::write(
        lib,
        source.replace(
            "pub mod render_cli;",
            "#[cfg(any())] // Studio uses the generated CPU CLI and worker.\npub mod render_cli;",
        ),
    )?;
    fs::write(
        root.join("PREVIEW-NOTES.md"),
        "# fframes Studio intro\n\nThis is the full landscape video, with its seven scenes, soundtrack, sound effects, fonts and video clips.\nStudio's current CPU preview/export does not execute the original Skia shader effects (grain, contour and grid). The original shader source is retained.\nThe source repository is not changed. fframes-components 0.1.2 is bundled in components/ with its license and credits for offline builds.\n",
    )?;
    println!(
        "  Note: Studio's CPU preview/export omits Skia shader effects; see PREVIEW-NOTES.md."
    );
    Ok(String::from(
        "let media = <example_video::StudioIntroMedia as fframes::StaticMediaProvider>::prepare()?;\n    let soundtrack_dir = fframes::MediaDirectory::read_folder(\"examples/fframes-intro/dynamic_media\")?;\n    let soundtrack = soundtrack_dir.process_media_source()?;\n    let clips_dir = fframes::MediaDirectory::read_folder(\"examples/fframes-studio-intro/dynamic_media\")?;\n    let clips = clips_dir.process_media_source()?;\n    let components = <fframes_components::ComponentsMedia as fframes::StaticMediaProvider>::prepare()?;\n    let all = fframes::CombinedMediaProvider::from([&media as &dyn fframes::MediaProvider, &soundtrack, &clips, &components]);\n    let video = example_video::StudioIntroVideo;\n    let options = fframes::RenderOptions { media: Some(&all), audio_mix: fframes::AudioMixOptions { master_gain_db: 4.5, limiter: Some(fframes::LimiterOptions { ceiling_db: -1.3, ..Default::default() }), ..Default::default() }, ..Default::default() };\n    ",
    ))
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(format!("Example contains a symlink: {}", entry.path().display()).into());
        } else if kind.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn verify(project: &OpenProject, sdk: &Path, compatibility: &CompatibilityManifest) -> Result<()> {
    let manager = ProcessTreeManager::new();
    let builds = studio_engine::app_paths::AppPaths::system()?.builds();
    let build = worker_project::compile_portable_worker(
        project,
        sdk,
        compatibility.clone(),
        &builds,
        &manager,
    )?;
    let mut worker = worker_project::launch_preview_worker(
        build,
        PreviewIdentity {
            project_id: project.manifest.project_id.to_string(),
            open_session: uuid::Uuid::new_v4().to_string(),
            source_revision: project.inventory.revision.to_string(),
            worker_generation: 1,
        },
        &manager,
    )?;
    let timeline = worker.timeline()?;
    let mut inspected = vec![0, timeline.total_frames - 1, timeline.total_frames / 2];
    for scene in &timeline.scenes {
        inspected.extend([scene.start_frame, scene.end_frame - 1]);
    }
    inspected.sort_unstable();
    inspected.dedup();
    let inspection = worker.inspect(inspected)?;
    if inspection.truncated
        || inspection.diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == fframes_studio_protocol::DiagnosticSeverity::Error
        })
    {
        return Err(format!("Preview inspection failed: {:?}", inspection.diagnostics).into());
    }
    worker.frame(0, 0, 0.5)?;
    let frame = worker.frame(timeline.total_frames / 2, 1, 0.5)?;
    let audio = worker.prepare_audio(48_000)?;
    let _audio_source = worker.retain_audio_source(&audio, || false)?;
    println!(
        "  Verified: {} frames, {} fps, {} scenes, {} preview bytes, {} audio samples (silent: {})",
        timeline.total_frames,
        timeline.fps,
        timeline.scenes.len(),
        frame.pixels.len(),
        audio.sample_count,
        audio.silent,
    );
    worker.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires cached fframes-components 0.1.2 and repository intro media"]
    fn intro_contains_shared_soundtrack_clips_and_offline_components() -> Result<()> {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let out = tempfile::tempdir()?;
        let (name, media, video, constructor) = EXAMPLES[3];
        let project = prepare(
            &repository,
            out.path(),
            name,
            media,
            video,
            constructor,
            &CompatibilityManifest::default_linux_x64(),
        )?;
        assert!(project.worker_available);
        for relative in [
            "examples/fframes-intro/dynamic_media",
            "examples/fframes-studio-intro/dynamic_media",
            "examples/fframes-studio-intro/media",
            "examples/fframes-studio-intro/src/scenes",
        ] {
            assert_eq!(
                studio_project::SourceInventory::scan(&repository.join(relative))?,
                studio_project::SourceInventory::scan(&project.root.join(relative))?,
            );
        }
        assert!(project.root.join("components/LICENSE").is_file());
        assert!(
            project
                .root
                .join("components/media/Inter-Regular.ttf")
                .is_file()
        );
        let cargo =
            studio_project::lifecycle::read_cargo(&project.root, &project.manifest.entry.manifest)?;
        assert_eq!(
            cargo["dependencies"]["fframes-components"]["path"].as_str(),
            Some("components")
        );
        assert!(
            fs::read_to_string(project.root.join("PREVIEW-NOTES.md"))?.contains("does not execute")
        );
        Ok(())
    }

    #[test]
    fn copies_real_examples_with_independent_identities_and_preserves_edits() -> Result<()> {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let out = tempfile::tempdir()?;
        let compatibility = CompatibilityManifest::default_linux_x64();
        let mut ids = std::collections::HashSet::new();
        // These projects need only the SDK. The intro additionally needs the
        // registry component source and shared footage; exercise it with --check.
        for (name, media, video, constructor) in EXAMPLES.into_iter().take(3) {
            let source = repository.join("examples").join(name);
            let original = ["src", "media"].map(|directory| {
                studio_project::SourceInventory::scan(&source.join(directory)).unwrap()
            });
            let project = prepare(
                &repository,
                out.path(),
                name,
                media,
                video,
                constructor,
                &compatibility,
            )?;
            assert!(project.worker_available);
            assert!(ids.insert(project.manifest.project_id.clone()));
            assert_eq!(project.manifest.sdk, sdk_pin(&compatibility));
            for directory in ["src", "media"] {
                let relative = format!("examples/{name}/{directory}");
                assert_eq!(
                    studio_project::SourceInventory::scan(&repository.join(&relative))?,
                    studio_project::SourceInventory::scan(&project.root.join(&relative))?
                );
            }
            fs::write(project.root.join("src/main.rs"), "my edit")?;
            assert!(
                prepare(
                    &repository,
                    out.path(),
                    name,
                    media,
                    video,
                    constructor,
                    &compatibility
                )
                .is_err()
            );
            assert_eq!(
                fs::read_to_string(project.root.join("src/main.rs"))?,
                "my edit"
            );
            assert_eq!(
                original,
                ["src", "media"].map(|directory| {
                    studio_project::SourceInventory::scan(&source.join(directory)).unwrap()
                })
            );
        }
        Ok(())
    }
}
