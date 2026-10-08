---
title: "Guided setup and verified SDK acquisition"
status: in-progress
priority: P1
effort: "7–10 engineer-days"
---

# Stage 2: Guided setup and verified SDK acquisition

## Context and outcome

Depends on stage 1. Read [plan](plan.md), [release contracts](phase-01-start.md), SDK manifest/Doctor/install/environment code, setup_view and M6 provider profiles. Deliver terminal-free setup for supported defaults with honest provider/prerequisite status.

## Verified starting point and architecture

SdkInstaller now records receipts, checks disk capacity, recovers interrupted SDK promotion, reuses a compatible active install without archives, and safely stages local artifacts with real offline compile/render probes. Ed25519 release-manifest verification and a bounded resumable downloader are implemented and have local HTTP tests for resume and ignored Range. The app exposes explicit offline-bundle import and receipt-first reuse, but it does not yet connect a pinned signed release feed/downloader to guided online setup or provider authentication. Native setup remains a separate unqualified gate.

Flow: trusted release metadata → target/compatibility check → existing receipt reuse or download/offline import → verified staging → Doctor/build/render → durable activation → ready. Network/download/install work runs on background owners, with immutable snapshots and cancellation; no blocking UI work.

## File ownership

| Action | Files under repository root | Purpose |
|---|---|---|
| Modify | desktop/crates/studio-sdk/src/install.rs, manifest.rs, doctor.rs, environment.rs, lib.rs; desktop/crates/studio-engine/src/app_paths.rs | Compatible receipt reuse, staged installation, owned storage |
| Create | desktop/crates/studio-sdk/src/download.rs; desktop/crates/studio-sdk/tests/sdk-download.rs | Bounded resumable acquisition and failure tests |
| Modify | desktop/app/src/setup_view.rs, project_view.rs, studio_shell.rs, lib.rs | Product onboarding/progress, offline chooser and recovery |
| Modify | desktop/app/src/conversation_panel/provider-profiles.rs, qualification.rs; desktop/crates/studio-agent-spike/src/discovery.rs | Managed runtime/detection/auth setup using M6 evidence |
| Create | desktop/app/tests/guided-setup.rs | Setup transitions and archive-free reopen |
| Modify | docs/desktop/README.md, architecture.md | Supported prerequisite/auth/offline routes |

No deletions. Shared ownership transfers from stage 1 and later to stage 4.

## Requirements and implementation

1. Verify signed metadata and compatibility before accepting URLs. Reuse a valid installed receipt before touching original archives; SDK identity/digest/target and required capabilities must agree. Never use developer environment variables as consumer requirements.
2. Show download size, expanded/install/build disk budget, actual bytes/progress, retry/cancel and prerequisite details. Check available space before acquisition and extraction, and handle mid-write exhaustion.
3. Store bounded partial files under app-owned cache, keyed by signed artifact identity. Resume with validated server range/content identity; if ignored or changed, discard/restart safely. Restrict redirect/origin policy; verify final size/hash before extraction. Avoid unsafe paths/links and bound archive entries/expanded bytes.
4. Serialize SDK installs. Persist install intent and recover staging/active/previous transitions after crashes; never report ready until receipt and real probes agree. Preserve known-good SDK and retain leases until jobs finish.
5. Add offline bundle file/directory selection and active receipt reuse with no archive present. Preserve existing compile/render probes and quarantine corrupt candidates.
6. Reuse M6 provider discovery/picker. Detect supported adapter/runtime versions, show authentication requirements, offer provider-supported GUI/browser authentication or managed runtime provisioning, and keep credentials in provider/OS stores. Never silently download an unpinned executable or feed credentials into portable projects/logs.
7. For a provider or OS prerequisite requiring terminal setup, either implement a supported native guided route or keep it outside advertised default workflows. Installed-but-unqualified remains experimental; initialization is not qualification.
8. Close/cancel reaps only owned verification/runtime processes off the UI thread, retains recoverable partial acquisition, and rejects late completion from a closed project/setup generation.

## Checklist and success criteria

- [x] A receipt-backed compatible SDK is reused without original archives (`receipt_backed_active_sdk_is_reused_without_original_archives`).
- [ ] Download resume/restart handles ignored Range, changed content, corruption and network failure.
- [ ] Disk exhaustion, interrupted promotion and cancellation leave a working SDK selectable.
- [ ] Auth/prerequisite guidance completes supported defaults without a terminal.
- [ ] Setup UI stays responsive and never labels an unverified install/provider ready.

## Validation

First run `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-sdk` and the new guided-setup test. Use a local HTTP fault server only for deterministic transport failures; record real native clean-user acquisition separately. Then run existing project_foundation and provider_profiles tests, plus real managed_worker/portable_managed_worker tests with SDK_BUNDLE set to the candidate bundle and `-- --ignored`.

## Risk, security and rollback

Downloaded SDKs/adapters execute native code, so metadata trust, pinned runtime provenance, extraction limits and credential redaction are required. If a provider cannot authenticate through a supported GUI route, block its default-onboarding claim without weakening M6 qualification. Failed installation leaves the prior verified SDK intact. SDK promotion/rollback presently has crash windows and ignored rename failures; add durable recovery tests here and extend them to app/SDK pairing in stage 4.

## Next step

Transfer compatible SDK/build ownership to export stage 3.
