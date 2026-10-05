# M3 protocol research

Date: 2026-10-04. This record supports planning; it does not qualify an adapter.

The roadmap explicitly selects ACP v1. Its [prompt-turn specification](https://agentclientprotocol.com/protocol/v1/prompt-turn) makes the correlated `session/prompt` response and its `stopReason` authoritative for turn completion. Streamed messages, tool updates and quiet intervals are not completion. Cancellation must resolve pending permissions as cancelled. Only content negotiated during initialization can be sent.

The [official Rust SDK repository](https://github.com/agentclientprotocol/rust-sdk) is the dependency candidate. Current documentation also exposes v2 behavior, so implementation must select and lock a version with tested v1 support instead of importing latest examples indiscriminately. Check its MSRV against the desktop toolchain, license, framing limits and cancellation behavior before dependency adoption. No exact SDK version was selected by this planning session.

The [v1 session setup contract](https://agentclientprotocol.com/protocol/v1/session-setup) supplies sessions and MCP configuration. Use capabilities to select image content, configuration and restoration. A successful initialize/new session does not establish authenticated editing, supported MCP tools or safe continuing sessions. A real two-edit workflow must prove these boundaries.

The [MCP stdio transport contract](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) separates protocol stdout from diagnostics stderr and frames messages by newlines. Use a version supported by the selected client/server implementation and record its negotiated version. Share the same task-scoped operations with the CLI facade; don't add arbitrary shell or file-write methods. This dated reference is a compatibility candidate, not a requirement to negotiate a version the provider lacks.

Implementation decisions inherited from repository evidence: keep configurable adapter executables and credentials owned by the adapter; keep the upstream GPUI pin, isolated SDK builds, source inventories, owned process trees and revision-safe preview. A draft working directory protects normal editing workflow but is not a sandbox for arbitrary adapter tools or Cargo build scripts.

Open execution gates: exact SDK/MCP dependency pins; a real authenticated adapter/account; provider-specific visual/configuration/restoration/tool support; physical presentation/audio and other operating systems. These gates remain explicit and must not be inferred from protocol fixtures.
