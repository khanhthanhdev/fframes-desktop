---
title: "M6: Provider qualification and controlled handoff"
description: "Qualify four ACP providers and safely switch or restore sessions in the native Studio workflow."
status: in-progress
priority: P1
effort: "18–27 engineer-days (tentative)"
branch: main
tags: [feature, desktop, agent, qualification]
blockedBy: []
blocks: []
created: 2026-10-07
---

# M6: Provider qualification and controlled handoff

## Overview

Deliver native provider selection, stopped-writer handoff and persistent supported session restoration using the implemented M3–M5 workflow. Qualify Claude, Codex, Pi and Antigravity through their current ACP distributions; recommend exactly the best two only after comparable authentic evidence. Keep every other connector experimental until it passes the same suite.

## Goals

| # | Goal | Priority |
|---|------|----------|
| 1 | Four provider profiles and honest evidence contracts | P1 |
| 2 | Controlled handoff preserving accepted and draft variants | P1 |
| 3 | Native picker, restart continuity and full provider qualification | P1 |

## Phases

| # | Phase | Status | Dependency | Tentative effort |
|---|-------|--------|------------|------------------|
| 1 | [Provider profiles and evidence contracts](phase-01-start.md) | Complete | Implemented M3–M5 | 4–6d |
| 2 | [Controlled handoff and restoration](phase-02-controlled-handoff-and-restoration.md) | Pending | 1 | 7–10d |
| 3 | [Native picker and provider qualification](phase-03-native-picker-and-provider-qualification.md) | Pending | 1, 2 | 7–11d |

## Success Criteria

- [ ] Every provider records observed adapter/CLI versions, platform, authentication route, negotiated capabilities and measured lifecycle/tool/visual behavior; credentials never enter projects or evidence.
- [ ] Switching stops and verifies the outgoing writer before any incoming writer starts, preserves both source and draft variants, and requires an explicit continuation choice.
- [ ] Handoff context binds current revision, task outcome, frozen selection, style and unresolved diagnostics within existing budgets; native conversation memory is explicitly not transferred.
- [ ] Restart resumes an eligible supported session at the stable draft cwd; unsupported, incompatible or failed restoration visibly offers a fresh session with explicit context.
- [ ] Each advertised connector passes authentic create/edit/export, Apply/Undo/reopen, interruption and resource checks; recommendations follow evidence rather than provider names.
- [ ] The export check uses the existing CLI backend; native export UI, installer and clean-machine release acceptance stay dependent on M7.

## Dependencies and execution

M3–M5 implementation is reused; [M5 ledger](../../desktop/qualification/m5-results.json) passes development only. Its authentic/provider, physical-device and Windows/macOS gaps remain open. M3 narrative plans are historical; source and qualification ledgers own implemented behavior.

Execute sequentially: [provider contracts](phase-01-start.md) → [handoff/restoration](phase-02-controlled-handoff-and-restoration.md) → [native picker and qualification](phase-03-native-picker-and-provider-qualification.md). Shared files transfer ownership only after the preceding gate; no parallel edits. Tentative estimates are 4–6d, 7–10d and 7–11d respectively. Durable checkboxes track execution; use the live plan CLI for status changes.

## Scope and evidence

Reuse stable drafts, task journals, writer scopes, immutable retrieval, MCP/CLI tools, candidate validation and guarded publication. No new native driver without a recorded concrete ACP blocker; no new orchestrator or export UI.

See [provider research](../reports/research-261007-0712-desktop-phase-six-providers.md), [baseline and validation](../reports/planning-261007-0712-m6-baseline-and-validation.md), [architecture](../../docs/desktop/architecture.md#4-agent-integration-and-handoff) and [roadmap](../../docs/desktop/implementation-plan.md). Research versions are observations to verify at execution, not guessed pins. Missing accounts, platforms or evidence produce blocked/not_run gates and never qualify a connector.

Unresolved product decisions: none. Runtime prerequisites and provider capability gaps are recorded by the qualification stages.

## Validation and review

Hard planning route; full M6 scope held. The roadmap already fixes the product choices, so no new interview question is required. Independent security, assumption and failure reviews verified 42 source/contract claims. Three Medium ordering findings were accepted and incorporated: preserve the queue before stop, settle publication/Undo before revision choices, and reuse the tool snapshot gate. No unresolved Critical/High findings or contradictions remain. See [validation evidence](../reports/planning-261007-0712-m6-baseline-and-validation.md).

The CLI format check and ownership-aware local link/file sweep pass. All stages remain pending; provider qualification is future execution evidence. Ready to implement using `/ak:cook /root/fframes-desktop/plans/261007-0712-desktop-phase-six-provider-qualification-handoff/plan.md`.

<!-- slug: desktop-phase-six-provider-qualification-handoff -->
