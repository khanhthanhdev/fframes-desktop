---
phase: 1
title: "Slides document and engine session"
status: pending
priority: P1
effort: 3d
dependencies: []
---

# Phase 1: Slides document and engine session

<!-- Updated: Red Team 2026-10-27 - retimed timelines reach the engine (RT1), overlap-aware
resolution (RT2), one undo history for run-time documents (RT8), candidate validation of
slides.json (RT9), symbolic clip-end stops and unresolved handling (RT14), consistent limits (RT15) -->

## Goal

Add `slides.json`, a run-time document that lists slide stops and speaker notes. Add a
controller session that reads, validates, writes and watches it like `props.json`, plus pure
logic that resolves stops to video frames against the current laid-out timeline.

## Context

- Document session pattern: `DocFile`, `read_doc`, `write_doc`, `recover_doc` in
  `desktop/crates/studio-engine/src/timing/file.rs:31-53, 108-146, 197-278, 311`. Props are the
  closest model: `desktop/crates/studio-engine/src/props.rs:18-120` and
  `desktop/crates/studio-engine/src/controller/props.rs:24-237` (size check before write at
  `:97-100`).
- Run-time documents are excluded from the build revision through `run_time_document` in
  `desktop/crates/studio-project/src/revision.rs:59-67`; build materialization skips them
  (`desktop/crates/studio-engine/src/build_materialization.rs:158, 276-282`).
- Sessions load at `controller.rs:356-357`, recover at `controller.rs:207-210`, observe outside
  writes on rescan at `controller.rs:604-605`.
- **Timelines reach the engine only on a new build.** `learn_timeline` runs only on
  `news.ready` (`engine/handle.rs:552-557`). A timing edit produces `PreviewEvents::retimed`
  (`services/preview_coordinator.rs:936-952`), which the shell installs itself
  (`studio-ui/src/shell/player.rs:66-69, 105-162`) and never forwards: `PreviewNews` carries
  only `compiled`, `error`, `ready` (`player.rs:94-100`).
- Laid-out frames: `PreviewTimelineResponse.scenes[*]` with `start_frame`, `end_frame` and
  `clip: Option<PreviewClipTiming>` (`fframes-studio-protocol/src/preview.rs:88-161`).
  `clip` and `scene_frames` are absent from workers without `set_timing_v1`
  (`preview.rs:101-103, 115-118`). Scene frame = video frame − `start_frame` +
  `trim_start_frames` (`fframes/src/fframes_context.rs:128-135`), and an overlapped scene's
  range includes its overlaps (`fframes/src/video.rs:164-190`).
- Agent edits are candidates, validated and then published through the edit transaction.
  Candidate validation reads only `timeline.json`
  (`services/candidate_runner.rs:293-302, 393-410`). Task undo requires untouched "after"
  bytes on every touched path (`edit_transaction/mod.rs:187-215, 636-637`).
- Undo today: Ctrl+Z in the Timeline context and the header buttons run only
  `TimelineCommand::Undo/Redo` (`studio-ui/src/shell/keymap.rs:183-188`,
  `shell/header.rs:44-66`).

## Document format

```json
{
  "version": 1,
  "stops": [
    { "scene": "intro", "at": "end", "title": "Welcome", "notes": "Say hello." },
    { "scene": "features", "at": 40, "notes": "First bullet." }
  ]
}
```

- `scene` is the clip key used by `timeline.json` (`editor_instance_key()`, else `name()`).
- `at` is a frame in the scene's own frame numbers, or `"end"`: the last frame of the clip
  before the next clip starts overlapping it. `"end"` follows trims automatically.
- No stops (missing file or empty `stops`) means **default stops**: `"end"` of every clip.
- Limits (one budget): the whole document at most 512 KiB; at most 300 stops; `title` at most
  256 bytes; `notes` at most 8 KiB; `scene` within the timing key bounds. Unknown fields are
  rejected; `version` must be 1. Bulk commands check the encoded size before writing, like
  props.

## Resolution rules (pure, in `studio-engine/src/slides/resolve.rs`)

- Input: the document and a laid-out `PreviewTimelineResponse` whose `timing_generation`
  matches the timing session. A timeline without `clip` data resolves nothing and reports
  "the preview worker is too old for slides; rebuild with the current SDK".
- A clip's drawn scene frames are `trim_start_frames .. trim_start_frames + (end_frame - start_frame)`,
  which includes overlaps. `"end"` resolves to video frame
  `min(end_frame, next_clip.start_frame) - 1`, so a default stop never sits inside a
  cross-fade into the next clip.
- A numeric `at` maps to `start_frame + (at - trim_start_frames)` of the first clip with that
  key whose drawn range contains `at`. A split scene resolves to its first clip containing the
  frame.
