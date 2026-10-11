---
title: "Signed installers and update rollback"
status: pending
priority: P1
effort: "9–14 engineer-days"
---

# Stage 4: Signed installers and update rollback

## Context and outcome

Depends on stages 1–3. Read [plan](plan.md), [release contract](phase-01-start.md), SDK promotion/rollback code, native package helpers, process manager and app close hooks. Produce consumer installers and automatically activate verified updates only at an idle boundary.

## Verified starting point and architecture

The release workflow now retains an unsigned Linux x64 .deb package candidate alongside its internal phase-zero ZIP. The extracted package launches the product shell in a clean X11 session, but apt-managed install/upgrade/uninstall, AppImage, the macOS installer, signing, notarization and consumer associations remain unimplemented or unqualified. A per-user Windows Setup.exe candidate (`desktop/packaging/windows/fframes-studio.iss`, `package-windows-installer.py`, pinned Inno Setup 7.1.0) is now built, attested and retained by the release workflow, and signed with the package when signing is requested. It embeds the SDK until online acquisition is wired, and was installed, launched and uninstalled on the Windows Server 2022 development VM (2026-10-11). Signed-installer, clean-machine and Open project association evidence is still missing. By the maintainer's decision (2026-10-11), `v*` tag pushes, or a `preview_release` dispatch, publish these unsigned builds as a labelled preview GitHub release with winget/Scoop manifests and `install.ps1` (`prepare-github-release.py`). This is a deliberate exception to gated publication: the consumer release job, `release-policy.json` and the M7 eligibility rules are unchanged. SDK promotion and rollback now journal intent and recover interrupted directory swaps; paired app/SDK activation and an app updater remain unimplemented. No trusted feed exists. Existing close hooks own agent/build/preview/audio shutdown; extend their ownership instead of creating a competing process supervisor.

Flow: signed immutable release feed → target/version/compatibility validation → stage app and SDK → candidate probes → idle recheck and exclusive activation lock → durable app/SDK activation → restarted product health → commit or recover previous pair. Network retrieval reuses stage 2.

## File ownership

| Action | Files under repository root | Purpose |
|---|---|---|
| Modify | desktop/scripts/package-linux-deb.py, package-release.py, test-packaging.py; .github/workflows/desktop-release.yml | Native installers/signing, immutable qualified publication |
| Create | desktop/packaging/macos/Info.plist, entitlements.plist; desktop/packaging/windows/studio-installer.iss; desktop/packaging/linux/fframes-studio.desktop, control | Target integration (retain repo conventions at implementation) |
| Create | desktop/app/src/update-service.rs, update-view.rs; desktop/app/tests/update-recovery.rs | Idle update UI/coordination and fault tests |
| Create | desktop/crates/studio-sdk/src/update.rs; desktop/crates/studio-sdk/tests/update-activation.rs | Paired activation/recovery and metadata compatibility |
| Modify | desktop/crates/studio-sdk/src/install.rs, lib.rs; desktop/crates/studio-engine/src/app_paths.rs, store.rs | Durable serialized transactions and retained state |
| Modify | desktop/app/src/main.rs, lib.rs, studio_shell.rs, project_view.rs, teardown.rs | Native Open project events, startup/close/health wiring |
| Modify | desktop/packaging/sdk/notices/LICENSE.txt; docs/desktop/README.md, architecture.md | Actual notices and operational/update/retention routes |

Files marked Modify include release/M7 files created by the preceding stages; they are not claimed to exist in the current baseline. No deletions. Installer compiler/tool versions must be pinned through stage 1 provenance; Windows .iss is the proposed per-user installer route, validated against native behavior before advertising it.

## Requirements and implementation

