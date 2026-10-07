---
title: Desktop M4 presets and scoped editing plan
date: 2026-10-05
summary: Created and validated the phased M4 implementation plan
---

# Desktop M4 presets and scoped editing plan

## Outcome

Created and validated the phased Desktop M4 implementation plan, based on the desktop roadmap and M0–M3 implementation/qualification evidence.

## Decisions recorded

- Use a GPUI-free preset model and portable, bounded directory packages with intentionally limited CSS import.
- Keep project-local style snapshots and overrides durable; expose resolved values through feature-gated `fframes::Styles`.
- Freeze task scopes to exact source/preview identity; reject stale queued scopes rather than silently remapping them.
- Preserve M3 candidate-validation broadening and distinguish development checks from authentic-provider/platform qualification.

## Validation

- Four sequential phases indexed and parsed; 44 tasks remain unchecked.
- Plan validation passed; 40 sampled repository claims verified; no broken local links.
- Planning-only deliverable; no implementation tests were run.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
