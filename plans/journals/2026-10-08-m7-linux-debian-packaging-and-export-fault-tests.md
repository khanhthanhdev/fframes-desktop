---
title: M7 Linux Debian Packaging and Export Fault Tests
date: 2026-10-08
summary: Built an unsigned Linux x64 .deb candidate path and extended native export smoke coverage without claiming M7 qualification.
---

# M7 Linux Debian Packaging and Export Fault Tests

## Work

- Added `desktop/scripts/package-linux-deb.py` to create an unsigned Linux x64 Debian package from phase-zero app binaries. It includes the product-shell launcher, desktop entry, sibling helpers and notices; derives direct shared-library dependencies with `dpkg-shlibdeps`; keeps the SDK separate; rejects symlink inputs; and publishes the package without clobbering an existing path.
- Wired Linux CI to retain the Debian candidate separately from the existing ZIP. Consumer publication remains fail-closed and does not include this unsigned candidate.
- Extended the ignored Linux real-worker export smoke to cancel during rendering and prove a destination created during rendering is preserved by no-clobber publication.
- Updated M7 execution notes and the Linux ledger blockers; all targets remain ineligible.

## Verification

- `python3 desktop/scripts/test-packaging.py`: 48 tests passed.
- Focused Debian packaging test passed after the final packaging hardening.
- Built an actual 0.1.0 amd64 Debian candidate; `dpkg-deb` reports dependencies derived from this Linux build and the package contains no SDK.
- Extracted-package product shell launched under Xvfb with a clean HOME and no inherited developer variables; screenshot inspected. Direct ELF dependencies resolve on this host.
- Linux ignored native export/decode/cancellation/destination-race smoke passed with the local SDK bundle.
- `cargo clippy --offline --manifest-path desktop/Cargo.toml -p fframes-studio --all-targets -- -D warnings`, desktop `cargo fmt --check`, M7 qualification tests/validator, workflow YAML parse, and `ak plan validate` passed.

## Remaining

M7 is incomplete. No signed trusted release origin/keys, online provider-auth setup, paired app/SDK updater and rollback, AppImage, signed macOS/Windows installers, actual apt lifecycle qualification, authentic provider journey, physical-device qualification, or codec/source-distribution clearance. The Linux SDK still has the host `libx265.so.199` dependency issue. No release was published.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
