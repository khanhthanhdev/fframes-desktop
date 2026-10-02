---
title: Desktop Phase Zero Implementation
date: 2026-10-01
summary: "Implemented Phase 0: isolated desktop GPUI spike, managed SDK setup, worker runtime IPC, ACP supervisor, and explicit selection anchor"
---

# Desktop Phase Zero Implementation

Implemented Phase 0: isolated desktop GPUI spike, managed SDK setup, worker runtime IPC, ACP supervisor, and explicit selection anchor

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.

## Remaining feasibility paths

Implemented paint-paced presentation counting, displayed-frame source inspection, configurable ACP edit/permission/cancel flow and native packaging/CI. Reviewer-found cancellation publication, cleanup ownership and build revision races were repaired. Real worker crash/restart and installed SDK worker checks passed; the assembled SDK also passed two isolated offline builds/renders. Qualification remains pending for native compositor/IME, clean accounts/platforms and authenticated ACP, which the user deferred. Details: [execution report](../reports/implementation-261001-2034-desktop-remaining-gates.md).

## Native completion checks

The first truly network-isolated account run exposed native FFmpeg downloading despite Cargo offline. SDK assembly now links the supplied static install; staged template and worker builds/renders are mandatory before promotion. Worker requests have total deadlines and shared increasing generations. Real-worker stress confirmed 1,000 painted frames and 2,000 completed renders with bounded queue/current-image counters, measured process RSS and UI heartbeat gaps. Native X11 typing, source clicks, stale-revision rejection, worker crash/restart and changed-source rebuild/selection passed; the last image survived a reaped worker while typing remained usable. ACP clarification now keeps the same session/draft and supports explicit replies and cancellation. Physical GPU/IME, Windows/macOS and authenticated ACP remain unqualified. The earlier Cargo-offline claim is superseded by the network-isolated evidence. See [completion report](../reports/implementation-261001-2213-phase-zero-completion.md).
