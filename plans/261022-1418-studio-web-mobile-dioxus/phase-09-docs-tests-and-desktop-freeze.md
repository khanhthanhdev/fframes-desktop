---
phase: 9
title: "Docs, tests and desktop freeze"
status: pending
priority: P2
effort: "1w"
dependencies: [7, 8]
---

# Phase 9: Docs, tests and desktop freeze

## Goal

Document the server, the web and phone clients and the edits document; make the new test sets part of
the Windows gate; mark the GPUI app as frozen.

## Requirements

- `docs/desktop/README.md` and `docs/desktop/architecture.md` describe the server, clients, protocol,
  security model and the authority table with `edits.json`.
- Root `AGENTS.md` documents `EditDoc` beside the timing document (section "Scenes"), including
  `edits_file`, `RenderOptions::edits` and `Previewer::set_edits`.
- The GPUI app is labelled frozen in the README: maintained for fixes only.
- New test sets run locally on Windows: `studio-api`, `studio-canvas`, `studio-server`, fframes edits
  tests, the web client browser tests.

## Related code files

- Modify: `docs/desktop/README.md`, `docs/desktop/architecture.md`, `AGENTS.md`
- Create: `docs/desktop/web-and-mobile.md` (setup, pairing, troubleshooting)
- Modify: `justfile` (recipes: `studio-server`, `studio-web`, `studio-mobile-android`)

## Implementation steps

1. Write `docs/desktop/web-and-mobile.md` and update README and architecture.
2. Update `AGENTS.md` for `EditDoc`.
3. Add `just` recipes.
4. Run all test sets on Windows and record results in `reports/verification.md`.
5. Re-run the GPUI app's Windows test sets to confirm the freeze did not break it.

## Todo

- [ ] Docs
- [ ] AGENTS.md
- [ ] just recipes
- [ ] Windows test run
- [ ] GPUI regression run

## Success criteria

All acceptance criteria in `plan.md` are checked with recorded evidence.
