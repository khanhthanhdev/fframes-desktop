# Open an existing fframes project in fframes Studio

This guide turns an existing fframes project into one that Studio can open, preview, edit
with an agent, and export. An existing project is a Rust crate that implements
`fframes::Video`, with or without the web editor.

Importing writes one file, `studio.json`, and nothing else. Your sources, `Cargo.toml`,
`Cargo.lock`, `AGENTS.md`, and Git history are left as they are. For the preview to work, you
also add a small worker binary yourself (step 6).

If you are starting from scratch, use **Create…** in Studio instead; it generates a project
that already has everything below.

## How Studio uses your project

You need a few details about how Studio uses your project to understand the requirements in
the steps below:

1. **Import** (`Import Rust…`) reads the package `Cargo.toml` you select. It checks that the
   entry source exists and that every file in the project folder is a regular file with a
   portable name, then writes `studio.json`. It never runs Cargo or installs anything.
2. **Build preview** copies the project folder into a private build directory in app data.
   In that copy, it points `fframes`, `fframes-studio-runtime`, and
   `fframes-studio-protocol` at the installed SDK. It replaces crates.io with the SDK's
   vendored crates and then runs
   `cargo build --locked --offline --package <your package> --bin studio_worker` (debug
   profile, default features). Your own `Cargo.toml` and `Cargo.lock` are never changed.
3. The worker it builds serves the preview, the timeline, audio, and **Export…**. Exports and
   previews read `timeline.json` (your scene order, trims, and splits) at run time.

Two consequences follow from this:

- **The build is offline.** Only crates in the SDK's vendor directory are available. These
  are the dependencies of the framework and its worker (for example `serde`, `serde_json`,
  `ttf-parser`, and `tiny-skia`). Git dependencies and any crates.io crate not vendored there
  fail with "failed to select a version" or "no matching package". To see what is available,
  look in `%USERPROFILE%\.fframes\sdk\active\framework\vendor` (or
  `~/.fframes/sdk/active/framework/vendor`).
- **Everything the video needs must be inside the project folder.** That includes path
  dependencies, `include_media_dir!` folders, `include_str!` files, and runtime media.

## Requirements at a glance

| Requirement | Checked by | Error if missing |
| --- | --- | --- |
| A package `Cargo.toml` (has `[package]`), selected directly | Import | "Choose a package Cargo.toml inside the workspace, not its virtual root" |
| `src/lib.rs`, `src/main.rs`, `[lib] path`, or a `[[bin]]` source exists | Import | "Rust entry source missing" |
| No symlinks, special files, or names with `\ : * ? " < > \|` anywhere in the project folder (`.git` and `target` are skipped) | Import and every build | "unsupported link", "invalid portable relative path" |
| `src/bin/studio_worker.rs` or a `[[bin]] name = "studio_worker"` | Build preview | "Worker bridge missing; add an explicit worker entry before building" |
| `fframes`, `fframes-studio-runtime`, and `fframes-studio-protocol` at exactly the SDK's versions | Build preview | "dependency version does not match SDK framework …" |
| No `[patch]` or `[replace]` | Build preview | "Cargo overrides need explicit SDK compatibility" |
| Path dependencies inside the project folder | Build preview | "external path dependency" |
| No `.cargo/config.toml` (or `.cargo/config`) in the project or **any parent folder** | Build preview | "Imported Cargo configuration conflicts with SDK binding" / "Inherited Cargo configuration is not portable" |
| The project is pinned to the installed SDK | Build preview | "SDK pin mismatch" |
| Each scene has a stable `editor_instance_key` | Timeline | Clips get orphaned when a scene is renamed |

## Steps

### 1. Install the SDK and an agent

Start Studio, either from a checkout with
`cargo run --locked --manifest-path desktop/Cargo.toml -p fframes-studio` or as the installed
app. Then finish the setup: **SDK**, **Agent**, and **Permissions**. You can reopen these
later under **Settings → Setup**.

Note which framework versions the SDK ships. They are in
`%USERPROFILE%\.fframes\sdk\active\compatibility.json`:

```json
"sdk_id": "studio-sdk-x86_64-pc-windows-msvc-v4",
"fframes_version": "1.4.1",
"runtime_version": "0.2.0",
```

`fframes_version` is the version for `fframes`. `runtime_version` is the version for both
`fframes-studio-runtime` and `fframes-studio-protocol`.

Install the newest SDK before importing. `studio.json` records the exact SDK that was
installed at import time (step 8), and switching SDKs later means importing again.

### 2. Make a standalone copy

