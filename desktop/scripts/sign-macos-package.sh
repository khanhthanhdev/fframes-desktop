#!/usr/bin/env bash
set -euo pipefail

fail() {
    printf 'macOS release signing failed: %s\n' "$1" >&2
    exit 1
}

[[ "$(uname -s)" == "Darwin" ]] || fail "this script must run on a macOS release runner"
[[ $# == 1 ]] || fail "usage: sign-macos-package.sh PACKAGE_DIRECTORY"

package_dir="$(cd -- "$1" && pwd -P)"
app="$package_dir/fframes Studio.app"
archive="$package_dir.zip"
scripts_dir="$(cd -- "$(dirname -- "$0")" && pwd -P)"

required_secrets=(
    APPLE_DEVELOPER_ID_P12_BASE64
    APPLE_DEVELOPER_ID_P12_PASSWORD
    APPLE_DEVELOPER_IDENTITY
    APPLE_NOTARY_KEY_P8_BASE64
    APPLE_NOTARY_KEY_ID
    APPLE_NOTARY_ISSUER_ID
)
for name in "${required_secrets[@]}"; do
    [[ -n "${!name:-}" ]] || fail "required GitHub Actions secret/environment variable $name is not configured"
done
[[ "$APPLE_DEVELOPER_IDENTITY" == "Developer ID Application: "* ]] \
    || fail "APPLE_DEVELOPER_IDENTITY must name a Developer ID Application certificate"
[[ -d "$app" ]] || fail "packaged app bundle is missing: $app"
[[ -f "$archive" ]] || fail "packaged archive is missing: $archive"

temporary_dir="$(mktemp -d "$RUNNER_TEMP/fframes-signing.XXXXXX")"
keychain="$temporary_dir/signing.keychain-db"
keychain_password="$(openssl rand -hex 32)"
certificate="$temporary_dir/developer-id.p12"
notary_key="$temporary_dir/notary-key.p8"

cleanup() {
    security delete-keychain "$keychain" >/dev/null 2>&1 || true
    rm -rf -- "$temporary_dir"
}
trap cleanup EXIT
chmod 700 "$temporary_dir"

decode_secret() {
    local name="$1"
    local destination="$2"
    printf '%s' "${!name}" | python3 -c 'import base64,sys; data=b"".join(sys.stdin.buffer.read().split()); sys.stdout.buffer.write(base64.b64decode(data, validate=True))' > "$destination" \
        || fail "$name is not valid base64"
    chmod 600 "$destination"
    [[ -s "$destination" ]] || fail "$name decoded to an empty file"
}

decode_secret APPLE_DEVELOPER_ID_P12_BASE64 "$certificate"
decode_secret APPLE_NOTARY_KEY_P8_BASE64 "$notary_key"
identity="$APPLE_DEVELOPER_IDENTITY"
certificate_password="$APPLE_DEVELOPER_ID_P12_PASSWORD"
notary_key_id="$APPLE_NOTARY_KEY_ID"
notary_issuer_id="$APPLE_NOTARY_ISSUER_ID"
unset APPLE_DEVELOPER_ID_P12_BASE64 APPLE_NOTARY_KEY_P8_BASE64
unset APPLE_DEVELOPER_IDENTITY APPLE_DEVELOPER_ID_P12_PASSWORD
unset APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID

security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$certificate" -k "$keychain" -P "$certificate_password" -T /usr/bin/codesign -T /usr/bin/security >/dev/null
unset certificate_password
security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_password" "$keychain" >/dev/null

identities="$(security find-identity -v -p codesigning "$keychain")"
grep -F -- "$identity" <<<"$identities" >/dev/null \
    || fail "the imported certificate does not contain the configured Developer ID Application identity"

sign_file() {
    local file="$1"
    codesign --force --timestamp --options runtime --keychain "$keychain" --sign "$identity" "$file"
    codesign --verify --strict --verbose=2 "$file"
}

for name in fframes-studio studio_setup studio-tools studio-mcp; do
    binary="$package_dir/bin/$name"
    [[ -f "$binary" ]] || fail "expected packaged Mach-O executable is missing: $binary"
    sign_file "$binary"
done

while IFS= read -r -d '' binary; do
    sign_file "$binary"
done < <(find "$app/Contents/MacOS" -maxdepth 1 -type f -print0)

while IFS= read -r -d '' library; do
    sign_file "$library"
done < <(find "$app/Contents" -type f \( -name '*.dylib' -o -name '*.so' \) -print0)

codesign --force --timestamp --options runtime --keychain "$keychain" --sign "$identity" "$app"
codesign --verify --deep --strict --verbose=2 "$app"
python3 "$scripts_dir/finalize-native-package.py" "$package_dir"

notary_archive="$temporary_dir/app-notary.zip"
ditto -c -k --keepParent "$app" "$notary_archive"
xcrun notarytool submit "$notary_archive" \
    --key "$notary_key" \
    --key-id "$notary_key_id" \
    --issuer "$notary_issuer_id" \
    --wait
xcrun stapler staple "$app"
xcrun stapler validate "$app"
codesign --verify --deep --strict --verbose=2 "$app"
spctl --assess --type execute --verbose=2 "$app"
python3 "$scripts_dir/finalize-native-package.py" "$package_dir"
