# Planning validation

Status: planning only; plan pending, 4 sequential stages, 32 unchecked execution tasks.

The controller offered the qualification fork: develop M1 using recorded Linux evidence with remaining gates open, or block M1 until all M0 qualification gates pass. User answer: “Plan M1 development now using the recorded Linux evidence; keep qualification gates open (Recommended)”. This confirms development scope only. Physical GPU/display/IME, Windows/macOS interactive qualification and authenticated ACP qualification remain open.

## Validation session — 2026-10-02

One material question was asked; the accepted roadmap resolved the other scope decisions. Question: “Phase 0 qualification is still pending for physical-display/IME, Windows/macOS and authenticated ACP. How should Phase 1 depend on those remaining gates?” Options: “Plan M1 development now using the recorded Linux evidence; keep qualification gates open (Recommended)” or “Require all remaining Phase 0 qualification gates before starting M1”. The user selected the first option. The index and Stage 1 entry condition reflect that answer.

CLI scaffolded this directory with UTC timestamp `261002-0434`; it was preserved unchanged. Live `ak plan add-phase` leaves the index table unchanged; `ak plan reindex` does not write files, so the file-owned index table was updated to list all four generated phases. No status cells were manually advanced.

`ak plan validate` passed. `ak plan parse` reports pending, four phases and zero completed tasks. Local Markdown link targets pass. Index is 56 lines. Source references, current caller inventories and existing test targets were checked with rg/nl. No implementation tests, native GUI walkthrough, build or qualification was run by this planning task.

Planned new test targets and files are explicitly proposed and become runnable only after implementation. Effort totals are tentative. No code/docs outside the new plan directory were edited by the planner.

## Controller consistency sweep

The controller read the index and all four execution files, checked SDK build paths, revision/copy helpers and their consumers, the media macro's working-directory behavior, existing CI gates and qualification status against source. The review corrected proposed Rust module filenames, removed milestone IDs from product empty-state text, and recorded the confirmed development gate throughout the plan. Dependencies are sequential and acyclic; planned files are distinguished from existing files. No unresolved contradiction remains. The main roadmap now links to this execution plan.
