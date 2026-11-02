---
phase: 2
status: completed
---

# Phase 2: Field doc comments

Write a `///` comment on every undocumented public field of the components listed by
`sync-docs --check --api-report`. Sonnet subagents, one per disjoint set of directories
under fframes-kit/src (core files, kits/math, kits/physics, kits/chemistry, kits/cs,
kits/ml + kits/biology, effects/transitions).

## Rules for the comments

- Read the code that uses the field; say what it does, its unit (scene units of
  `ui.space`, pixels, seconds, 0..1), what `None`/empty means. One line where possible.
- Match the existing voice: plain, present tense, no "This field". Backtick identifiers
  (clippy `doc_markdown`). Never change code.

## Validation

`just clippy`, `just test`, `node scripts/sync-docs.mjs --check` (coverage 100%).
