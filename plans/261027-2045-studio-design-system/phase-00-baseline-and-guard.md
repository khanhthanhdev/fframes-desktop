# Phase 0: baseline captures and the style guard

## Goal

Before anything changes: record how every screen looks today, and add a test that stops new
off-system styling while the migration runs.

## 0.1 Capture tool

A gallery mode that renders named fixture states headless and writes PNGs, so every later phase
captures the same states.

- Extend `desktop/app/examples/ui_gallery.rs` with `--capture <dir> [--appearance light|dark]
  [--page <name>]`. Each page renders into an offscreen window at fixed sizes and saves a PNG per
  state. Use the GPUI test-support window capture if the fork has it. Otherwise open a real
  window and capture it with the Win32 `PrintWindow` call (with `SetProcessDPIAware`), which
  worked for `agent-current.png` in this thread.
- Pages and states (fixtures, no live engine):
  - `kit`: every component in every state (see phase 9 list).
  - `shell`: start screen with recents; open project with preview; build failed notice; status
    details open; overflow menu open; present mode with progress.
  - `sidebar`: Project, Assets, Style, Props (all control kinds), Slides (the three review states),
    Presets.
  - `agent`: empty, not set up, working, failed tool, permission, question, todo plus queue,
    long code block, history.
  - `export`: settings, running, done, failed.
  - `settings`: General, Agent, SDK, Presets, Export pages.
  - `onboarding`: each step.
  - `timeline`: scenes, stops, trim, audio lanes, minimap.
- Sizes: the main window at 1275×800 and 1920×1080; sidebar captures at 320 and 520.
- Output to `.amp/in/artifacts/design-system/<phase>/<appearance>/<page>-<state>.png`. The files
  are not committed.

## 0.2 Baseline

Run the capture for light and dark and keep it as `baseline/`. Later phases compare against it.

## 0.3 Style guard

`desktop/crates/studio-ui/tests/style_guard.rs`, a plain test that reads source files.

- Scope: every `.rs` under `studio-ui/src` except `kit/` and `#[cfg(test)]` modules. The guard
  itself skips lines inside `mod tests`.
- Forbidden patterns, each counted per file:
  - colours: `gray(`, `.gray_`, `.blue_`, `.red_`, `.amber_`, `.green_`, `rgb(`, `rgba(`,
    `hsla(`, `gpui::black()`, `gpui::white()`, `SETTINGS_ACCENT`, `.settings_`, `.body_`,
    `header_bg`, `header_border`;
  - type: `text_size(`, `text_xs(`, `text_sm(`, `font_weight(` outside `TextStyle`;
  - sizes: `px(` followed by a digit, `rounded(px(`, `opacity(` with a literal.
- Allowlist: `tests/style_guard_allowlist.txt` with `path pattern count`, generated once from the
  current tree. The test fails when a file's count exceeds its entry or an unlisted file has a hit.
  When a count drops, the message prints the new lower number to paste in, so the list only
  shrinks.
- Exceptions by design, listed at the top of the allowlist with a reason:
  - `timeline/` painters and `agent/changes/paint.rs` may use `px(` only inside `const` items;
  - `agent/syntax/` may use the `syntax` colour set (phase 1).
- Kit: a second, smaller guard for `kit/` that forbids colour literals and Radix steps outside
  `kit/` theme plumbing. `px(` in the kit is migrated in phase 2 and guarded from then on.

## Checks

- `cargo test -p studio-ui style_guard` passes on the current tree with the generated allowlist.
- Adding a `gray(3)` to any screen makes it fail.
- Baseline captures exist for every page and state in both appearances.
