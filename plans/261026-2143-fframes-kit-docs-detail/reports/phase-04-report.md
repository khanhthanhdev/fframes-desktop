# Phase 04 report: agent docs for fframes-kit

Repo: C:\Users\Administrator\fframes-kit (branch khanhthanhdev/feat/website). Nothing committed, nothing compiled.

## Files written

- skills/fframes-kit/SKILL.md (238 lines)
- skills/fframes-kit/references/: component-model.md, timing-and-animation.md, picking-components.md, explainer-kits.md, formulas-and-latex.md, theming.md, verifying-videos.md
- AGENTS.md, CLAUDE.md (one line + `@AGENTS.md`)
- README.md: "AI agents" section before "## License"; teaser 46 s -> 78 s in the Layout table.

## Verification done

- CamelCase API names in the skill, references and AGENTS.md grepped against fframes-kit/src and fframes-kit-macros/src. Remaining non-matches of the struct/enum regex are gallery or doc-comment names found in lib.rs, external fframes types, or prose (Intro, MyVideo, ApplyMethod, AbsoluteFill). Snake_case functions checked: all found.
- just recipes and website commands checked against the justfile, website/package.json and script headers.
- Teaser length confirmed from teaser/src/lib.rs (156 beats x 0.5 s = 78 s).
- Examples are copied from the docs doctests.

## Not verified

- Doctest examples were not compiled (cargo not run).
- "Every public field has a `///` comment" comes from the task, not checked per field.
- Studio / PropsStore details live in an external repo.
- `npx skills add khanhthanhdev/fframes-kit` (repo is private) and the website URLs llms.txt, llms-full.txt, llms-small.txt, components.json and `.md` pages: not live at writing time.
- Frontmatter is a `>-` folded description; not run through a YAML parser.

- File paths named in AGENTS.md (CHANGELOG.md, scripts, gallery/build.rs, scenes/mod.rs, docs heading, .gitignore entries) were checked and exist; the 'scenes.extend(..)' line in scenes/mod.rs was not read in full.
- Shape and Sfx names were checked against prelude.rs and sfx.rs; shape types are re-exported by the prelude, their declarations were not traced.
