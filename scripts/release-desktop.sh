#!/usr/bin/env bash
# Release helper for fframes Studio native desktop packages.
# Usage:
#   ./scripts/release-desktop.sh 0.1.0
#   ./scripts/release-desktop.sh 0.1.0 --run-id 36963570458
#   ./scripts/release-desktop.sh 0.1.0 --draft
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

command -v git >/dev/null 2>&1 || { echo "Error: git is required" >&2; exit 1; }
command -v gh >/dev/null 2>&1 || { echo "Error: gh (GitHub CLI) is required" >&2; exit 1; }

VERSION="${1:-}"
if [ -z "$VERSION" ] || [[ "$VERSION" =~ ^- ]]; then
  echo "Usage: ./scripts/release-desktop.sh <version> [--run-id <id>] [--draft] [--prerelease]" >&2
  exit 1
fi
shift

VERSION="${VERSION#v}"
TAG="v${VERSION}"

RUN_ID=""
DRAFT="false"
PRERELEASE="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run-id)
      RUN_ID="$2"
      shift 2
      ;;
    --draft)
      DRAFT="true"
      shift
      ;;
    --prerelease)
      PRERELEASE="true"
      shift
      ;;
    *)
      echo "Unknown flag: $1" >&2
      exit 1
      ;;
  esac
done

echo "=== Preparing fframes Studio Release: $TAG ==="

# Check and update version in desktop/app/Cargo.toml
DESKTOP_CARGO="$ROOT_DIR/desktop/app/Cargo.toml"
if [ -f "$DESKTOP_CARGO" ]; then
  CURRENT_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$DESKTOP_CARGO" | head -n1)
  if [ "$CURRENT_VERSION" != "$VERSION" ]; then
    echo "Updating desktop/app/Cargo.toml version from $CURRENT_VERSION to $VERSION..."
    sed -i "s/^version = \".*\"/version = \"$VERSION\"/" "$DESKTOP_CARGO"
    git add "$DESKTOP_CARGO"
    if ! git diff --cached --quiet; then
      git commit -m "chore(desktop): bump desktop app version to $VERSION"
    fi
  fi
fi

# Ensure tag exists locally
if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  echo "Creating tag $TAG..."
  git tag -a "$TAG" -m "Release $TAG"
fi

echo "Pushing tag $TAG to origin..."
git push origin "$TAG"

if [ -n "$RUN_ID" ]; then
  echo "Triggering desktop release workflow from existing CI run $RUN_ID..."
  gh workflow run desktop-release.yml \
    -f tag="$TAG" \
    -f run_id="$RUN_ID" \
    -f draft="$DRAFT" \
    -f prerelease="$PRERELEASE"
else
  echo "Triggering desktop release workflow build matrix..."
  gh workflow run desktop-release.yml \
    -f tag="$TAG" \
    -f draft="$DRAFT" \
    -f prerelease="$PRERELEASE"
fi

echo "Release workflow triggered! Watch status with: gh run list --workflow=desktop-release.yml"
