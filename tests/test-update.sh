#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
mkdir -p "$ROOT/.work"
tmp="$(mktemp -d "$ROOT/.work/update-test.XXXXXX")"
trap 'find "$tmp" -depth -delete' 0 1 2 15
fixture="$tmp/fixture"
install="$tmp/install"
fakebin="$tmp/fakebin"
package="$tmp/package"
mkdir -p "$fixture" "$install" "$fakebin" "$package"

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

for binary in opencoding opencoding-daemon opencoding-cli opencoding-update; do
  printf '#!/bin/sh\nprintf "old-%s\\n"\n' "$binary" > "$install/$binary"
  chmod 0755 "$install/$binary"
done
printf 'v1.0.0\n' > "$install/.opencoding-version"

for binary in opencoding-daemon opencoding-cli; do
  name="$binary"
  printf '%s\n' '#!/bin/sh' \
    'if [ "${1:-}" = "--self-test" ]; then' \
    '  case "$0" in' \
    '    "$OPENCODING_INSTALL_DIR"/*) [ "${FAIL_AFTER_INSTALL:-0}" = "0" ] || exit 42 ;;' \
    '  esac' \
    '  exit 0' \
    'fi' \
    "printf 'new-${name}\\n'" > "$package/$binary"
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
: > "$fixture/SHA256SUMS.sig"
: > "$fixture/SHA256SUMS.pem"

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
printf '%s\n' '#!/bin/sh' 'exit 0' > "$fakebin/cosign"
chmod 0755 "$fakebin/curl" "$fakebin/cosign"

run_update() {
  env PATH="$fakebin:$PATH" FIXTURE_DIR="$fixture" \
    OPENCODING_REPOSITORY="test/repository" OPENCODING_VERSION="v2.0.0" \
    OPENCODING_INSTALL_DIR="$install" FAIL_AFTER_INSTALL="$1" \
    "$ROOT/scripts/update.sh"
}

if run_update 1 >/dev/null 2>&1; then
  echo "update unexpectedly passed a failing post-install health check" >&2
  exit 1
fi
for binary in opencoding opencoding-daemon opencoding-cli opencoding-update; do
  "$install/$binary" | grep "old-$binary" >/dev/null
done
[ "$(sed -n '1p' "$install/.opencoding-version")" = "v1.0.0" ]

run_update 0 >/dev/null
[ "$(sed -n '1p' "$install/.opencoding-version")" = "v2.0.0" ]
"$install/opencoding-daemon" | grep 'new-opencoding-daemon' >/dev/null
"$install/opencoding-cli" | grep 'new-opencoding-cli' >/dev/null
"$install/opencoding" web | grep 'new-opencoding-daemon' >/dev/null
"$install/opencoding" cli | grep 'new-opencoding-cli' >/dev/null

launcher="$tmp/launcher"
mkdir -p "$launcher"
cp "$ROOT/scripts/opencoding" "$launcher/opencoding"
printf '%s\n' '#!/bin/sh' 'printf "%s\n" "${OPENCODING_VERSION:-latest}"' > "$launcher/opencoding-update"
chmod 0755 "$launcher/opencoding" "$launcher/opencoding-update"
[ "$("$launcher/opencoding" update v2.1.0)" = "v2.1.0" ]
if "$launcher/opencoding" update --unsafe >/dev/null 2>&1; then
  echo "public update launcher accepted an option as a release version" >&2
  exit 1
fi
echo "signed updater rollback test passed"
