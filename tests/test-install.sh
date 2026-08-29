#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
mkdir -p "$ROOT/.work"
tmp="$(mktemp -d "$ROOT/.work/install-test.XXXXXX")"
trap 'find "$tmp" -depth -delete' 0 1 2 15
fixture="$tmp/fixture"
fakebin="$tmp/fakebin"
package="$tmp/package"
mkdir -p "$fixture" "$fakebin" "$package"

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-gnu" ;;
  *) exit 0 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) exit 0 ;;
esac
archive="opencoding-${arch}-${os}.tar.gz"

for binary in opencoding-daemon opencoding-cli; do
  name="$binary"
  printf '%s\n' '#!/bin/sh' \
    'if [ "${1:-}" = "--self-test" ]; then exit 0; fi' \
    "printf 'installed-${name}\\n'" > "$package/$binary"
  chmod 0755 "$package/$binary"
done
cp "$ROOT/scripts/opencoding" "$package/opencoding"
chmod 0755 "$package/opencoding"
cp "$ROOT/scripts/update.sh" "$package/opencoding-update"
chmod 0755 "$package/opencoding-update"
tar -C "$package" -czf "$fixture/$archive" .
printf '{"schema_version":1,"version":"v2.0.0"}\n' > "$fixture/RELEASE.json"
for file in "$archive" RELEASE.json; do
  hash="$(openssl dgst -sha256 "$fixture/$file" | awk '{print $NF}')"
  printf '%s  %s\n' "$hash" "$file" >> "$fixture/SHA256SUMS"
done
: > "$fixture/SHA256SUMS.sigstore.json"

printf '%s\n' '#!/bin/sh' \
  'out=""' 'url=""' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in' \
  '    --output) out="$2"; shift 2 ;;' \
  '    http*) url="$1"; shift ;;' \
  '    *) shift ;;' \
  '  esac' \
  'done' \
  'cp "$FIXTURE_DIR/${url##*/}" "$out"' > "$fakebin/curl"
printf '%s\n' '#!/bin/sh' \
  'printf "%s\n" "$*" >> "$COSIGN_ARGS_FILE"' \
  '[ "${COSIGN_FAIL:-0}" = "0" ]' > "$fakebin/cosign"
chmod 0755 "$fakebin/curl" "$fakebin/cosign"

run_install() {
  destination="$1"
  cosign_fail="$2"
  env PATH="$fakebin:$PATH" FIXTURE_DIR="$fixture" \
    COSIGN_ARGS_FILE="$tmp/cosign-args" COSIGN_FAIL="$cosign_fail" \
    OPENCODING_REPOSITORY="test/repository" OPENCODING_VERSION="v2.0.0" \
    OPENCODING_INSTALL_DIR="$destination" "$ROOT/scripts/install.sh"
}

install="$tmp/install"
run_install "$install" 0 >/dev/null
[ "$(sed -n '1p' "$install/.opencoding-version")" = "v2.0.0" ]
"$install/opencoding-daemon" | grep 'installed-opencoding-daemon' >/dev/null
"$install/opencoding-cli" | grep 'installed-opencoding-cli' >/dev/null
"$install/opencoding" --help | grep 'opencoding web' >/dev/null
"$install/opencoding" web | grep 'installed-opencoding-daemon' >/dev/null
"$install/opencoding" cli | grep 'installed-opencoding-cli' >/dev/null
grep -- '--certificate-identity-regexp' "$tmp/cosign-args" >/dev/null
grep -- 'release.yml@refs/tags/' "$tmp/cosign-args" >/dev/null
grep -- '--certificate-oidc-issuer https://token.actions.githubusercontent.com' "$tmp/cosign-args" >/dev/null
grep -- "--bundle .*SHA256SUMS.sigstore.json" "$tmp/cosign-args" >/dev/null

signature_failure="$tmp/signature-failure"
if run_install "$signature_failure" 1 >/dev/null 2>&1; then
  echo "installer accepted a failed signature verification" >&2
  exit 1
fi
[ ! -e "$signature_failure/opencoding-daemon" ]
[ ! -e "$signature_failure/opencoding" ]

lock_failure="$tmp/lock-failure"
mkdir -p "$lock_failure/.opencoding-update.lock"
if run_install "$lock_failure" 0 >/dev/null 2>&1; then
  echo "installer ignored an active installation lock" >&2
  exit 1
fi
[ ! -e "$lock_failure/opencoding-daemon" ]
[ ! -e "$lock_failure/opencoding" ]

printf 'tampered\n' >> "$fixture/$archive"
checksum_failure="$tmp/checksum-failure"
if run_install "$checksum_failure" 0 >/dev/null 2>&1; then
  echo "installer accepted a checksum mismatch" >&2
  exit 1
fi
[ ! -e "$checksum_failure/opencoding-daemon" ]
[ ! -e "$checksum_failure/opencoding" ]

echo "signed installer contract test passed"
