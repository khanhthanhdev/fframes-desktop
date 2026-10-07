# Linux M3 agent-transaction evidence

Written by `desktop/scripts/qualify-m3-agent.py`. Development evidence only: it proves the workflow, engine, tools and cleanup behavior of this repository against a SCRIPTED ACP peer, a fake SDK tree / preview worker (and an installed SDK for the optional real-SDK checks). It does not qualify a provider, an adapter, an account, a writer process-group model, a physical device, IME input, Windows or macOS.

## Commands recorded here

- `acp_transport` [PASS] 46 tests passed in 2 targets: `cargo test --locked -p studio-agent-spike --test acp_v1 --test acp_v1_review`
- `engine_transactions` [PASS] 123 tests passed in 7 targets: `cargo test --locked -p studio-engine --test agent_task --test candidate_validation --test edit_transaction --test promotion_install --test review_history --test review_publication --test task_recovery`
- `app_workflow` [PASS] 64 tests passed in 1 targets: `cargo test --locked -p fframes-studio --test agent_workflow`
- `app_tools_validation` [PASS] 102 tests passed in 4 targets: `cargo test --locked -p fframes-studio --test agent_tools --test build_sharing --test candidate_runner --test promotion_handoff`
- `app_ui_workflow` [PASS] 11 tests passed in 1 targets: `cargo test --locked -p fframes-studio --test agent_workflow_ui`
- `dev_evidence` [PASS] 1 tests passed in 1 targets: `cargo test --locked -p fframes-studio --test m3_development_evidence -- --nocapture`
- `real_sdk_workflow` [PASS] 2 tests passed in 2 targets: `cargo test --locked -p fframes-studio --test agent_workflow --test real_sdk_promotion -- --ignored real_sdk`
- `native_x11_shell` [PASS] 1 tests passed in 1 targets: `cargo test --locked -p fframes-studio --test x11_shell -- --ignored --nocapture`

## Files

- `environment.json`: OS, kernel, toolchain, git, SDK and adapter identity (environment variable names only, never values).
- `harness-run.json`: harness arguments, endpoint guard, every owned PID, per-check exit status and leftovers.
- `logs/*.log`: redacted, size-bounded cargo output per check.
- `resource-bounds.json`, `compiler-count.json`, `cycles.json`, `cli-mcp-parity.json`, `publication-primitive.json`, `summary.json`: measurements written by `app/tests/m3_development_evidence.rs` (full profile).

## Limits

- No ACP adapter or credentials exist on the development machine unless `--adapter` was supplied; every other authentic gate is `not_run` in `../../m3-results.json` with its prerequisite.
- Event-queue bound: the driver exposes no queue-depth gauge; the measurement is the absence of an overflow failure while the scripted event count exceeds the 256-event queue (the peer paces its tool-card flood: an unpaced burst of non-coalescing events deliberately seals the producer).
- Playback/scrub concurrency, GPU layout cost, physical audio and IME were not measured.
- Re-run: `python3 scripts/qualify-m3-agent.py --replace` from `desktop/`, then `python3 scripts/validate-qualification.py`.
