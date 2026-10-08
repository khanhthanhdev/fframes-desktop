# M7 baseline and planning validation

Date: 2026-10-07. This report began as the planning baseline for M7; the execution update below records the first implementation pass. No consumer release or platform qualification has occurred.

## Desired outcome and boundaries

Deliver the full existing installer/setup/export/update/consumer qualification scope. Reuse the Rust/GPUI stack, managed native SDK, immutable worker/build architecture and controlled agent workflow. No framework change, cloud service or provider driver is added. Acceptance is an installed authentic create/edit/preview/export/reopen journey, plus safe updates and data retention, on each advertised platform.

## Verified baseline

| Milestone | Implemented/evidence | Remaining boundary |
|---|---|---|
| M0 | Managed native build, worker and explicit selection anchor on Linux | Physical GPUI/display/IME and authentic tasks |
| M1 | Create/open/import/assets/relocation/checkpoint/recovery; real killed-child and SDK tests | No separate m1-results ledger; not consumer installation evidence |
| M2 | Immutable CPU preview, timeline, CPAL output-clock scheduling; pixels/PCM/resource tests | Physical audio timing/disconnect and other platforms |
| M3 | Actor/ACP panel, tools, candidate validation/promotion/Undo and development gates | Authentic Claude handshake is not prompt/edit/repair/tool/containment qualification |
| M4 | Presets/styles and scoped edits; real managed SDK/native development tests | Authentic, physical and Windows/macOS evidence |
| M5 | IDs/anchors/geometry/retrieval and element/rectangle workflow | Authentic, physical and Windows/macOS evidence |
| M6 | Profiles/picker, stopped-writer handoff and durable restoration in dirty workspace | Every authentic provider scenario remains unrun; ranking insufficient |

M6 latest development record: [run](../../desktop/qualification/evidence/m6-development-20261007T123057Z/run.json). Seven checks pass, authentic_status is not_run, ledger_updated is false, and scout verified every referenced log hash. The [ledger](../../desktop/qualification/m6-results.json) still has acceptance.development not_run; this is separate from passing runner reports. All four providers have missing executable observations and unresolved versions/capabilities; no recommendations.

Historical M6 plan status is stale: phase 2 checks are done while index says Pending; phase 3 picker checks exist while authentic gates remain unrun. Preserve historical execution records; M7 authority is current source and qualified evidence. Roadmap M3/M6 implementation paragraphs are corrected narrowly to avoid planning against nonexistent gaps.

## Source contracts and actual gaps

- SDK manifest/install/Doctor/environment code exists under desktop/crates/studio-sdk. Active/previous rollback is real but not crash-safe paired app/SDK activation. compatible_app_range exists without enforcement.
- BuildService freezes SDK/environment identity, deduplicates exact requests and leases MaterializedBuild. BuildProfile only Debug; CargoCompiler rejects non-default features/backend/options. Export must implement supported compilation and retained-source requests.
- Root fframes-studio-runtime and fframes-studio-protocol own worker contracts; no native export operation exists. fframes/src/renderer/cli.rs and encoder.rs provide render/codec/JSON progress foundations.
- package-phase-zero.py verifies helpers and SDK bytes, copies Windows DLLs and emits ZIP/.app/launchers. Launch scripts use spike-ui; macOS direct bundle and default binary use studio. Consumer packaging must make entry routes consistent.
- desktop-release.yml can publish phase-zero assets on core v* tags and reuse existing runs; asset replacement uses clobber. M7 requires consumer eligibility, provenance and immutable release bytes.
- SDK acquisition is local-only; setup checks archives before existing receipt reuse. Network resume/progress/disk checks, authenticated metadata, consumer update and signed installers are new work.
- Current notices are broad placeholders. Actual static/shared FFmpeg inputs differ across OSes, and Windows acquisition is moving latest. Native MP4 encoder and distribution inventory must be proven, not assumed.

## Validation questions and answers

Three questions were asked using the asynchronous input tool; all were answered.

1. “For M7, should each platform ship as it passes qualification, or should the first release wait for macOS Apple Silicon, Windows x64 and Linux x64 together?”
   Options: Ship each qualified platform (Recommended) | Wait for all three platforms.
   Answer: “Ship each qualified platform (Recommended)”.
   Impact: stages 1 and 5 retain all targets but permit independently qualified releases.
2. “Should M7 updates ask the user before installing, or install automatically while Studio is idle?”
   Options: Ask before installing (Recommended) | Install automatically when idle.
   Answer: “Install automatically when idle”.
   Impact: stage 4 must implement automatic idle activation and recheck races, preserving active work and unsaved input.
3. “Should uninstall retain projects, downloaded SDKs and app state by default, with an explicit option to remove app-managed data?”
   Options: Retain data by default (Recommended) | Retain projects; remove SDK and app state.
   Answer: “Retain data by default (Recommended)”.
   Impact: stage 4 installer removal and stage 5 retention tests preserve user data.

## External verification

