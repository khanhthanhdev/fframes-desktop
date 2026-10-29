# AGENTS.md - AI Coding Agent Guidelines for fframes

This file provides guidelines for AI coding agents working on the fframes codebase.

## Project Overview

fframes is a Rust-based video generation framework that renders videos from SVG-based scene descriptions.

**Tech Stack:**

- **Core:** Rust (Edition 2024), Cargo workspace
- **Editor:** ReScript (compiles to JS), React 19, Tailwind CSS v4
- **WASM:** wasm-bindgen, wasm-pack for browser preview
- **Graphics:** `svgr!` SVG trees rendered by the built-in CPU backend (tiny-skia) or the optional Skia GPU backend; FFmpeg for encoding/decoding

## Build & Development Commands

### Task Runner: Just (justfile)

```bash
# Full repository setup (run first after clone)
just init-repo

# Build everything (cargo + editor)
just build

# Watch editor during development
just watch-editor

# Run example with live reload (editor + WASM hot reload)
just run <example-name>

# Render example to video file
just render <example-name>
```

### Rust Commands

```bash
just clippy   # = cargo clippy -- -D warnings

# Run all tests
just test
cargo test -p fframes_test_utils --no-default-features

# Check WASM compilation for examples
just check-wasm <example-name>
just check-examples  # checks all examples
```

### Editor (ReScript/TypeScript) Commands

```bash
cd fframes-editor

# Build ReScript and bundle
pnpm build

# Development mode with watch
pnpm dev

# Clean ReScript build
pnpm rescript:clean
```

### Package Management

```bash
# Lint package.json consistency
pnpm syncpack lint

# Format JS/TS files
pnpm prettier --write .
```

## Code Style Guidelines

### Rust Conventions

**Imports:**

- Group imports: std lib first, then external crates, then local modules
- Use `use crate::` for internal module imports
- Re-export public API through `lib.rs` using `pub use module::*;`

```rust
use std::{error::Error, fmt};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::{Color, Duration, Frame, Svgr};
```

**Error Handling:**

- Define custom errors as enums in `error.rs` modules
- Implement `std::error::Error` and `fmt::Display` for error types
- Use `type Result<T> = std::result::Result<T, CustomError>;` pattern
- Implement `From` traits for error conversion

```rust
#[derive(Debug)]
pub enum FFramesError {
    UserError(String),
    MediaError(crate::media::FFramesMediaError),
}

impl From<SomeError> for FFramesError {
    fn from(err: SomeError) -> Self { ... }
}
```

**Traits:**

- Use associated constants for video metadata: `const FPS`, `const WIDTH`, `const HEIGHT`
- Prefer `&self` methods over consuming self when possible
- Use lifetimes explicitly when returning references tied to self

```rust
pub trait Video: Sync + Sized {
    const FPS: usize;
    const WIDTH: usize;
    const HEIGHT: usize;

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a>;
}
```

**Documentation:**

- Use `///` doc comments for public APIs
- Include examples in doc comments with ```rust blocks
- Document panics, safety concerns, and performance considerations

**WASM Compatibility:**

- Use `#[cfg(not(target_arch = "wasm32"))]` for native-only code
- Use `#[cfg(target_arch = "wasm32")]` for WASM-specific code

### ReScript Conventions

**File Structure:**

- One component per file, named after component (e.g., `Editor.res`)
- Use `open` sparingly, prefer qualified access
- Place bindings in `src/bindings/` directory

**Components:**

```rescript
@genType.as("ComponentName") @react.component
let make = (~prop1: type1, ~prop2: type2) => {
  // hooks first
  let (state, setState) = React.useState(_ => initialValue)

  // effects
  React.useEffect0(() => { ... None })

  // render JSX
  <div className="tailwind-classes">
    {React.string("text")}
  </div>
}
```

**Naming:**

- Components: `PascalCase` files, lowercase `make` function
- Types: `lowerCamelCase` (ReScript convention)
- Variants: `PascalCase` (e.g., `MediaList.Grid`, `MediaList.List`)

**Styling:**

- Use Tailwind CSS classes via `className`
- Use `Cx.cx([...])` for conditional class concatenation

## Testing

**Rust Tests:**

- Unit tests in `#[cfg(test)] mod tests` blocks
- Integration tests in `/e2e/tests/`
- Visual regression tests compare rendered frames using `odiff`
- `just e2e cpu|skia-cpu|gpu` renders key frames of every example through its CLI and compares
  them with `e2e/snapshots`, the same images for every renderer; pass `--update` after an
  intended change and commit the new PNGs

## Project Structure

```
/fframes                   # Core: Video/Scene traits, svgr! trees, animation, text, CPU renderer, encoding
/fframes-media             # Audio/video decoding (ffmpeg), fonts, images, subtitles
/fframes-skia-renderer     # Optional Skia backend (GPU via vulkan/metal, or Skia CPU)
/fframes-native-player     # Real-time preview window: fframes_native_player::play(&video, &PlayerOptions)
/fframes-editor            # Web editor UI (ReScript + React), published as @fframes/editor
/fframes-editor-controller # WASM bridge between a Video impl and the editor
/fframes-test-utils        # Snapshot helpers for svgr! trees
/fframes-three-d           # 3D: three.js style scene graph, glTF, CPU rasterizer or wgpu (GPU texture shared with Skia)
/svgr-macro                # The svgr! procedural macro (SVG DSL + static-subtree hashing)
/media-dir-macro           # include_media_dir! (embeds a media folder into the binary)
/webvtt-parser             # Subtitle format parser
/cargo-fframes             # `cargo fframes new`: project scaffolding (templates/)
/scripts/new-video.sh      # curl | bash bootstrap for cargo-fframes
/e2e                       # End-to-end visual regression test
/examples                  # Example video projects (each is a lib + bin + editor bridge)
/desktop                   # fframes Studio: native GPUI desktop app, its own Cargo workspace
/fframes-studio-kit        # Studio's props store for fframes-kit (github.com/khanhthanhdev/fframes-kit)
```

fframes Studio (`/desktop`) is documented in [docs/desktop/README.md](docs/desktop/README.md).

## Creating a New Video

Every video is a Rust crate (a library implementing `fframes::Video`, a binary that renders it
through `fframes::cli`, and an optional WASM bridge for the web editor). Read
[docs/creating-a-video.md](docs/creating-a-video.md) before writing or changing a video or an
example: scaffolding, media, `Video`, animation, text, scenes and `timeline.json`, video sources,
shaders, 3D, wgpu layers, audio mix, Lottie, the CLI commands, editor preview and the
verification checklist.

## Important Notes

- Run `just clippy` before committing - warnings are errors
- Use `cargo fmt` for Rust formatting
- Use `pnpm prettier --write .` for JS/TS/ReScript formatting
- Check WASM compilation with `just check-wasm <example>` when modifying editor bridge code
