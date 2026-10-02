#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Supply the actual FFmpeg 9 install used by fframes.
exec python3 "$SCRIPT_DIR/assemble-phase-zero-sdk.py" "$@"
