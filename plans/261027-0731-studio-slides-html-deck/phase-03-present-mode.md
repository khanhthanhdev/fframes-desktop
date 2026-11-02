---
phase: 3
title: "Present mode"
status: pending
priority: P1
effort: 4d
dependencies: [1]
---

# Phase 3: Present mode

<!-- Updated: Red Team 2026-10-27 - render inside the player section with its own focus and key
context (RT5), fullscreen state tracking (RT5), audio clamped at the bound (RT6), promotions
and retimes while presenting (RT7) -->

## Goal

Play the project as a slide deck inside Studio: full screen, each slide's animation plays with
audio from the previous stop to the next one and holds there until the presenter advances.

## Context

- `PlaybackClock` is the single cursor owner
  (`desktop/crates/studio-engine/src/playback_clock.rs:45-57`). `tick` advances and stops at
  `total_frames` (`:171-190`); `toggle` at the end seeks to 0 (`:148-150`); `reinstall`
  (`:68-86`) resets everything except `playing`. Clock tests: `studio-engine/tests/timeline.rs`.
- Shell playback in `desktop/crates/studio-ui/src/shell/player.rs`: `seek_preview` (`:253-274`,
  resumes when `resume_install` is set, `:267-269`), `toggle_playback`/`pause_playback`
  (`:322-358`), `pump_playback` (`:676-724`, stops audio after `tick` stops). A promotion while
  playing sets `resume_install` and pauses (`:207-217`); `retimed()` resumes playback after
  reinstall (`:132, 158-160`).
- Audio: `AudioService::stage` computes its sample bound from `timeline.total_frames`
  (`services/audio_service.rs:625-631`) and the callback writes until it (`:795-830`); `stop`
  only flips flags (`:159-161`). So samples past a UI-side stop reach the device.
- Rendering: during playback a new frame notifies only the cached player section
  (`shell/sections.rs:1-5, 59-64`, `player.rs:282-289`). The stage element carries the
  `PLAYER` key context and its own handlers (`player.rs:830-848`), whose bindings for space,
  arrows, Home, End and Escape (`shell/keymap.rs:43-50`) beat an outer context.
- GPUI fork `c32bfd3` implements fullscreen on Windows (`gpui_windows/src/window.rs:284-340,
  992-1003`), asynchronously: `is_fullscreen()` returns the old value right after
  `toggle_fullscreen()`. Studio uses neither yet.
- Preview frames are capped at 1280×720 (`studio-ui/src/frame_image.rs:18-25`).

## Requirements

- **Enter:** "Present" button in the player controls and the `Present` action (F5). Starts held
  on frame 0, or on the previous stop of the selected slide ("Present from here" in the tab).
- **Layout:** the player section renders a presentation stage (frame only, letterboxed on
  black, cursor hidden after 2 s idle) and the rest of the shell is hidden. The window enters
  fullscreen. Leaving restores the editing layout and the window's previous fullscreen state.
- **Keys** on a dedicated `FocusHandle` with the `PRESENT` key context, focused on entry; the
  presentation stage has no `PLAYER` context. →, Space, PageDown, click = next; ←, PageUp =
  previous; Home/End = first/last; number + Enter = go to slide; Esc = leave.
- **Next:** if playing, jump to the target stop and hold; if held, play to the next stop.
  **Previous:** seek to the previous stop and hold. Audio plays only while playing and never
  past the stop.
- **Changes while presenting:** promotions and retimes never start playback. A new build is
  held back until the presentation ends (the shell shows "Update ready" in the progress strip).
  A timing or slides change rebuilds the deck from the new resolved stops and holds at the
  stop nearest to the current frame.
- A thin progress strip shows slide N of M for 2 s after each change.

## Design

- `PlaybackClock` gains `play_until: Option<usize>`: `tick` clamps to
  `min(total_frames, play_until)` and stops there; `toggle` restarts from 0 only at
  `total_frames`; `seek` and `reinstall` clear `play_until`.
- `AudioService::stage` gains an `end_frame` argument: the callback writes silence from the
  exact sample of the bound. The shell passes the target stop while presenting and
  `total_frames` otherwise.
- Pure navigator `desktop/crates/studio-engine/src/slides/deck.rs`:
  `Deck::new(stops: &[ResolvedStop], total_frames)`, `next(position, playing) -> DeckStep`,
  `previous(position) -> DeckStep`, `nearest(position) -> usize`,
  `DeckStep::{PlayTo(frame), HoldAt(frame), Stay}`.
- The shell owns `presenting: Option<PresentState { deck, focus, was_fullscreen, fullscreen }>`.
  It keeps its own fullscreen flag, toggles only when `is_fullscreen()` differs from the
  wanted state, and on each render while presenting checks `is_fullscreen()`; if the OS left
  fullscreen (display unplugged, Win+↓), the presentation stays in a window and Esc does not
  toggle fullscreen back on.

## Files

- Modify: `desktop/crates/studio-engine/src/playback_clock.rs`, `tests/timeline.rs`,
  `services/audio_service.rs` (`end_frame`).
- Create: `desktop/crates/studio-engine/src/slides/deck.rs`.
- Create: `desktop/crates/studio-ui/src/shell/present.rs` (state, stage variant, keys).
- Modify: `desktop/crates/studio-ui/src/shell/{mod.rs,player.rs,layout.rs,keymap.rs}`
  (promotion and retime handling while presenting, `render_player_area` stage variant),
  `actions.rs` (`Present`, `PresentNext`, `PresentPrevious`, `PresentFirst`, `PresentLast`,
  `ExitPresent`), `slides_panel.rs` ("Present from here").

## Tasks & Steps

1. `play_until` with tests: `tick` stops exactly at the bound; a later `tick` does not move;
   `toggle` at the bound does not seek to 0; `seek` and `reinstall` clear it.
2. `AudioService` end bound with a test on the sample count written past the bound (zero).
3. `Deck` navigator with tests: next from 0 plays to stop 1; next while playing holds at the
   target; previous from a held stop holds at the one before; previous at stop 1 holds at 0;
   next at the last stop stays; `nearest` after stops change.
4. Presentation stage in the player section, `PRESENT` focus and keys, fullscreen state.
5. Promotion and retime rules while presenting.
6. Progress strip and cursor hiding.

## Verification

- Unit tests above (`-p studio-engine`).
- `#[gpui::test]`: while presenting, → dispatches `PresentNext`, not `StepForward`; a simulated
  retime while held does not start playback.
- Manual run on Signal Lab (from `prepare_examples`): present all slides; each hold equals the
  resolved stop frame (read from the clock); no audio is heard at holds; Esc restores the
  layout; unplug or Win+↓ during a presentation, then Esc returns to the editing layout without
  re-entering fullscreen. Record a short screen capture.

## Success criteria

- [ ] The held frame after each Next equals the resolved stop frame.
- [ ] No audio past a stop; audio resumes with the next slide.
- [ ] Agent promotions and timing edits during a presentation never start playback.
- [ ] Esc always returns to the editing layout and the window state from before.

## Risks

- **Audio gap on resume:** each resume restages PCM. Measure during the manual run; accept a
  short gap for v1.
- **Soft image on large screens** from the 1280×720 cap. Documented; the HTML deck is the
  full-resolution path.
