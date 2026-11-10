#!/bin/sh
# Installs fframes Studio for the current user on macOS (Apple silicon) or Linux (x86_64)
# from a GitHub release:
#   curl -fsSL https://github.com/khanhthanhdev/fframes-desktop/releases/latest/download/install.sh | sh
# With options:
#   curl -fsSL .../install.sh | sh -s -- --version v0.2.9 --no-launch
# Downloads the release's package, checks it against SHA256SUMS.txt and installs it without
# administrator rights: ~/Applications/fframes Studio.app on macOS, ~/.local/share/fframes-studio
# on Linux, plus an `fframes-studio` command in ~/.local/bin. Rust, FFmpeg and the fframes
# framework come with Studio's managed SDK; projects also need the system C toolchain, which
# this script names when it is missing. The builds are not signed; curl marks no download as
# quarantined, so Gatekeeper does not stop the app. Reinstalling keeps projects and app data.
set -eu

REPOSITORY="khanhthanhdev/fframes-desktop"
version="latest"
launch=1
base_url=""

usage() {
    cat <<'EOF'
Usage: install.sh [--version vX.Y.Z] [--no-launch] [--base-url URL]
  --version    a release tag such as v0.2.9 (default: latest)
  --no-launch  do not start fframes Studio after installing
  --base-url   download location override (a mirror, or a local server for testing)
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) version="${2:?--version needs a tag}"; shift 2 ;;
        --no-launch) launch=0; shift ;;
        --base-url) base_url="${2:?--base-url needs a URL}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

fail() {
    echo "error: $*" >&2
    exit 1
}

os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
    Darwin/arm64) target="aarch64-apple-darwin" ;;
    Linux/x86_64) target="x86_64-unknown-linux-gnu" ;;
    Darwin/*) fail "fframes Studio needs a Mac with Apple silicon." ;;
    Linux/*) fail "fframes Studio for Linux needs an x86_64 machine; this one is $arch." ;;
    *) fail "This script installs on macOS and Linux; on Windows run: irm https://github.com/$REPOSITORY/releases/latest/download/install.ps1 | iex" ;;
esac
package="fframes-studio-$target"

command -v curl >/dev/null 2>&1 || fail "curl is required."
command -v unzip >/dev/null 2>&1 || fail "unzip is required (for example: sudo apt install unzip)."
if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    fail "sha256sum or shasum is required."
fi

if [ -z "$base_url" ]; then
    if [ "$version" = "latest" ]; then
        base_url="https://github.com/$REPOSITORY/releases/latest/download"
    else
        base_url="https://github.com/$REPOSITORY/releases/download/$version"
    fi
fi
base_url="${base_url%/}"

work="$(mktemp -d "${TMPDIR:-/tmp}/fframes-studio-install.XXXXXX")"
trap 'rm -rf "$work"' EXIT INT TERM

echo "Downloading checksums from $base_url"
curl -fsSL "$base_url/SHA256SUMS.txt" -o "$work/SHA256SUMS.txt"
expected="$(awk -v name="$package.zip" '{ file = $2; sub(/^\*/, "", file); if (file == name) print $1 }' "$work/SHA256SUMS.txt")"
[ -n "$expected" ] || fail "The release has no checksum for $package.zip."

echo "Downloading $package.zip (about 300 MB)"
curl -fL --progress-bar "$base_url/$package.zip" -o "$work/$package.zip"
actual="$(sha256 "$work/$package.zip")"
[ "$actual" = "$expected" ] || fail "Checksum mismatch for $package.zip (expected $expected, got $actual); nothing was installed."

unzip -q "$work/$package.zip" -d "$work/unpacked"
source_dir="$work/unpacked/$package"
[ -d "$source_dir" ] || fail "$package.zip does not contain $package/."

bin_dir="$HOME/.local/bin"
mkdir -p "$bin_dir"
if [ "$os" = "Darwin" ]; then
    app="$HOME/Applications/fframes Studio.app"
    [ -d "$source_dir/fframes Studio.app" ] || fail "$package.zip has no fframes Studio.app."
    mkdir -p "$HOME/Applications"
    rm -rf "$app"
    mv "$source_dir/fframes Studio.app" "$app"
    xattr -dr com.apple.quarantine "$app" 2>/dev/null || true
    executable="$app/Contents/MacOS/fframes-studio"
    location="$app"
else
    location="${XDG_DATA_HOME:-$HOME/.local/share}/fframes-studio"
    mkdir -p "$(dirname "$location")"
    rm -rf "$location"
    mv "$source_dir" "$location"
    executable="$location/bin/fframes-studio"
    applications="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
    mkdir -p "$applications"
    cat > "$applications/fframes-studio.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=fframes Studio
Comment=Author fframes videos with coding agents
Exec="$executable"
Terminal=false
Categories=AudioVideo;Video;Development;
EOF
fi
# A wrapper, not a symlink: on macOS the app's own path is the symlink's when started
# through one, and Studio finds its SDK relative to that path.
rm -f "$bin_dir/fframes-studio"
printf '#!/bin/sh\nexec "%s" "$@"\n' "$executable" > "$bin_dir/fframes-studio"
chmod +x "$bin_dir/fframes-studio"

echo "fframes Studio is installed in $location (command: fframes-studio)."
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Add $bin_dir to your PATH to run fframes-studio from a terminal." ;;
esac
echo "On first launch, Setup installs the managed SDK (Rust, FFmpeg and fframes) and checks the build prerequisites."
if [ "$os" = "Darwin" ]; then
    xcode-select -p >/dev/null 2>&1 || echo "Projects need the Xcode Command Line Tools: xcode-select --install"
else
    # The SDK's host_prerequisites for Linux (desktop/packaging/sdk/phase-zero-sdk.json).
    for tool in clang cmake pkg-config; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            echo "Projects need clang, cmake and pkg-config, for example: sudo apt install clang libclang-dev cmake pkg-config"
            break
        fi
    done
fi

if [ "$launch" = 1 ]; then
    if [ "$os" = "Darwin" ]; then
        open "$location"
    elif [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
        nohup "$executable" >/dev/null 2>&1 &
    fi
fi
