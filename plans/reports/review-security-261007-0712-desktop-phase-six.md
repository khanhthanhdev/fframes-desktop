# M6 plan security and contract review

Date: 2026-10-07. Scope: plan and all three phase documents in `plans/261007-0712-desktop-phase-six-provider-qualification-handoff/`, architecture ownership rules and planning baseline. Read-only source verification; no implementation tests, lint or build were run.

## Findings

Zero Critical, High or Medium findings. The plan explicitly addresses the concrete ownership, credential, restoration and evidence failure modes exposed by the current source. This is plan acceptance, not qualification of future code or authentic providers.

## Verified contracts

- **Launch and evidence:** `desktop/app/src/conversation_panel/qualification.rs:148` hashes the executable and absolute argument files; `:236` derives ownership from validated evidence, and `:258` fails closed through verification. Phase 1, Implementation steps 1 and 5, extends this to exact runtime/dependency closure and current platform rather than assuming the existing executable-only identity proves an entire package launch. Phase 3, steps 5 and 8, requires disposable authentic containment qualification and rejects escaped/unproven writers.
- **Credential handling:** `desktop/crates/studio-agent-spike/src/discovery.rs:185` forwards only named authentication variables and collects their values as redaction secrets; `:153` also redacts neutral authentication variable names. `desktop/app/src/conversation_panel/host.rs:284` provides the existing owner-only atomic configuration save pattern. Phase 1 requires credential-free app-local profiles; phase 2 requires owner-only atomic session storage and provider-managed credentials; phase 3 excludes credentials, private prompts and opaque IDs from evidence.
- **Writer shutdown and retention:** `desktop/crates/studio-engine/src/agent_task.rs:1128` converts abandoned active ownership to unsafe, and `:1182` refuses preparation under a live/unknown writer while archiving retained drafts. Phase 2 explicitly prevents blind reuse of that refresh route and requires verified scope teardown, immutable variant capture, fenced user choice and cause-specific draft preparation. This meets `docs/desktop/architecture.md:181` ownership rules without introducing a competing owner.
- **Restoration:** `desktop/crates/studio-agent-spike/src/driver/runtime.rs:1625` chooses negotiated resume, then load, otherwise rejects restoration; `:1706` sanitizes the public session ID. Phase 2 preserves native identity independently of transcript rows, binds launch/project/draft/capabilities, retains identical cwd, rejects unknown versions and requires explicit user continuation with context refresh. It does not auto-resend interrupted work or auto-publish recovered candidates.
- **Capability/event isolation:** `desktop/app/src/agent_workflow/tools.rs:33` binds liveness to the active task; `:147` revokes grants and `:152` unregisters ended tasks. `desktop/app/src/agent_workflow/actor.rs:2151` checks task/writer correlation on replies. Phase 2 extends this explicitly across provider/session/task/writer epochs and replay boundaries; phase 3 tests retired grants, stale replies, duplicate restores and queue ownership.
- **Evidence integrity:** `desktop/scripts/validate-qualification.py:283` confines hashed evidence to its directory; `:325` scans secrets/private paths; `:500` binds authentic records to an exact gate and launch. Phases 1 and 3 extend these existing strict contracts, keep fixtures distinct, forbid test writer injection in authentic runs and refuse recommendations until mandatory comparable passes exist.

## Implementation contract to preserve

`desktop/crates/studio-agent-spike/src/driver/runtime.rs:2126` already exposes an exact wire ID only when redaction would leave it unchanged. Phase 2's “preserve opaque IDs exactly” must preserve that safeguard: safe IDs round trip exactly; IDs containing detected credential material make persistence unavailable and use the already specified visible fresh-session fallback. The plan permits minimal API changes where necessary and requires provider credentials to remain in provider storage/environment, so this is a verification detail rather than an uncovered plan finding.

## Remaining uncertainty

Authentic credential routes, package closure, subprocess containment, restoration and tool behavior remain execution evidence requirements. The plan correctly records unavailable prerequisites as blocked/not_run and does not infer safety from fixture or Linux development passes.

Status: DONE
Summary: Reviewed all three M6 stages against security ownership and current source contracts; no concrete Critical/High/Medium plan findings.
Concerns/Blockers: None. Preserve the existing secret-bearing session ID exclusion during implementation.
