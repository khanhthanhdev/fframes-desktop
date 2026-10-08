---
title: M7 review fixes for release verification
date: 2026-10-08
summary: "Bind qualification to trusted evidence and worktree content, independently verify MP4 output, and journal SDK rollback recovery."
---

# M7 review fixes for release verification

## Findings fixed

- Require each supplied SDK artifact record to exactly match the signed compatibility manifest before extracting its destination metadata.
- Require Ed25519 signatures from explicitly trusted qualification keys, bind signed evidence to the source commit/worktree digest and exact artifact identity, and verify the source worktree before release eligibility.
- Independently open and decode representative video frames and bounded audio samples in the app before export publication; reject invalid or truncated output.
- Journal SDK rollback intent and recover Prepared/Finalizing swaps after interruption without replaying a completed rollback.

## Verification

- `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --lib`: passed (148 tests).
- `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-sdk --lib`: passed (32 tests).
- `cargo test -p fframes-media --lib`: passed (5 tests).
- `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test native-export`: passed (3; real SDK_BUNDLE worker test ignored because no assembled bundle is available).
- `cargo clippy` for fframes-studio, studio-sdk and fframes-media with `-D warnings`: passed.
- M7 qualification validator tests: passed (11); packaging tests: passed (48); workflow YAML and M7 JSON schema validation: passed.

## Remaining release state

Consumer publication remains disabled, trusted qualification evidence keys are not configured, and the M7 target ledger remains unqualified. Native install/update, signed-platform and authentic/physical qualification gates remain open; this change does not claim M7 release qualification.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
