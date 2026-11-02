---
title: Plan the fframes-kit component library
date: 2026-10-23
summary: Detailed 12-phase plan for the component library; crate renamed to fframes-kit after a crates.io name clash
---

# Plan the fframes-kit component library

Wrote the implementation plan in `plans/261023-1255-components-library/` from the five research
reports of 2026-10-23. The component model report (Remotion-style `Ui`, `Component`, `Sequence`)
is the contract; the strategy report's contract and roadmap are superseded.

While checking the workspace, the planned name `fframes-components` turned out to be a
third-party crates.io crate (0.1.2, ported from remocn) that `examples/fframes-studio-intro` and
`examples/vinuni-future-rises` already use. None of the research reports mentioned it. The
library is now `fframes-kit`; the 0.1.2 dependency stays.

Decisions from the user: Studio props panel after the general layer and math, CeTZ vendored
in a separate `fframes-kit-cetz` crate so the kit stays MIT, struct update syntax with themed
defaults. Studio prop edits are planned as `props.json`, mirroring how timeline edits live in
`timeline.json` and reach the worker through `SetTiming`.

Core changes are deliberate and additive: `Frame::text_width`/`text_fit` lifetimes, component
tags in `svgr!`, and the props document. The first two phases (core crate and the Typst
formula spike) can start in parallel.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
