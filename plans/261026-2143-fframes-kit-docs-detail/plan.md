---
title: fframes-kit docs for humans and agents
status: in-progress
created: 2026-10-26
repo: C:\Users\Administrator\fframes-kit (branch khanhthanhdev/feat/website)
---

# fframes-kit docs for humans and agents

## Outcome

Every component page on https://fframes-kit.pages.dev says, beside the prose, clip and
example it has now, exactly what each field does, its type and its default: the Props
section of Remotion's pages and the Parameters list of Manim's reference. Agents get the
same docs in forms they read well: Markdown per page, llms.txt, a components.json, a
page menu that copies or opens the page in an assistant, and an installable skill.

User picked all four parts: props tables, field doc comments, agent-readable site, agent
skill + AGENTS.md.

## Constraints

- fframes-kit/docs/*.md stay the source of prose and examples (they are doctests).
- Props tables come from the kit source (one generator), never hand-copied.
- Field docs are `///` comments only; no behavior change; `just clippy` (pedantic,
  `doc_markdown`) and `just test` stay green.
- Lighthouse stays at the current gates (perf/a11y/best practices >= 95, SEO 100).
- PowerShell only; no secrets; no push. Redeploy to Pages with the existing route.

## Non-goals

- A full rustdoc-style reference of every non-component type (docs.rs does that).
- An MCP server; Accept-header content negotiation.
- Rewriting the existing prose in fframes-kit/docs.

## Phases

| # | Phase | Depends | Status |
|---|---|---|---|
| 1 | [API extractor and props tables](phase-01-api-extractor.md) | - | completed |
| 2 | [Field doc comments](phase-02-field-docs.md) | 1 | completed (2,995 lines, coverage 100%) |
| 3 | [Agent-readable site](phase-03-agent-site.md) | 1 | completed (not deployed) |
| 4 | [Agent skill and AGENTS.md](phase-04-skill-agents-md.md) | - | completed |
| 5 | Verify, review, commit, deploy | 2, 3, 4 | committed 23d39e8, d620761, 27c3b3c; deploy awaits the user |

Phases 2, 3 and 4 own disjoint files (fframes-kit/src, website/, repo root + skills/) and
run in parallel.

## Acceptance criteria

- Each of the components on a docs page (332 found by rustdoc; the old regex count of 247 missed macro-made ones) has an API section: its summary, a table of
  public fields (name, type, default, description), a source link; types that have their
  own page link to it.
- `sync-docs --check` fails when a component on a page has an undocumented public field;
  it passes at the end (100% coverage).
- `/llms.txt`, `/llms-full.txt`, `/llms-small.txt`, `/components.json` and `<page>.md`
  for every docs page are served in production; each page has Copy page / View Markdown /
  Open in Claude / Open in ChatGPT.
- `skills/fframes-kit/SKILL.md` installs with `npx skills add khanhthanhdev/fframes-kit`;
  AGENTS.md (+ CLAUDE.md importing it) describes contributor workflow.
- just test, just clippy, website unit tests, pnpm build (links valid), Lighthouse pass.