Work on a copy, or commit first. Put the project in its own folder, outside every other
Cargo workspace and outside any folder that has a `.cargo/config.toml`. Studio checks every
parent folder up to the drive root. That includes your home folder: if
`%USERPROFILE%\.cargo\config.toml` (`~/.cargo/config.toml`) exists, a project anywhere under
your home folder fails to build. Put the project somewhere else, for example `D:\Videos\`,
or move that file.

Import treats the nearest parent `Cargo.toml` that has a `[workspace]` table as the project
root. That parent is where `studio.json` goes, and it is the folder that is copied and
scanned on every build. Copying an example out of the fframes repository therefore matters.
If you import `examples/hello-world/Cargo.toml` in place, the whole repository becomes the
project.

Leave out of the folder anything that is not part of the video. Every file except `.git/`
and `target/` is scanned and copied for each build, and import refuses symlinks:

- `node_modules/` (pnpm creates symlinks) and the web editor's build output;
- rendered videos, frame dumps, and other large scratch files.

### 3. Make the package its own workspace and pin the SDK versions

```toml
[package]
name = "my-video"
version = "0.1.0"
edition = "2024"
default-run = "my-video"      # needed once there are two binaries (main + studio_worker)

[workspace]                   # this folder is the workspace root

[dependencies]
fframes = { version = "=1.4.1", features = ["cli", "compile-time-svgtree"] }
fframes-studio-runtime = "=0.2.0"
fframes-studio-protocol = "=0.2.0"
```

Replace the versions with the ones from step 1. Studio compares them exactly, with or without
the leading `=`. The template uses `=`, which keeps a standalone `cargo` from picking another
release.

Then clean up the rest of `Cargo.toml`:

- **`.workspace = true` dependencies.** Replace inherited `fframes.workspace = true` (and
  similar) with real versions. The parent workspace that defined them is no longer the
  root. The same applies to `[lints] workspace = true` and other inherited keys: remove
  them or define `[workspace.lints]` here.
- **Features.** Studio builds with default features. If your crate gates `cli` and
  `compile-time-svgtree` behind a `renderer` feature (as the repository examples do), keep
  `default = ["renderer"]` or list the features on `fframes` directly as above. Add the
  `styles` feature if you use `fframes::Styles` (step 9).
- **Remove what the offline build cannot resolve:**
  - `[patch]` and `[replace]`;
  - git dependencies;
  - path dependencies outside the folder (copy the crate into the folder and point `path` at
    the copy);
  - crates that are not in the SDK vendor directory (see "How Studio uses your project").
- **Renderer and FFmpeg crates.** Remove a direct `fframes_skia_renderer` dependency (the
  examples use it for `--renderer gpu` in their own CLI). Studio rewrites only the three
  crates above to the SDK, so that dependency cannot resolve offline. The worker gets Skia
  from `fframes-studio-runtime` instead (see "Renderer and shaders"). Also drop
  FFmpeg build features of `fframes` such as `build-portable`, `h264`, or `libav-agree-gpl`:
  the SDK links its own bundled FFmpeg.
- **`crate-type`.** Drop `cdylib` from `[lib] crate-type` unless you still build the WASM
  editor bridge from this package. Studio only needs `rlib`.
- **Binary `required-features`.** If your CLI binary has `required-features = ["renderer"]`,
  make sure that feature is on by default, or the binary is silently skipped.

### 4. Fix paths that were relative to the old workspace

`include_media_dir!` resolves its path from the **workspace root**. In the fframes
repository the examples use `include_media_dir!(pub struct HelloWorldMedia,
"examples/hello-world/media")`. Once the package is its own workspace, that path must
change. Choose one of these options:

- **Simplest:** move nothing and change the path to `"media"`.
- **Keep the original layout:** put the sources under `examples/<name>/` inside the new
  folder and point `[lib] path` at them. This is what Studio's own example importer does (see
  [`prepare_examples.rs`](../../desktop/crates/studio-engine/examples/prepare_examples.rs)).

`include_str!`/`include_bytes!` paths are relative to the source file and keep working as
long as the files move with it. Runtime folders read with `MediaDirectory::read_folder`
resolve from the package folder (the directory with `Cargo.toml`), so keep them inside the
project, for example `dynamic_media/`.

### 5. Give every scene a stable key

The timeline and `timeline.json` find a scene by its `editor_instance_key()`, or by its
`name()` when there is no key. Give each scene a fixed key that will not change when you
rename the type:

```rust
impl Scene for Intro {
    fn name(&self) -> &'static str { "intro" }
    fn editor_instance_key(&self) -> Option<&str> { Some("intro") }
    // duration, render_frame, ...
}
```

List each scene once in `define_scenes`. A video without scenes still opens, but its timeline
has a single clip that cannot be reordered or split.

### 6. Add the Studio worker

Copy the template worker
[`studio_worker.rs`](../../desktop/crates/studio-project/templates/src/bin/studio_worker.rs)
to `src/bin/studio_worker.rs`. Import never adds it for you; without it the project opens
but **Build preview** fails. Change only two things in it:

1. **Your video type and constructor.** Replace `studio_video::StudioVideo` (twice: in
   `print_export_capabilities::<…>()` and in `StudioVideo::new()?`) with your library crate
   name and type. The library crate name is the package name with `-` replaced by `_`, or
   `[lib] name`. Types with a lifetime use `'_`, for example
   `print_export_capabilities::<my_video::MyVideo<'_>>()`.
