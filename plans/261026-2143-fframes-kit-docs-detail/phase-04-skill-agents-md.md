---
phase: 4
status: completed
---

# Phase 4: Agent skill and AGENTS.md

## Files (fframes-kit repo)

- `skills/fframes-kit/SKILL.md` (Agent Skills format: `name`, `description` frontmatter)
  with `references/`: the component model and `ui` API, picking a component (by intent,
  pointing to components.json / llms.txt), time containers and animation, theming, the
  explainer kits and their features, verifying a video without watching it (fframes cli
  strip/frame/inspect), common mistakes.
- `AGENTS.md`: layout, commands, conventions (docs are doctests, every component has a
  gallery scene and documented fields, regenerate the site), where things live.
- `CLAUDE.md`: `@AGENTS.md`.
- README: an "AI agents" section linking the skill, llms.txt and AGENTS.md.

Facts must be checked against the kit source and docs; examples must compile (copy from
the doctests).
