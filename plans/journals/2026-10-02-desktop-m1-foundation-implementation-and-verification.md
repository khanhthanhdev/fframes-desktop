---
title: Desktop M1 foundation implementation and verification
date: 2026-10-02
summary: All four M1 development stages verified locally; release gates remain open
---

# Desktop M1 foundation implementation and verification

Completed all four M1 development stages: portable projects/SDK materialization, durable checkpoints/journal/recovery, and native shell. Restore exports an independent copy, preserving external source and Git.

Verification: 78 desktop tests, 10 root protocol/runtime tests, 5 packaging tests, formatting/build/Clippy, three explicitly executed real-worker tests. Native Linux X11 software-rendered create/import/copy/relocation, SDK setup, recovery/errors and keyboard flows were exercised and captures inspected. M0 regression passed 2000 renders/1000 presentations. Native quit during setup left no owned app/compiler/worker/display processes.

Corrected review findings: Cargo ancestor-workspace leakage in CPU SDK verification; missing focus navigation; generic asset/SDK errors; lost watcher hints; failed-journal continuation; late process spawning after shutdown. Real dirty Git import preserves HEAD, diff and untracked bytes.

Delivery: local and uncommitted. Playback/agent transactions/presets remain later milestones. Physical display/GPU/IME, Windows/macOS interactive and authenticated ACP qualification remain pending. Evidence is in the plan phases and .amp/in/artifacts; no release claim.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