2. **The media setup**, between `let value = …;` and `// One-shot export:`. The template
   reads a runtime `media` folder. For a video that embeds media with `include_media_dir!`,
   use:

   ```rust
   let media = <my_video::MyVideoMedia as fframes::StaticMediaProvider>::prepare()?;
   let video = my_video::MyVideo { media: &media, title: "World" };
   let options = fframes::RenderOptions {
       media: Some(&media),
       ..Default::default()
   };
   ```

   For several sources (embedded media plus a runtime folder of clips), combine them:

   ```rust
   let media = <my_video::MyVideoMedia as fframes::StaticMediaProvider>::prepare()?;
   let clips_dir = fframes::MediaDirectory::read_folder("dynamic_media")?;
   let clips = clips_dir.process_media_source()?;
   let all = fframes::CombinedMediaProvider::from([&media as &dyn fframes::MediaProvider, &clips]);
   let video = my_video::MyVideo { media: &media };
   let options = fframes::RenderOptions { media: Some(&all), ..Default::default() };
   ```

   Copy every other `RenderOptions` field your `main.rs` sets (for example `audio_mix`), so
   that the preview and export sound and look like your CLI render.

Leave the rest of the file unchanged: the export flags, `--preview-worker`, `--frame-port`,
`with_timing_args`, and `serve_preview_worker`. Studio talks to the worker through them.
[`prepare_examples.rs`](../../desktop/crates/studio-engine/examples/prepare_examples.rs)
makes exactly this adaptation for four repository examples, including one with combined
media, a bundled path dependency, and a custom audio mix.

### 7. Make the CLI follow the timeline

Opt `main.rs` in to `timeline.json`, so that `cargo run -- render` produces the same cut as
Studio:

```rust
fframes::cli::new(&video, options).timing_file("timeline.json").run()
```

A missing `timeline.json` means code order, so you do not need to create one. An invalid one
stops the CLI and export with the reason; fix it rather than deleting it.

Optionally, copy the template's [`AGENTS.md`](../../desktop/crates/studio-project/templates/AGENTS.md)
rules into your own `AGENTS.md`. The Studio agent reads `AGENTS.md` and `CLAUDE.md`, and
the template rules tell it about the worker, `timeline.json`, and stable keys.

### 8. Import into Studio

1. Click **Import Rust…** in the header (or on the start screen) and select the package's
   `Cargo.toml`.
2. Studio writes `studio.json` next to the workspace root `Cargo.toml` and opens the project.
   It contains:
   - a new project id;
   - the display name (the package name);
   - the SDK pin (`sdk.release` is the SDK id, plus a compatibility checksum);
   - the entry (`manifest`, `package`, `worker_target: "studio_worker"`);
   - an empty asset list.
3. Click **Build preview**. The first build compiles the framework and can take several
   minutes. Later builds reuse a shared target directory per SDK, and edits only recompile
   your crate.

If `studio.json` already exists, **Import Rust…** just opens it; it does not overwrite it.

### 9. Optional: canvas selection and the Style tab

These are not needed to preview, edit, or export. Add them when you want to click elements on
the canvas and edit design tokens:

- `style/tokens.json`, loaded once with `fframes::Styles::from_json_str(include_str!(…))`
  in your constructor (needs the `styles` feature of `fframes`);
- an `EditorObjectKey` id on the elements you want to select.

The template [`lib.rs`](../../desktop/crates/studio-project/templates/src/lib.rs) shows both.

## Renderer and shaders

The worker draws with whatever the installed SDK's `fframes-studio-runtime` supports:

- **SDK v5 and later** ship Skia. Studio asks for `--renderer auto`, which draws with Skia
  on the GPU when one works (Vulkan on Windows and Linux, Metal on macOS), so
  `fframes::Shader` effects run. Without a usable GPU it uses the built-in CPU renderer,
  which skips shaders. A GPU worker that stops responding moves the rest of the session to
  the CPU renderer.
- **SDK v4 and earlier** have no Skia in the runtime. Previews and exports always use the
  CPU renderer and skip shaders.

