# Finish Phase Zero implementation and available evidence

Outcome: require real candidate build/render before SDK promotion; bound worker requests and preserve monotonic worker identities; exercise 1,000 real-worker presentations with measured process, queue, submission/release and UI heartbeat results. Run all available native checks and keep external qualification gaps explicit.

Constraints: configurable ACP adapter, authenticated qualification deferred; no invented platform or physical-display evidence; preserve existing setup, preview and draft behavior. Signing and production editor work remain out of scope.

- [x] Candidate template and worker compile/render checks run offline inside staging before promotion; failure preserves active SDK.
- [x] Worker request write/read/frame operations have a total deadline and owned-process cleanup; worker and agent generations share a monotonic allocator.
- [x] Stress run performs real repeated worker seeks, confirms 1,000 painted images and records bounded queue/image counters, process memory and UI heartbeat gaps.
- [x] Execute available Linux checks; expose a reproducible native evidence artifact. Windows/macOS and authenticated ACP stay unmet without native runs.
- [x] ACP clarification replies reuse the same session/draft; cancellation and authoritative completion remain fenced before candidate rebuild/publication.
- [x] Focused tests, Clippy, formatting, review, docs and ledger reconciliation.

Result: requested implementation and available Linux evidence complete. Overall qualification remains PENDING for physical GPU/IME, Windows/macOS and authenticated ACP. See [completion report](../reports/implementation-261001-2213-phase-zero-completion.md), [native procedure](../../docs/desktop/phase-zero-feasibility.md) and [ledger](../../desktop/qualification/m0-results.json).

## Windows development run (2026-10-10)

On 2026-10-10 a Windows Server 2022 x64 VM (virtual RDP display, no GPU) ran the Windows development checks: the full desktop workspace suite (79 test targets) passes on MSVC, the Windows SDK assembles with its offline double build, and the packaged app installs the managed SDK into a fresh home, compiles a worker, presents 1,000 frames and accepts native SendInput typing and preview selection with no leftover processes. Results and evidence are in the [M0 ledger](../../desktop/qualification/m0-results.json) under `additional_platforms`. Windows qualification itself remains PENDING.