Platform/distribution requirements were checked against primary sources while planning. Apple documents Developer ID notarization and notarytool; Microsoft documents package signing/timestamp use; FFmpeg documents configuration-dependent distribution considerations. Execution must recheck the chosen installer and actual codec build:
[Apple](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow), [Microsoft](https://learn.microsoft.com/en-us/windows/win32/appxpkg/how-to-sign-a-package-using-signtool), [FFmpeg](https://www.ffmpeg.org/legal.html).
These checks do not certify the project's packages or decide unresolved distribution policy.

## Tooling and tracking

CLI created the plan and five phase files. Creation warned that the global index database was outside the writable workspace; files succeeded. The UTC-generated directory was renamed to the session's required 261007-2054 name. Live phase-update help confirms titles/content/status are file-owned. No runtime task surface is available, so checklist state is durable authority.

Independent scouts and planner advice informed the draft. Formal assumption/failure/security review and final format/link/consistency checks are recorded below. Runtime native tests are intentionally future stage gates, not planning results.

## Red-team adjudication and verification

Three independent reviewers checked all six plan files and 58 material source/contract claims in total, with overlapping checks: security 19, failure 18 and assumptions 21. They traced compile leases/lifetimes, source/checkpoint authority, setup/install/update flow, schema consumers, worker templates and release provenance. Full-tier roles were covered across source fact checking, flow tracing, lifetime/scope checks and contract verification. This count is reviewer checks, not 58 unique facts.

| Finding | Severity | Disposition and source |
|---|---|---|
| Saved checkpoint can lag an applied task revision | High | Corrected stage 3: export selects explicit validated/published source, tests Apply without Save and Undo. state.rs:241,321; edit_transaction.rs:281,299 distinguish saved checkpoint from published revision. |
| Update rollback treated existing SQLite as hypothetical | High (highest reviewer rating) | Corrected stage 4: consistent online database backup including WAL before candidate migration, plus coordinated journal/session backup and old-app restore tests. store.rs:22,39,163–175. |
| Ambiguous lifecycle inventory and invented controller type | Low | Corrected stage 3 to desktop/crates/studio-project/src/lifecycle.rs and Controller::export_checkpoint. lifecycle.rs:277–281; controller.rs:61,450. |

These corrections restore source accuracy without changing any user decision. No unresolved Critical/High/Medium findings remain. No evidence-free security checklist or additional product feature was added.

## Whole-plan consistency sweep

Reread plan.md and every phase file after review corrections. Checked export revision authority versus the separate M1 saved checkpoint, existing SQLite backup/rollback, full lifecycle path/Controller symbol, automatic idle policy, independent platform delivery, retained-data uninstall, authentic evidence boundaries and pending stage status. Proposed files are explicitly Create; later Modify entries may reference artifacts introduced in earlier stages. Shared ownership transfers sequentially; no dependency cycles.

The plan parser identifies five pending phases and 32 unchecked phase tasks. CLI format validation passes. All 38 local Markdown links in the plan/report/roadmap resolve. Existing Modify paths were checked against source; stage-1-created release/M7 files are intentional later-stage inputs, not falsely claimed baseline files. Roadmap has 268 lines, below the configured 800-line ceiling. Diff whitespace check passes; no product code/tests were changed by this planning session, so runtime gates remain future work.

## Handoff and remaining prerequisites

The plan is ready for implementation using `plans/261007-2054-desktop-phase-seven-packaging-release-qualification/plan.md` as the cook path. Scaffolding/index warning is recorded above; journal creation succeeded under plans/journals. Native signing identities, trusted release origin/keys, codec distribution decisions, actual provider accounts and physical/native test machines remain execution prerequisites with explicit blocker handling, not unanswered product-policy questions. No consumer-ready platform/provider claim is made.

## Execution update: stage 1 in progress

The opening repository identity was commit `63052c5d70359cd105bb752fdd74088f38c7d622`; the prior executor recorded the opening dirty-worktree digest `a97b789abde5fe9ff34706b1490489c7051fb3bf84d83268475e5bb30e2ac4c4` across 48 changed/untracked paths. This preserves the M6/plan baseline identity and is not the digest of the later M7 implementation tree.

Stage 1 now adds app/SDK compatibility-range validation at SDK setup/build boundaries, a typed Ed25519 release-manifest v1 verifier, release-policy and M7 qualification schemas/ledger, hash-bound evidence checks, and fail-closed consumer publication guards. Windows FFmpeg CI inputs now use the immutable `autobuild-2026-10-07-13-07` asset and its SHA-256 rather than the moving `latest` URL. Existing SDK manifest serialization and legacy reads remain covered by tests. The M7 ledger intentionally retains `worktree_digest: null` and all candidates blocked: no M7-qualified full package exists.

Verification so far: `studio-sdk` tests (21), Studio provider handoff/profile tests (38), Studio workflow UI tests (13), Studio SDK and app Clippy, root runtime/protocol tests (12 each), the 47-test packaging suite, the 8-test M7 contract suite, M6 qualification negative/positive tests and ledger validation all pass. The four relevant workflow YAML files and M7/release-policy JSON parse. A full Linux phase-zero ZIP with the M5 SDK bundle was assembled at `/tmp/fframes-studio-m7-linux-m5-sdk.zip`; its 21-entry inventory hashes correctly, executables run help paths from the extracted layout, `ldd` found no unresolved shared libraries, and the ignored managed-worker and portable-worker package tests passed using that bundle. These are SDK/helper and worker-layout checks, not consumer installer qualification: no real GUI launch, clean-user install, signing or cross-platform run was included.

Codec verification against the packaged SDK archive now succeeds for a one-second MP4 containing HEVC video and AAC audio; full decoding with FFmpeg 6.1.1 returned success. The FFmpeg archive SHA-256 is `5afe219f662f1ed816fae9ab5571663dadb42836f85d90a3174d67e51ec4dca8`; the render output and probe record are retained under `.amp/in/artifacts/`. However, `ldd` on the HEVC-enabled renderer shows `libx265.so.199` resolving from the host, not from the SDK archive. The SDK therefore does not yet qualify as self-contained for HEVC; a clean-user runtime and GPL/source-distribution review are still blockers. The separate local FFmpeg build with a different `libavcodec.a` hash was not treated as package evidence. Stage-1 installed-location/font/runtime checks and full codec/source review remain open; stages 2–5 have not started. Trusted release keys and consumer publication remain disabled. No release, commit, or push was performed.