There is no renderer setting in the UI yet. For testing, start Studio with
`FFRAMES_STUDIO_RENDERER=gpu|skia-cpu|cpu` to force one. Keep the default features of
`fframes-studio-runtime`; they enable Skia when the SDK has it.

## Working with the project afterwards

- **Edit:** use the agent panel, the timeline (order, trim, and split are written to
  `timeline.json`), and the **Style** tab. Export from **Export…**.
- **Commit `studio.json` and `timeline.json` to Git.** Studio's history, checkpoints, agent
  sessions, and build caches live in app data under the project id and do not travel with
  the folder.
- **Web editor:** it keeps working from its own folder outside the Studio project, since it
  builds the same library crate. Do not keep its `node_modules/` inside the Studio project
  (step 2).
- **Upgrading the SDK.** A project is pinned to the SDK it was imported with. After
  installing a new SDK, Build preview reports "SDK pin mismatch", and Studio has no re-pin
  action. To move the project:
  1. update the three versions in `Cargo.toml`;
  2. move `studio.json` out of the folder (keep it as a backup);
  3. run **Import Rust…** again.

  The project gets a new id, so its earlier Studio history stays with the old id.

## Building outside Studio

`fframes 1.4.1` and the `fframes-studio-*` crates ship inside the SDK and are not published
on crates.io (the newest published `fframes` is `1.2.1-rc.45`). A plain `cargo build` in the
project therefore fails with "failed to select a version". To check the project from a
terminal, point Cargo at the SDK's framework on the command line instead of in the project.
A `[patch]` in `Cargo.toml` or a `.cargo/config.toml` in the project would break Studio's
build:

```powershell
$fw = "$env:USERPROFILE/.fframes/sdk/active/framework/framework".Replace('\', '/')
$patch = @(
  "--config", "patch.crates-io.fframes.path='$fw/fframes'",
  "--config", "patch.crates-io.fframes-studio-runtime.path='$fw/fframes-studio-runtime'",
  "--config", "patch.crates-io.fframes-studio-protocol.path='$fw/fframes-studio-protocol'"
)
cargo tree --depth 1 @patch            # resolves against the SDK
cargo build --bin studio_worker @patch # also needs FFmpeg set up as for any fframes build
cargo run @patch -- timeline
cargo run @patch -- inspect
```

This standalone build resolves other crates from crates.io, not the SDK vendor directory, so
it can succeed where Studio's offline build fails. Studio's **Build preview** is the
authoritative check.

## Troubleshooting

| Message or symptom | Cause and fix |
| --- | --- |
| "Choose the selected package's Cargo.toml" | A file other than `Cargo.toml` was selected. |
| "Choose a package Cargo.toml inside the workspace, not its virtual root" / "ambiguous or virtual workspace" | You selected a workspace root without `[package]`. Select the member crate's `Cargo.toml`. |
| "Rust entry source missing" | No `src/lib.rs`, `src/main.rs`, `[lib] path`, or `[[bin]]` source file exists. |
| "unsupported link" / "invalid portable relative path" / "unsupported special file" | There is a symlink, junction, or file name that is not portable somewhere in the project. Remove it or replace it with real files (step 2). |
| "Worker bridge missing; add an explicit worker entry before building" | Add `src/bin/studio_worker.rs` (step 6). |
| "dependency version does not match SDK framework X" | Pin the crate to exactly `X` (step 3). |
| "Cargo overrides need explicit SDK compatibility" | Remove `[patch]` / `[replace]`. |
| "external path dependency" | Copy the dependency into the project folder and point `path` at the copy. |
| "Inherited Cargo configuration is not portable" | A parent folder, often your home folder, has `.cargo/config.toml`. Move the project outside it (step 2). |
| "Imported Cargo configuration conflicts with SDK binding" | Remove the project's own `.cargo/config.toml`. |
| "SDK pin mismatch" | The project was imported with another SDK. See "Upgrading the SDK". |
| Build fails with "failed to select a version" / "no matching package" | A crate is not in the SDK vendor directory, or is a git dependency. Remove it, or copy its source into the folder as a path dependency (its own dependencies must be vendored too). |
| Build fails with `include_media_dir` "Media dir should be relative cargo project root" | The path still points to the old workspace layout (step 4). |
| "made by an older Studio … cannot be opened" / "made by a newer Studio" | `studio.json` is from another Studio version. Older schemas are not migrated: move `studio.json` away and import again, or update Studio. |
| Preview has no shader effects | The CPU renderer is in use: either the SDK is older than v5, or no usable GPU was found. Install the latest SDK and update GPU drivers (Vulkan). |
| Timeline cannot split or reorder | The video has no scenes or no stable keys (step 5), or `main.rs` is not using `timing_file` (step 7). |
