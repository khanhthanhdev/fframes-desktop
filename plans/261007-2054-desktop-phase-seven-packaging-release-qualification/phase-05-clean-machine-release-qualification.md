---
title: "Clean-machine release qualification"
status: pending
priority: P1
effort: "6–10 engineer-days"
---

# Stage 5: Clean-machine release qualification

## Context and outcome

Depends on stages 1–4 and authentic M6 provider evidence for each advertised connector/target. Read [plan](plan.md), M0/M2–M6 ledgers and qualification scripts, native release policy and final installers. Qualify the exact installed bytes; release each passing platform independently.

## Starting evidence and architecture

Linux X11/software rendering and virtual audio establish development behavior. M6's authentic runner only inventories PATH, launches no adapters and does not update the ledger. No full authentic default connector is currently qualified. Build CI, mock-agent integration, initialization or screenshots alone cannot satisfy this stage.

M7 evidence references the exact app installer/SDK/feed digests, source commit/worktree digest, target/baseline/device, native provider distribution/auth route/capabilities and scenario results. Separate automated, authentic, physical and installation evidence. Reuse M6 provider gates and ranking; gather real supervised workflow evidence rather than renaming readiness inventory.

## File ownership

| Action | Files under repository root | Purpose |
|---|---|---|
| Create | desktop/scripts/qualify-m7-release.py | Installed authentic/physical/manual scenario collection and checks |
| Modify | desktop/scripts/test-qualification-m7.py, validate-qualification.py | Negative evidence/provenance and release eligibility checks |
| Modify | desktop/qualification/m7-results.json, m7-results.schema.json | Exact target/provider/installer/export/update evidence |
| Create | desktop/qualification/evidence/m7-<target>-<timestamp>/ | Sanitized logs/artifacts/hash-bound records |
| Modify as evidence warrants | desktop/qualification/m6-results.json; desktop/scripts/qualify-m6-providers.py | Authentic M6 evidence ingestion without replacing development status |
| Modify | .github/workflows/desktop-release.yml | Publish only eligible target artifacts |
| Modify | docs/desktop/README.md, architecture.md, implementation-plan.md, phase-zero-feasibility.md | Actual support, setup/update/rollback and remaining blockers |
| Update execution records | This plan's checklists and validation report | Record gate evidence, not evergreen product authority |

Files marked Modify include release/M7 files created by the preceding stages; they are not claimed to exist in the current baseline. No deletions. Qualification uses genuine accounts and owned test users/projects; never include credentials or private prompts in evidence.

## Implementation and scenario matrix

1. Provide ledger ingestion with explicit operator attestations and hash-bound outputs for authentic/device tests. PATH inventory remains readiness only. Establish sterile users/machines with no checkout, preinstalled developer tools, global Cargo registry cache, undeclared PATH runtimes or initial SDK.
2. Install the signed candidate through the native installer and launch from the installed route, including OS association/Open project. Record unavoidable guided prerequisites and verify product setup handles them; do not preinstall them silently.
3. Authenticate supported connectors through the native guided route, download SDK, generate a real video, apply a style, select scene/range and annotated title, prompt an edit, Apply/Undo and controlled handoff when both connectors are qualified. Preview/scrub/play with physical input/display/audio and export through native controls.
4. Decode exported MP4, verify streams/duration/representative frames/audio against the pinned backend, record revision/media/style/SDK identity and codec tolerances. Exercise overlap/shared-helper/imported-project fallback and visible unsupported shader handling.
5. Close/reopen and relocate project with assets; remove original SDK archives and network access, then reopen/rebuild/export using the installed SDK. Imported custom dependencies may need explicit advanced setup and must not silently redefine the advertised default path.
6. Exercise update N→N+1 and rollback with real signed artifacts and app/SDK combinations: active-work deferral, idle race, corrupt feed/download, interrupted activation, startup/probe failure and offline recovery. Test installer repair, reinstall, default uninstall retention and explicit managed-data removal.
7. Measure a 10-minute playback/scrub/task/export run: bounded queue/cache/snapshot/disk accounting, UI input response, physical audio timing, compiler and child process counts, and project close/shutdown cleanup. Compare to recorded reference-machine thresholds; no universal performance claim from an unloaded fixture.
8. Generate release eligibility for each exact target artifact. Require every applicable authentic/device/install/export/update gate passed with evidence; absent accounts/signatures/hardware means blocked/not_run. Publish only qualifying targets and leave other target work pending, preserving full scope and previous releases.

| Area | Positive path | Required failure/recovery evidence |
|---|---|---|
| Installer/runtime | Installed launch, fonts/helpers/codecs, native Open project | Missing dependency, quarantine/signature error, wrong OS/target |
| SDK/setup | Native auth/download/build; archive-free offline reuse | Disk full, interrupted/resumed download, bad hash, missing linker |
| Edit/selection | Authentic selected edit, Apply/Undo/reopen; supported handoff | Compiler repair, Stop/crash, stale selection, external edits |
| Physical preview | IME/input, scrub/play, matching audio/backend | Device disconnect/reconnect; overload/end/seek and shader gaps |
| Export | Accepted snapshot MP4 with video/audio, bounded FIFO/progress | Concurrent edit/media change; cancel/crash; destination conflict/full disk |
| Update | Automatic idle activation; reopen with compatible SDK | Replay/tampering, busy-to-idle race, each interrupted move, failed health |
| Retention | Update/rollback/reinstall preserve user project/state | Default uninstall and explicit data removal preserve external content |

## Checklist and success criteria

- [ ] Each advertised target passes the full installed-app journey with authentic qualified providers.
- [ ] Physical display/input/IME/audio gates are distinct from virtual-device development results.
- [ ] Export/output integrity, cancellation and revision identity pass on native filesystems.
- [ ] Signed automatic update/rollback and retention pass with exact candidate bytes.
- [ ] Validator rejects tampered/stale/missing/mocked evidence and wrong run/tag/artifact provenance.
- [ ] Publication includes only qualified targets and authentic provider claims.
- [ ] Docs describe supported routes, limitations and recoverable blockers from observed evidence.

## Validation and release gate

Run first `python3 desktop/scripts/test-qualification-m7.py`, existing M4–M6 evidence-contract tests and `python3 desktop/scripts/validate-qualification.py desktop/qualification/m7-results.json`. Format/Clippy/full relevant desktop and root protocol/runtime tests run on native CI. Execute new qualifier per its implemented live help; do not invent CLI flags in advance. Run the installed matrix above on real machines, with hashes and named devices/accounts/routes.

Before publishing, compare final artifacts and feed to qualified digests and signing checks; requalification is required when relevant bytes/configuration change. A functional SDK/renderer test from the checkout does not substitute for installer evidence.

## Risks and rollback

Missing provider accounts, physical devices or signing infrastructure can block a target without blocking independent passing targets. Do not downgrade standards to finish the milestone. Retain previous qualified releases and SDKs; immediately withdraw a failing update feed entry and use the verified local rollback route. Evidence must be sanitized and reproducible enough to explain every advertised claim.

## Completion

M7 is complete only when all planned target work and required gates are completed or the user explicitly changes scope. A partial platform release is a valid intermediate delivery, not completion of the entire three-platform plan.