- A stop whose frame is not drawn by any clip is **unresolved**: shown in the Slides tab with
  the reason, skipped by Present mode and export with a warning, never moved silently.
- Resolved stops are sorted by video frame; equal frames merge (first title and notes win,
  warning reported). Stops at frame 0 are dropped with a warning. The video's last frame is
  always an implicit final stop.
- Output: `Vec<ResolvedStop { video_frame, title, notes, source: Explicit | Default }>` and
  `Vec<SlidesProblem>`.

## Files

- Modify: `desktop/crates/studio-project/src/revision.rs` (`SLIDES_FILE = "slides.json"`, add to
  `run_time_document`, `slides_file()` like `props_file()`).
- Create: `desktop/crates/studio-engine/src/slides/{mod.rs,doc.rs,resolve.rs}`.
- Create: `desktop/crates/studio-engine/src/controller/slides.rs` (session, `write_slides`,
  `observe_slides_file`).
- Modify: `desktop/crates/studio-engine/src/timing/history.rs` and `controller/timing.rs`: one
  **run-time document history** whose entries are tagged `Timing` or `Slides`, so the existing
  Undo/Redo in the Timeline context and the header undoes the latest timing **or** slides
  gesture in order.
- Modify: `desktop/crates/studio-engine/src/engine/handle.rs` and `engine/backend.rs`
  (`PreviewNews::retimed: Option<Arc<ReadyPreview>>` handled in `gate`, which records the
  latest laid-out timeline; slides commands; `slides_snapshot`), `controller.rs`, `lib.rs`.
- Modify: `desktop/crates/studio-ui/src/shell/player.rs` (forward `events.retimed` in
  `PreviewNews` as well as installing it).
- Modify: `desktop/crates/studio-engine/src/services/candidate_runner.rs` (parse and resolve the
  candidate's `slides.json` against the candidate timeline next to the `TIMING_FILE` check;
  fail the candidate with `FailureKind::Source` and the reason).
- Modify: `desktop/crates/studio-engine/src/edit_transaction/mod.rs` (task undo treats a
  run-time document changed after the task as "keep current"; it does not block undoing the
  task's code).
- Modify: `desktop/crates/studio-project/templates/AGENTS.md` (`slides.json` format, scene keys,
  `"end"`, limits, that no rebuild is needed and that notes end up in shared decks).
- Create: `desktop/crates/studio-engine/tests/slides.rs`.

## Tasks & Steps

1. `slides::doc`: `SlidesDoc` (serde `deny_unknown_fields`, `at` as frame or `"end"`), `parse`
   with limits and reasons, stable `to_json`.
2. `slides::resolve` with the rules above.
3. `SLIDES_FILE` in `studio-project`; a `slides.json` edit leaves `build_revision()` unchanged.
4. `PreviewNews::retimed`, forwarded by the shell and recorded in `gate`, so the engine always
   holds the timeline the person sees.
5. `SlidesSession` modeled on `PropsSession`; load, recovery, outside-write observation.
6. Shared run-time document history (timing and slides).
7. Engine commands: `SetSlides`, `AddStop`, `MoveStop`, `RemoveStop`, `SetStopText`,
   `MaterializeDefaultStops` (writes `"end"` stops). Each is one gesture in the shared history.
   Events carry resolved stops and problems.
8. Candidate validation and task undo handling for `slides.json`.
9. Template `AGENTS.md` section.

## Verification

- Unit tests in `slides/doc.rs` and `slides/resolve.rs`, chosen to catch likely mistakes:
  a stop in a trimmed-start clip (off by trim); a frame inside a clip's `Overlap::Previous`
  part (resolved, not unresolved); `"end"` of A when B overlaps A (lands before the
  cross-fade); a reordered timeline (sort by video frame); a split clip; three-clip defaults;
  a duplicate frame; a timeline without `clip` data; a document over each limit.
- `tests/slides.rs`: trim a clip, then stops resolve against the retimed timeline with no
  rebuild; trim, then move a marker, then two undos revert marker then trim; outside write
  conflict; crash recovery; an agent candidate with an invalid `slides.json` fails
  validation; task undo after the user edited notes keeps the notes and reverts the code.
- `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project -p studio-engine`.

## Success criteria

- [ ] A project without `slides.json` resolves one stop per clip end (outside cross-fades) plus
      the video end.
- [ ] A trim, split or move updates resolved stops at once, without a rebuild.
- [ ] Invalid documents are refused whole with a reason, also when the agent writes them.

## Risks

- **Overlap edge cases** in hand-written stops: covered by the overlap unit tests.
- **Shared history** changes the timing undo code; the existing `tests/timing.rs` undo cases
  must pass unchanged.
