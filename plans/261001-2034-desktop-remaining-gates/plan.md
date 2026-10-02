# Complete the remaining desktop feasibility paths

Outcome: implement 1,000 frame-completion-paced GPUI presentations, displayed-frame source selection, a configurable real ACP session/edit/cancel flow, and native packaging/CI with honest qualification records.

Constraints: preserve existing SDK/setup work; no simulated provider success; no claims of native compositor, clean-account, or authenticated evidence from unit tests. Retain the minimal spike UI. Signing, updater, full timeline, and production provider UX remain outside this work.

## Execution and acceptance

- [x] Presentation: conversion runs off-thread; exactly one pending image; only a painted image acknowledged on the subsequent GPUI frame advances the 1,000 counter. Abort/error cannot report success. Test duplicate/stale acknowledgments and run boundaries.
- [x] Selection: metadata belongs to the displayed worker request/revision/generation; actual preview bounds and contain-fit geometry map clicks; validate root containment, hash, unique markers, UTF-8 span and symbol; show source snippet. Cover stale metadata and invalid paths/spans.
- [x] ACP: newline-delimited JSON-RPC v1 initialize/new/prompt/update/permission/cancel; bounded streams, authoritative completion, draft isolation, rebuild/worker restart, configurable executable/arguments and provider-managed auth. Run protocol integration tests; real authentication remains NOT_RUN without credentials.
- [x] Packaging: native app/worker/companion artifacts, Windows wrapper, dependency inventory/checksums, native CI matrix, and qualification validation. CI build evidence must remain separate from interactive/sterile host gates.
- [x] Verify touched Rust packages, formatting, Clippy and script/schema checks; review contracts and update the owning decision record with measured results only.

## Validation boundaries

Linux headless checks are available locally. Native compositor/IME and Windows/macOS runs need native runners. Live provider qualification requires an adapter/account. These checks remain open even when implementation passes automated tests.

## Result

Implementation checks passed; native and authenticated qualification are still open as described above. See [execution report](../reports/implementation-261001-2034-desktop-remaining-gates.md) and [native procedure](../../docs/desktop/phase-zero-feasibility.md). The adapter remains configurable, with live qualification deferred by user choice.
