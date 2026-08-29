#!/bin/sh
set -eu

REPOSITORY="${OPENCODING_REPOSITORY:-shilongliu-iteria/opencoding-community}"
VERSION="${OPENCODING_VERSION:-latest}"
INSTALL_DIR="${OPENCODING_INSTALL_DIR:-$HOME/.local/bin}"
IDENTITY="https://github.com/${REPOSITORY}/.github/workflows/release.yml@refs/tags/"
ISSUER="https://token.actions.githubusercontent.com"

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 1; }
command -v cosign >/dev/null 2>&1 || { echo "cosign is required to verify releases" >&2; exit 1; }
command -v openssl >/dev/null 2>&1 || { echo "openssl is required" >&2; exit 1; }

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
lock="$INSTALL_DIR/.opencoding-update.lock"
locked=0
swap_started=0
committed=0
cleanup() {
  status=$?
  trap - 0 1 2 15
  if [ "$swap_started" -eq 1 ] && [ "$committed" -eq 0 ]; then
    rollback
  fi
  [ "$locked" -eq 0 ] || rmdir "$lock" 2>/dev/null || true
  rm -rf "$tmp"
  exit "$status"
}
trap cleanup 0 1 2 15

mkdir -p "$INSTALL_DIR"
if ! mkdir "$lock" 2>/dev/null; then
  echo "another Opencoding update is in progress" >&2
  exit 1
fi
locked=1

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
version_is_not_older() {
  awk -v candidate="${1#v}" -v current="${2#v}" 'BEGIN {
    split(candidate, ca, "-"); split(current, cu, "-");
    split(ca[1], a, "."); split(cu[1], b, ".");
    for (i = 1; i <= 3; i++) {
      av = a[i] + 0; bv = b[i] + 0;
      if (av > bv) exit 0;
      if (av < bv) exit 1;
    }
    exit 0;
  }'
}
if [ -f "$INSTALL_DIR/.opencoding-version" ] && [ "${OPENCODING_ALLOW_DOWNGRADE:-0}" != "1" ]; then
  current_version="$(sed -n '1p' "$INSTALL_DIR/.opencoding-version")"
  if ! version_is_not_older "$release_version" "$current_version"; then
    echo "refusing downgrade from $current_version to $release_version" >&2
    exit 1
  fi
fi

mkdir -p "$tmp/unpack"
tar -xzf "$tmp/$archive" -C "$tmp/unpack"
for binary in opencoding opencoding-daemon opencoding-cli opencoding-update; do
  [ -x "$tmp/unpack/$binary" ] || { echo "release is missing $binary" >&2; exit 1; }
  cp "$tmp/unpack/$binary" "$tmp/$binary.new"
  chmod 0755 "$tmp/$binary.new"
done
for binary in opencoding-daemon opencoding-cli; do
  "$tmp/unpack/$binary" --self-test >/dev/null
done

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
}

if [ -e "$INSTALL_DIR/.opencoding-version" ]; then
  cp "$INSTALL_DIR/.opencoding-version" "$tmp/version.previous"
fi
swap_started=1
for binary in opencoding opencoding-daemon opencoding-cli opencoding-update; do
  if [ -e "$INSTALL_DIR/$binary" ]; then
    mv "$INSTALL_DIR/$binary" "$tmp/$binary.previous"
  fi
  mv "$tmp/$binary.new" "$INSTALL_DIR/$binary"
done

if ! "$INSTALL_DIR/opencoding-daemon" --self-test >/dev/null 2>&1 \
  || ! "$INSTALL_DIR/opencoding-cli" --self-test >/dev/null 2>&1 \
  || ! "$INSTALL_DIR/opencoding" --help >/dev/null 2>&1; then
  echo "update health check failed; previous installation restored" >&2
  exit 1
fi

printf '%s\n' "$release_version" > "$INSTALL_DIR/.opencoding-version"
committed=1
echo "Updated Opencoding to $release_version in $INSTALL_DIR"