1. Resolve protected CI signing identities, immutable release origin and metadata key ownership; document source/routes without credentials. Sign nested macOS binaries/runtime libraries and bundle appropriately, enable required hardened-runtime settings, notarize/staple and verify downloaded installation under Gatekeeper. Timestamp and verify Windows installer/application signatures. Produce and test Linux AppImage and deb with package/runtime dependencies and verified metadata.
2. Install per-user where supported, set accurate product version/identifier/icon and register Open project for studio.json/project opening. Parse OS open events safely, including startup and already-running instances; prefer a project directory or selected studio.json without editing portable source.
3. Wire automatic update policy. Idle means no provider task/permission/writer, build, playback, export/queue, SDK download/install, migration, mutation or unsaved prompt input. Retained drafts alone need not block indefinitely, but must be durably preserved. Recheck ownership/quiescence and acquire the activation lock immediately before transition; a new task defers activation.
4. Validate signed feed payload, trusted key, version/sequence, target, artifact size/hash, app↔SDK/protocol/export compatibility and minimum OS. Never execute download-location commands. Stage verified candidates without touching active installations. Signing failure/incompatible release remains a visible rejected update.
5. Implement a small platform activation helper/installer-owned operation for locked running binaries; derive its contract from native installer behavior. Keep its executable signed/trusted and authority limited to inventoried app locations. Coordinate deb/package-manager updates without overwriting files owned by another installer path.
6. Persist transaction intent and completed steps before filesystem moves. Serialize app/SDK activation; handle restart/power loss between every move. Logical atomicity comes from recovery intent, not a claim that two directories rename together. Retain previous app plus exact compatible SDK/resources until new startup, project reopen and tiny build/render probes pass.
7. Preserve prior app-local state through backup and backward-compatible migration; do not irreversibly migrate state before rollback is possible. The existing Store is SQLite and already has a consistent online backup helper including WAL state. Reuse/expose that mechanism to back up the database before any schema/data change and before opening it with the candidate app; back up journals/session files under the same recovery intent. Do not copy only the main SQLite file or let a candidate migrate the sole previous-state copy. A failed startup/health probe restores the verified previous pair and state without downgrading portable project content.
8. Test associations/Open project after update and rollback. Default uninstall removes application/integration only and retains projects, SDKs and state. An explicit remove-app-data action lists managed SDK/cache/state roots and never follows symlinks/reparse points into user folders or deletes source/media/exports. Reinstall must rediscover retained compatible state.

## Checklist and success criteria

- [ ] macOS notarized/stapled DMG, signed per-user Windows installer and Linux AppImage/deb launch the product shell from installed locations.
- [ ] Automatic idle updates preserve unsaved input, projects, drafts and active-job ownership.
- [ ] Unknown signature, tampering, replay/downgrade, wrong target or incompatible SDK rejects activation.
- [ ] Crash at each activation/state-migration boundary restores a usable app/SDK pair; WAL-containing database plus candidate schema migration and failed health restores state readable by the previous app.
- [ ] Native startup/build/render health failure rolls back with actionable status.
- [ ] Associations/recent/relink/import/Open project survive update and rollback.
- [ ] Uninstall retains all data by default; explicit managed-data removal never deletes user content.

## Validation

First new SDK update-activation and app update-recovery tests; then studio-sdk/studio-bootstrap suites and existing project_foundation/agent_workflow/provider_handoff tests. Inject rename/write/locked-file failures at every durable step and restart a real child between steps. Native installed tests verify signatures, quarantine/loader behavior, Windows locks and Linux package-manager ownership. Run scoped format/Clippy. Signing credentials unavailable means blocked evidence, never a bypass-signature consumer artifact.

## Risks, security and rollback

Updater executes downloaded code and mutates installed binaries. Metadata authenticity, least-scoped helper paths, installer ownership and recoverable activation are real trust boundaries. Automatically updating mid-task is explicitly disallowed by the accepted idle policy. Retain previous pair until health succeeds; keep last qualified release downloadable. Release-manifest key rotation requires an authenticated transition and an explicit rejection path for unknown keys; do not accept arbitrary keys supplied by the feed.

## Next step

Transfer candidate installers/artifact identities to stage 5; only its evidence authorizes consumer publication.
