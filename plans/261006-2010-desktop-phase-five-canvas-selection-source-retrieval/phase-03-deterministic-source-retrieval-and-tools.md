---
phase: 3
title: "Deterministic source retrieval and tools"
status: complete
priority: P2
effort: "5–7 engineer-days"
dependencies: [1, 2]
---

# Phase 3: Deterministic source retrieval and tools

## Goal and context

Resolve explicit selected-object anchors into containing Rust implementations and bounded helper/asset/token context, then expose identical read-only results to GUI, CLI and MCP. Read [selection architecture](../../docs/desktop/architecture.md#retrieval-strategy), [baseline](../reports/planning-261006-2010-m5-baseline-and-validation.md), `task_scope.rs`, `agent_tools.rs` and `agent_tools/backend.rs`.

M4 scans exact identifier text; comments/strings are not syntax evidence. Existing tools are six methods, task-bound capabilities, immutable restored revisions and shared builds. Add three methods without creating a second broker, compiler or authorization route.

## Requirements and proposed retrieval contract

- Build a deterministic syntax/symbol index over hash-verified immutable inventory bytes, keyed by project/source revision and index schema version. Stable sort files/modules/symbols/edges. No rustc invocation, macro execution or code evaluation for indexing. Use a Rust parser after an isolated spike proves byte-span extraction; `syn` v2 is already used by svgr-macro but is not yet a desktop dependency. Verify parser span-to-UTF-8 byte mapping before adopting it.
- Locate inline/file modules, declarations, containing impl/method/function, imports/reexports/aliases, syntactically visible helper calls, and explicit media/token bindings. Represent exact anchors separately from syntactic candidates; do not claim type resolution, expanded macros, runtime dispatch or full cfg evaluation. Unknown/ambiguous dependencies are explicit and trigger broader context/validation.
- Resolve unique explicit markers/registration keys, ordered spans, UTF-8 boundaries, expected symbol containment and file hash. Duplicate/missing markers or changed hashes reject exact lookup. Generated/cache/private/app-local files and links are not source candidates. Read immutable checkpoint objects or verified materializations through existing safe source APIs; never canonicalize then read arbitrary live paths.
- A retrieval response carries revision, object tuple/anchor, relative path, symbol, file digest, span, bounded snippet, confidence/reason and related edges. Missing anchors return scene/syntax hints with uncertainty. File parse errors are per-file diagnostics, never a fabricated precise map.
- Bound index inputs to the existing M4 baseline of 256 Rust files, 1 MiB/file and 8 MiB total initially; allow bounded on-demand requests for omitted source files so truncation does not make context unreachable. Proposed lookup limits: 8 snippets, 4 KiB each, 32 KiB attached text, dependency depth 2 and 32 edges. Explicitly report truncation/unsupported constructs; continuations bind to immutable revision/query, never an arbitrary host file path.
- Retain at most four revision indexes and 32 MiB total in memory initially, with eviction and build/source leases respected. Build off UI threads; cancellation checks occur per file and dependency batch. Record input/output/cache constants with tests and tune only from measured fixtures.

## Tools and revision behavior

| Method | Input | Result |
|---|---|---|
| `selection_context` | Optional asserted project/revision; requested frozen selection or revision-bound object/frame tuple | Validated selection, visible geometry/support, scene/boundaries, anchor pointers and bounded app-owned evidence refs |
| `source_lookup` | Bounded explicit anchor, project-relative path/symbol or continuation; asserted revision | Hash/span-verified snippets, containing impl, helper/import/asset/token candidates and uncertainty |
| `style_context` | Bound revision and optional object/token filter | Active project preset snapshot, resolved token types/values/origins, explicit object binding names and hashes |

Separate the frozen task-base selection from current draft/candidate context. An active writer holds the draft gate, so base selection/lookup/style requests must remain answerable from the task's captured base through an explicitly authorized base view. Calls requesting draft still acquire the existing writer gate and return Busy if needed; no call may quietly answer a base object using newer draft bytes. Candidate/draft object lookup requires metadata and anchors generated for that exact captured revision; deletion returns NotFound/StaleRevision, never remaps by name.

`style_context` reads frozen M4 preset state and registered token bindings; it does not infer that an arbitrary literal or helper is token-bound. Return no-active-preset/no-binding explicitly. Preserve existing capability expiry, task liveness, project assertion, 16 queued calls, two tool workers, 256 KiB text envelope, 8 MiB image artifact and artifact TTL limits. New budgets are lower than these outer limits; style/source output is context data, not trusted agent instructions.

## Files and ownership

Paths are relative to `/root/fframes-desktop`.

| Action | Files | Purpose |
|---|---|---|
| Create | `desktop/crates/studio-project/src/source_index.rs`, `desktop/crates/studio-project/tests/source_index.rs` | GPUI-free parser/index and bounded lookup |
| Modify | `desktop/crates/studio-project/src/lib.rs`, `desktop/crates/studio-project/Cargo.toml`, `desktop/Cargo.toml`, `desktop/Cargo.lock` | Parser dependency, exports and pinned resolution after proof |
| Modify | `desktop/crates/studio-engine/src/task_scope.rs`, `desktop/crates/studio-engine/src/controller.rs` | Resolve immutable anchors/context; preserve legacy best-effort lookup |
| Modify | `desktop/app/src/agent_tools.rs`, `desktop/app/src/agent_tools/backend.rs`, `desktop/app/src/agent_tools/client.rs`, `desktop/app/src/agent_tools/mcp.rs` | Strict new method parsing, dispatch, results and MCP schemas |
| Modify | `desktop/app/src/bin/studio_tools.rs`, `desktop/app/src/bin/studio_mcp.rs`, `desktop/app/src/agent_workflow/tools.rs` | CLI/help and revision-bound base view/task registration |
| Modify | `desktop/app/tests/agent_tools.rs`, `desktop/app/tests/build_sharing.rs` | Parity, authorization, bounded replies and compile deduplication |
| Create | `desktop/app/tests/selection_tools.rs` | Selection/source/style tool lifecycle and revision tests |

`agent_tools/broker.rs` remains the owner of grants; change it only if exposing an explicit base view requires a new bounded binding, preserving its security invariants. No persistent database schema is required for derived indexes.

## Tasks and steps

- [x] Spike parser span recovery against UTF-8, comments, raw strings, CRLF, nested modules and parse errors. Choose the smallest deterministic parser route that meets source-span checks; if spans cannot be proven, keep explicit marker spans and return syntax candidates rather than inventing precision.
- [x] Implement immutable inventory ingestion, per-file errors, sorted symbols and syntax edges. Index inline/file modules and aliases; reject outside-root #[path] and unsupported module resolution as explicit gaps. Restrict dependency exploration and attach counts/truncation.
- [x] Validate explicit source anchors against unique marker interiors and containing syntax, then retrieve containing impl and referenced local helpers. Test duplicate symbols, repeated object keys, helper aliases/reexports, macros/cfg ambiguity, missing files, asset paths and typed Styles accessor binding names.
- [x] Build style_context from the exact resolved style snapshot and explicit registration. Return token name/type/value/origin, preset/override and digest evidence; any project/style/source mismatch refuses the query. Test aliases, overridden values, absent presets and absent bindings.
- [x] Extend ToolCall parsing/schema/name mappings, backend exhaustive matches, MCP tool list, CLI help and error parity. Add captured base context authorization so initial selection queries work while the writer is active. Preserve fixed candidate and draft snapshot semantics; validate source/revision assertions before fetching context.
- [x] Add cache eviction, cancellation, bounded on-demand omitted-file lookup/continuations and release on task/project teardown. Render evidence through existing BuildService/ArtifactStore; syntax-only calls do not compile. Prove no duplicate compilers or stale index reuse when style/assets/source changes.

## Verification and success criteria

```bash
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project --test source_index
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test agent_task_scope
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test agent_tools --test selection_tools --test build_sharing
```

Pass when repeated identical queries return identical ordered context; anchors point to real immutable UTF-8 bytes; missing/ambiguous/unsupported cases are labelled; base requests work during an active draft writer; draft requests obey Busy; cross-task/project/expired/stale requests fail; all three methods agree across direct, CLI and MCP access; resource ceilings and old six-method behavior hold.

## Risks, security and rollback

Rust syntax cannot prove every dependency: expose gaps and preserve broad validation. Do not search app credentials, agent config, Git internals or files outside the captured project. Worker-supplied anchors are assertions, never authorization to read paths. Discard derived indexes on schema/revision mismatch; disabling new methods leaves old tools and scene/range packets functional. If a durable schema change becomes necessary, stop and design a backed-up migration first.

Next: Stage 4 freezes retrieved context in the existing agent transaction.
