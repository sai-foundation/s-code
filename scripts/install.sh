#!/bin/sh
set -eu

REPOSITORY="${OPENCODING_REPOSITORY:-shilongliu-iteria/opencoding-community}"
VERSION="${OPENCODING_VERSION:-latest}"
INSTALL_DIR="${OPENCODING_INSTALL_DIR:-$HOME/.local/bin}"
IDENTITY="https://github.com/${REPOSITORY}/.github/workflows/release.yml@refs/tags/"
ISSUER="https://token.actions.githubusercontent.com"

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 1; }
command -v cosign >/dev/null 2>&1 || { echo "cosign is required to verify the release signature" >&2; exit 1; }

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-gnu" ;;
  *) echo "unsupported operating system" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) echo "unsupported architecture" >&2; exit 1 ;;
esac

if [ "$VERSION" = "latest" ]; then
  base="https://github.com/${REPOSITORY}/releases/latest/download"
else
  base="https://github.com/${REPOSITORY}/releases/download/${VERSION}"
fi
archive="opencoding-${arch}-${os}.tar.gz"
tmp="$(mktemp -d)"
lock=""
swap_started=0
committed=0
cleanup() {
  status=$?
  trap - 0 1 2 15
  if [ "$swap_started" -eq 1 ] && [ "$committed" -eq 0 ]; then
    rollback
  fi
  [ -z "$lock" ] || rmdir "$lock" 2>/dev/null || true
  rm -rf "$tmp"
  exit "$status"
}
trap cleanup 0 1 2 15

curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error "$base/$archive" --output "$tmp/$archive"
for file in RELEASE.json SHA256SUMS SHA256SUMS.sig SHA256SUMS.pem; do
  curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error "$base/$file" --output "$tmp/$file"
done
cosign verify-blob "$tmp/SHA256SUMS" \
  --signature "$tmp/SHA256SUMS.sig" \
  --certificate "$tmp/SHA256SUMS.pem" \
  --certificate-identity-regexp "^${IDENTITY}v?[0-9]" \
  --certificate-oidc-issuer "$ISSUER" >/dev/null

verify_checksum() {
  name="$1"
  expected="$(awk -v name="$name" '$2 == name { print $1 }' "$tmp/SHA256SUMS")"
  [ -n "$expected" ] || { echo "$name is absent from signed checksums" >&2; exit 1; }
  actual="$(openssl dgst -sha256 "$tmp/$name" | awk '{print $NF}')"
  [ "$actual" = "$expected" ] || { echo "$name checksum mismatch" >&2; exit 1; }
}
verify_checksum "$archive"
verify_checksum RELEASE.json
release_version="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$tmp/RELEASE.json")"
[ -n "$release_version" ] || { echo "signed release metadata has no version" >&2; exit 1; }
if [ "$VERSION" != "latest" ] && [ "${VERSION#v}" != "${release_version#v}" ]; then
  echo "signed release version does not match requested version" >&2
  exit 1
fi

mkdir -p "$tmp/unpack" "$INSTALL_DIR"
tar -xzf "$tmp/$archive" -C "$tmp/unpack"
for binary in opencoding opencoding-daemon opencoding-cli opencoding-update; do
  [ -x "$tmp/unpack/$binary" ] || { echo "release is missing $binary" >&2; exit 1; }
done
for binary in opencoding-daemon opencoding-cli; do
  "$tmp/unpack/$binary" --self-test >/dev/null
done

lock="$INSTALL_DIR/.opencoding-update.lock"
if ! mkdir "$lock" 2>/dev/null; then
  echo "another Opencoding install or update is in progress" >&2
  exit 1
fi
rollback() {
  for binary in opencoding opencoding-daemon opencoding-cli opencoding-update; do
    rm -f "$INSTALL_DIR/$binary"
    if [ -e "$tmp/$binary.previous" ]; then
      mv "$tmp/$binary.previous" "$INSTALL_DIR/$binary"
    fi
  done
  rm -f "$INSTALL_DIR/.opencoding-version"
  if [ -e "$tmp/version.previous" ]; then
    mv "$tmp/version.previous" "$INSTALL_DIR/.opencoding-version"
  fi
  rm -f "$INSTALL_DIR"/.opencoding-*.new."$$"
}
if [ -e "$INSTALL_DIR/.opencoding-version" ]; then
  cp "$INSTALL_DIR/.opencoding-version" "$tmp/version.previous"
fi
swap_started=1
for binary in opencoding opencoding-daemon opencoding-cli opencoding-update; do
  staging="$INSTALL_DIR/.${binary}.new.$$"
  cp "$tmp/unpack/$binary" "$staging"
  chmod 0755 "$staging"
  if [ -e "$INSTALL_DIR/$binary" ]; then
    mv "$INSTALL_DIR/$binary" "$tmp/$binary.previous"
  fi
  mv -f "$staging" "$INSTALL_DIR/$binary"
done
if ! "$INSTALL_DIR/opencoding-daemon" --self-test >/dev/null 2>&1 \
  || ! "$INSTALL_DIR/opencoding-cli" --self-test >/dev/null 2>&1 \
  || ! "$INSTALL_DIR/opencoding" --help >/dev/null 2>&1; then
  echo "installation health check failed; previous installation restored" >&2
  exit 1
fi
printf '%s\n' "$release_version" > "$INSTALL_DIR/.opencoding-version"
committed=1
rmdir "$lock"
lock=""
echo "Installed Opencoding $release_version to $INSTALL_DIR"
