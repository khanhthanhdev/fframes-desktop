---
phase: 3
status: completed
---

# Phase 3: Agent-readable site

## Files (website/)

- `scripts/sync-docs.mjs` + `scripts/lib/agent-docs.mjs` (+ test): from the same page
  text, write `public/docs/<cat>/<slug>.md` (SceneVideo replaced by the scene name and
  clip URL), `public/llms.txt` (llmstxt.org format: title, summary, sections of links to
  the .md pages with descriptions), `public/llms-full.txt` (every page), `public/llms-small.txt`
  (API sections and signatures only), `public/components.json` (name, feature, page, md,
  scene, clip, summary, fields). All gitignored output.
- `src/components/PageActions.astro` in the PageTitle override: Copy page (fetches the
  .md), View as Markdown, Open in Claude, Open in ChatGPT. Small inline script, keyboard
  accessible, no layout shift.
- `public/_headers`: `text/markdown; charset=utf-8` for `*.md`, `text/plain` for llms*.txt.
- `src/content/docs/docs/ai.mdx`: "Use with AI agents" page (llms files, .md URLs,
  components.json, the skill install command, a prompt template). In the sidebar.

## Validation

Unit tests, pnpm build, Lighthouse, curl the outputs after deploy.
