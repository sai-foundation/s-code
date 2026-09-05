#!/bin/sh
set -eu

release_workflow="$(dirname "$0")/../.github/workflows/release.yml"
grep -F 'git_tag_and_attested_source_archive' "$release_workflow" >/dev/null
grep -F 'anchore/sbom-action@3ad7283483fc7af8ff2b4ea19663c2d5ca935e26 # v0.24.2' "$release_workflow" >/dev/null
test "$(grep -Fc 'actions/attest@f7c74d28b9d84cb8768d0b8ca14a4bac6ef463e6 # v4.2.0' "$release_workflow")" -eq 2
for permission in 'artifact-metadata: write' 'attestations: write' 'id-token: write'; do
  grep -F "$permission" "$release_workflow" >/dev/null
done
for asset in s-code.spdx.json source-provenance.sigstore.json source-sbom.sigstore.json; do
  grep -F "$asset" "$release_workflow" >/dev/null
done
# A release must remain private as a draft until the complete asset set has
# been uploaded, downloaded again and checksum-verified. Same-tag runs are
# serialized, and a retry must refuse to replace an already-published release.
grep -F 'cancel-in-progress: false' "$release_workflow" >/dev/null
grep -F -- '--draft \' "$release_workflow" >/dev/null
grep -F 'gh release upload "$VERSION"' "$release_workflow" >/dev/null
grep -F 'gh release download "$VERSION"' "$release_workflow" >/dev/null
grep -F 'sha256sum --check SHA256SUMS' "$release_workflow" >/dev/null
grep -F 'refusing to replace an already-published release' "$release_workflow" >/dev/null
grep -F -- '--draft=false' "$release_workflow" >/dev/null
draft_line="$(grep -n -F -- '--draft \' "$release_workflow" | tail -1 | cut -d: -f1)"
publish_line="$(grep -n -F -- '--draft=false' "$release_workflow" | tail -1 | cut -d: -f1)"
test "$draft_line" -lt "$publish_line"

# RUSTSEC-2023-0071 is present only in Cargo.lock metadata for SQLx's disabled
# MySQL backend. It must never enter the compiled normal dependency graph.
if cargo tree --workspace --edges normal | grep -Eq '(^|[[:space:]])rsa v0\.9\.10([[:space:]]|$)'; then
  echo "error: vulnerable rsa 0.9.10 entered the active dependency graph" >&2
  exit 1
fi

cargo audit --deny warnings --ignore RUSTSEC-2023-0071
