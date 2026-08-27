#!/bin/sh
set -eu

# RUSTSEC-2023-0071 is present only in Cargo.lock metadata for SQLx's disabled
# MySQL backend. It must never enter the compiled normal dependency graph.
if cargo tree --workspace --edges normal | grep -Eq '(^|[[:space:]])rsa v0\.9\.10([[:space:]]|$)'; then
  echo "error: vulnerable rsa 0.9.10 entered the active dependency graph" >&2
  exit 1
fi

cargo audit --deny warnings --ignore RUSTSEC-2023-0071
