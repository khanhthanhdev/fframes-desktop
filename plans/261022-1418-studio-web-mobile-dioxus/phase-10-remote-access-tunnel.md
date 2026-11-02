---
phase: 10
title: "Remote access tunnel"
status: pending
priority: P2
effort: "1.5w"
dependencies: [8]
---

# Phase 10: Remote access tunnel

## Goal

The phone and a browser reach the user's Studio server from outside the local network, the way T3 Code
does: first over the user's own Tailscale tailnet, later optionally through a managed relay.

## Context

- Local network pairing from phases 3 and 8.
- T3 Code offers three routes: direct pairing with a one-time code and QR, `t3 pair --tailscale`
  (publishes the server on the user's tailnet with MagicDNS addresses), and T3 Connect (a relay client
  on the computer opens a tunnel to a hosted relay that gives a stable public URL). Sources:
  [T3 Code remote access docs (mirror)](https://ithub.global.ssl.fastly.net/Type-Delta/t3code/blob/main/docs/user/remote-access.md),
  [DeepWiki: cloud and remote access](https://deepwiki.com/pingdotgg/t3code/7-cloud-and-remote-access),
  [T3 Code README](https://raw.githubusercontent.com/pingdotgg/t3code/main/README.md).
  These are mirrors and summaries; confirm against the current upstream docs before implementing.

## Requirements

- **Tailnet first.** **Settings → Remote devices → Tailscale** detects an installed and signed-in
  Tailscale, publishes the server on the tailnet with `tailscale serve` (HTTPS with a certificate
  Tailscale issues, so no self-signed pinning is needed on that route), and shows the MagicDNS address in
  the pairing code. Studio never installs Tailscale or changes its settings beyond its own `serve` entry,
  and removes that entry when remote access is turned off.
- Pairing, device tokens, role limits and revoke work exactly as on the local network.
- The phone stores several routes per computer (LAN, tailnet) and tries the LAN one first.
- **Managed relay later.** A hosted relay needs a service run by the project (accounts, relay servers,
  cost). It is designed here only as an interface (`RemoteRoute` trait in the server) and is built only
  after the user decides to run that service.
- A public route never exposes the server without a device token; the Origin and token checks from
  phase 3 stay in force.

## Architecture

- `studio-server/src/remote/`: `RemoteRoute` trait (`enable`, `disable`, `status`, `addresses`),
  `lan.rs` (the phase 3 TLS listener), `tailscale.rs` (runs the user's `tailscale` CLI by absolute path
  found the same way Studio finds agent CLIs, never from a `PATH` search; parses `tailscale status --json`).
- Pairing payload version 2: a list of routes with kind, address and, for the LAN route, the certificate
  fingerprint.

## Related code files

- Create: `desktop/crates/studio-server/src/remote/{mod.rs,lan.rs,tailscale.rs}`
- Modify: `desktop/crates/studio-server/src/devices.rs`, `desktop/crates/studio-api` (pairing payload),
  `desktop/clients/studio-web/src/mobile/pair.rs`, settings page for remote devices

## Implementation steps

1. Move the LAN listener behind `RemoteRoute`.
2. Tailscale detection, `serve` enable and disable, status; tests with a fake `tailscale` executable.
3. Pairing payload v2 and multi-route reconnect on the phone.
4. Manual check: phone on mobile data reaches the server through the tailnet; turning remote access off
   removes the `serve` entry and the phone is refused.
5. Write the managed relay design note in `reports/managed-relay-design.md` for a later decision.

## Todo

- [ ] RemoteRoute and LAN route
- [ ] Tailscale route with fake-CLI tests
- [ ] Pairing payload v2 and multi-route phone reconnect
- [ ] Manual off-network check
- [ ] Managed relay design note

## Success criteria

A paired Android phone on mobile data prompts, watches and approves through the tailnet; turning the
feature off closes the route.

## Security considerations

The tailnet limits who can reach the server to the user's own devices; the device token still applies.
Turning the feature off must leave no `serve` entry behind.

## Risks

Tailscale's `serve` command line may change between versions; parse its JSON status and record the
tested version.
