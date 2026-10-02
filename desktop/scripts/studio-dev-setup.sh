#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== fframes Studio Dev Setup ==="
echo "Running GPUI-free doctor companion..."

cargo run --manifest-path "$DESKTOP_DIR/Cargo.toml" -p studio-sdk --bin studio_setup -- doctor "$@"
