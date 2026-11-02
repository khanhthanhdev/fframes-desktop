---
phase: 6
title: "Web client workspace"
status: pending
priority: P1
effort: "4w"
dependencies: [3]
---

# Phase 6: Web client workspace

## Goal

A Dioxus 0.7.10 web client served by `studio-server` with the Studio workspace: shell, sidebar, agent
panel, changes and diffs, history, setup, settings, presets, assets, export and the player (canvas
editing and timeline follow in phase 7).

## Context

- Scout inventory of `studio-ui` (surfaces and line counts) in the proposal, section 6.
- `docs/desktop/design-system.md`, `desktop/crates/studio-theme/src/{palette.rs,metrics.rs,typography.rs}`
- `desktop/crates/studio-ui/src/shell/layout.rs` (the Cap-style layout)
- `desktop/crates/studio-ui/src/shell/keymap.rs` (key contexts)

## Requirements

- Same layout as the desktop app: header, player card beside a resizable sidebar (Agent, Project,
  Assets, Style), timeline card below, status bar.
- Theme tokens exported to CSS custom properties from `studio-theme` by a build script, light and dark,
  so the GPUI and web palettes cannot drift.
- Long lists (transcript, history, diff lines) are windowed and stay smooth at 10,000 rows.
- Markdown and diff parsing run in Rust (`studio-markdown`, the diff model) inside WASM; only visible
  blocks render.
- Reconnects automatically after the server restarts and restores the view from the next snapshot.
- Keyboard map follows the desktop contexts; plain keys only in player and timeline.
- Works in current Chrome, Edge and Firefox, verified on this VM. Safari cannot run on Windows and is
  reported as unverified unless a Mac is available.

## Architecture

- `desktop/clients/studio-web/src/`: `app.rs` (router: setup or workspace), `net/` (transport, request
  ids, snapshot store as Dioxus signals), `theme/` (generated CSS), `kit/` (button, icon, menu, select,
  segmented, slider, number field, toggle, tooltip, notice, text input, tab rail, card), `shell/`,
  `agent/` (chat, composer, todo, question and permission cards, history, changes with unified and split
  diffs), `player/` (canvas, WebCodecs decoder, AudioWorklet, transport controls), `setup/`,
  `settings/`, `presets/`, `assets/`, `export/`, `files/` (server folder browser dialog).
- `virtual_list.rs`: fixed-overscan windowed list with measured row heights.
- JS glue only where `web-sys` cannot reach (AudioWorklet processor script), kept in `assets/`.

## Related code files

- Create: `desktop/clients/studio-web/{Cargo.toml,Dioxus.toml,build.rs,assets/*,src/**}`
- Modify: `desktop/crates/studio-theme` (CSS export function), `desktop/Cargo.toml`
- Modify: `desktop/crates/studio-server/src/transport.rs` (embed or serve the built bundle)

## Implementation steps

1. Project setup with `dx`, pinned versions, theme CSS generation, kit components with a gallery page.
2. Transport and snapshot store; connection banner and reconnect.
3. Shell and layout with resizable sidebar and timeline split.
4. Agent panel: chat with windowed transcript and markdown, composer (Enter, Shift+Enter, slash
   completion, attachments by upload, steer, send at boundary, send now), todo, questions, permissions.
5. Changes tab and review view: file list, unified and split diffs, toggles, Apply, Revert file, Undo.
6. History tab with search and resume.
7. Player: canvas frame drawing, play, pause, step, scrub, audio, mute (no editing yet).
8. Setup and onboarding, settings pages, presets, assets, export panel, project open/create/import with
   the folder browser.
9. Browser tests with the scripted agent server from phase 3.

## Todo

- [ ] Setup, theme CSS, kit, gallery
- [ ] Transport and store
- [ ] Shell and layout
- [ ] Agent chat and composer
- [ ] Changes and review
- [ ] History
- [ ] Player
- [ ] Setup, settings, presets, assets, export, projects
- [ ] Browser tests

## Success criteria

- The acceptance flow in `plan.md` (except canvas editing and timeline editing) passes in Chrome on this
  VM against a real project and a real agent CLI.
- A 10,000-row transcript scrolls without visible stalls.

## Risks

Feature parity is wide (about 50k lines of GPUI UI). Port surface by surface and keep the desktop
README's behaviour as the specification.
