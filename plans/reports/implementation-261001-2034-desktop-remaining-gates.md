# Remaining desktop feasibility implementation

Date: 2026-10-01. Implementation delivered; native qualification pending.

Superseded by [the completion report](implementation-261001-2213-phase-zero-completion.md). The Cargo-offline runs recorded below did not block native FFmpeg build-script downloads. A later network-isolated clean-account run exposed that dependency configuration and rejected SDK promotion. The corrected SDK and subsequent native evidence are recorded in the completion report and current ledger; this earlier report is historical evidence only.

Implemented frame-completion-paced stress presentation, actual preview click mapping and source snippets, configurable ACP v1 sessions with streamed updates/permissions/cancellation, isolated draft edit/build/frame verification, native SDK assembly and packaging, CI matrix and evidence validation. The adapter remains configurable; authenticated qualification is deferred by user choice.

Presentation counts only images successfully painted and acknowledged through the subsequent GPUI frame callback. Exactly one conversion/presentation is outstanding; duplicate and old-run callbacks cannot advance the 1,000 counter. This measures GPUI submission rather than a physical-display fence. Source metadata becomes selectable only with the painted image. Selection checks displayed revision/generation, project revision, hash, root containment, unique identity/markers and UTF-8 span, then shows the snippet.

ACP uses actual newline-delimited JSON-RPC subprocess I/O. Tests cover streaming, permission responses, authoritative completion, isolated source edits, oversized/truncated messages, split-chunk secret redaction and cancellation of a nonreading peer. The peer is protocol test infrastructure, not provider qualification. Cancellation is rechecked before publishing a candidate; each run owns a process manager and callback generation. Real worker builds resolve the initial lockfile and compare source revisions before/after compilation.

The Unix native FFmpeg builder verifies official n9.0 commit `d32b387f2b0a484599d4587d651891f0c63c4238`. Assembly refuses incorrect versions, vendors the expanded worker graph and builds/renders twice offline with empty Cargo home and different target directories before deriving archive sizes/hashes. The embedded Linux default manifest now uses the actual manifest instead of duplicated placeholder artifacts. Filesystem promotion/rollback tests no longer pretend a shell script is a compiler; the real installed-bundle integration test establishes buildability.

## Verification

- Desktop app, bootstrap, SDK and ACP: 40 passing tests. The two artifact-dependent tests are excluded from routine unit runs and were separately executed successfully.
- Actual worker subprocess: hello, 1920×1080 frame/payload, source snippet, duplicate identity rejection, forced termination and restart passed.
- Actual SDK installation: all three archives extracted transactionally; managed worker built offline, launched, rendered and returned metadata; owned process cleanup passed.
- Root protocol/runtime: 10 passing tests.
- Annotated fixture: source-anchor test passed; CLI frame zero produced a valid PNG.
- Packaging: four Python regression tests passed; qualification ledger validator and CI YAML parsing passed.
- Touched Rust packages: Clippy with warnings denied passed. Root, desktop and fixture formatting checks passed.
- Reviewer rechecked ACP cancellation/publication, worker revision, SDK assembly, packaging and docs: no remaining blocking findings.

The first worker integration attempt was blocked by sandbox socket binding; its approved native rerun passed. Vendoring needed approved network access; the subsequent offline builds passed. Replacing placeholder default artifacts exposed two old installer tests that assumed only two artifacts; the lifecycle test now exercises real filesystem promotion/rollback, and the negative candidate includes all three declared artifacts. A YAML scalar containing a colon was corrected to a block scalar and parsed successfully.

## Local artifacts

The SDK bundle is `/tmp/studio-sdk-native-verified` with compiler, FFmpeg and framework/vendor archives of 208,280,949, 82,659,659 and 44,401,157 bytes respectively. The native application package includes that SDK, notices, worker source, dependency inventory, checksums and a pending qualification ledger. Temporary local artifacts are not committed or published.

Native compositor/IME and actual visible 1,000-presentation resource measurements, sterile clean-account/offline setup, Windows/macOS runner results and authenticated ACP edit/cancel remain pending. CI configuration exists but has not run on those remote runners. See [native procedure](../../docs/desktop/phase-zero-feasibility.md) and [results ledger](../../desktop/qualification/m0-results.json).
