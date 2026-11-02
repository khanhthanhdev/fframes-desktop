# fframes-kit release readiness (2026-10-26)

Repo: `github.com/khanhthanhdev/fframes-kit` (private), local `C:\Users\Administrator\fframes-kit`,
`main` at `86554d6`, clean and in sync with origin. Crates: `fframes-kit` 0.1.0 and
`fframes-kit-macros` 0.1.0, both MIT, both names free on crates.io. Depends on crates.io
`fframes` ^1.3.0 (latest stable 1.3.0, 1.3.1-rc.6 in flight).

## Verified green

| Check | Result |
| --- | --- |
| `cargo package -p fframes-kit-macros -p fframes-kit` (with verify) | passes; kit 2.15 MB, macros 0.01 MB (limit 10 MB) |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo test -p fframes-kit --all-features` | 731 passed, 0 failed, 3 ignored (unit + doctests of the references) |
| `cargo doc --no-deps -p fframes-kit --all-features` with `RUSTDOCFLAGS=-D warnings` | clean |
| `just check-wasm` (gallery bridge, wasm32) | clean |
| TODO/FIXME/`todo!`/`unimplemented!` in kit and macros sources | none |
| Bundled assets | sfx CC0 (documented), New Computer Modern fonts under GUST (license copied), mitex specs license copied |
| `just e2e cpu` (gallery snapshots) | passes; 160 of 167 identical, 7 formula frames within tolerance |
| `cargo fmt --all --check` | clean |

## Fixed on 2026-10-26 (uncommitted in the kit repo)

| Problem | Fix |
| --- | --- |
| No README in the crate | `readme = "../README.md"` in `fframes-kit/Cargo.toml`; the packaged crate now contains `README.md` |
| README links break on crates.io | links point at `https://github.com/khanhthanhdev/fframes-kit/blob/main/fframes-kit/docs/...`; they work only once the repo is public |
| No docs.rs metadata | `[package.metadata.docs.rs]` with `all-features = true` and `rustc-args = ["--cfg", "fframes_docsrs"]`, as fframes does |
| Metadata missing | `rust-version`, `documentation`, `keywords`, `categories` on both crates |
| Macros could drift from the kit | workspace dep is `version = "=0.1.0"` |
| No LICENSE inside the crates | `LICENSE` copied into `fframes-kit/` and `fframes-kit-macros/`; both packages contain it |
| Install docs said "git, private repo" | README has `cargo add fframes-kit` lines and no longer says "not on crates.io"; `getting-started.md` uses `fframes-kit = "0.1"` and keeps git `main` as an option |
| No CHANGELOG | `CHANGELOG.md` with a 0.1.0 entry; every type it names exists in the source |

Verified after the fixes:

- `cargo package` with verify for both crates, `cargo clippy --all-targets -D warnings`,
  `cargo fmt --check` and `cargo test -p fframes-kit --all-features` (731 passed) are green.
- `cargo publish --dry-run -p fframes-kit-macros` succeeds. The kit's dry run needs the macros
  crate on crates.io first; `cargo package` above already verifies the pair.
- `rust-version = "1.92"`: it is the highest `rust-version` in the dependency graph (typst 0.15,
  hayro). `cargo +1.92.0 check -p fframes-kit-macros -p fframes-kit --all-features` passes.
- docs.rs build reproduced locally: `cargo doc --no-deps -p fframes-kit --all-features
  --target x86_64-pc-windows-msvc` with `RUSTFLAGS`/`RUSTDOCFLAGS` `--cfg fframes_docsrs`,
  `-D warnings` and `DOCS_RS=1` passes. The same flags without `--target` break rapier2d and
  typst-library, because the cfg then also reaches proc-macros and build scripts. docs.rs
  always passes `--target`, so this does not apply there.

## Still open

1. **The repository is private (blocks publishing).** The crates.io `repository` link and
   the README's doc links 404 until it is public. This is the user's decision.
2. **Publish order.** `cargo publish -p fframes-kit-macros` first, then
   `cargo publish -p fframes-kit`. Publishing is permanent.
3. **Tag and release.** Commit the fixes, tag `v0.1.0` after publishing, create the GitHub
   release, and check the first docs.rs build.
4. **CI was not added**, on purpose: the user's standing rule is that local Windows runs are
   the gate.
5. **fframes-desktop consumers** (`fframes-studio-kit`, `desktop/fixtures/kit-props`,
   `examples/fframes-studio-intro`) still depend on the kit by git `rev`. After publishing
   they can switch to `fframes-kit = "0.1"` together, which also drops the need for
   `CARGO_NET_GIT_FETCH_WITH_CLI`.
6. **Seven formula snapshots** were not refreshed (see E2E).

## E2E

`just e2e cpu` passed (exit 0): 167 key frames compared with `gallery/snapshots`, 160 identical.
Seven formula scenes differ by 0.01 to 0.29 % (eulerjourney, formulaparts, mathtable, mathtex,
transformmatchingtex, writeformula, arcs), within the script's 0.5 % limit.

These snapshots predate the Typst backend. `mathtex_50p.png` was last committed on 2026-10-24
(TeX era) and `eulerjourney_end.png` on 2026-10-25 12:16, while Typst became the default at
`5b1f6ca` (2026-10-25 20:30). The diffs are Typst glyphs against TeX glyphs, not
nondeterminism. `just e2e cpu --update` does not refresh them: `scripts/e2e.mjs` copies only
frames above the 0.5 % limit. To make them exact, look at `target/e2e/cpu/gallery/<name>.png`
and copy it over `gallery/snapshots/<name>.png`. The gate passes without this.

