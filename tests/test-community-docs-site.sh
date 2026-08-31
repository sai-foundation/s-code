#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
if [ -d "$ROOT/release/community/docs-site" ]; then
  COMMUNITY_ROOT="$ROOT/release/community"
else
  COMMUNITY_ROOT="$ROOT"
fi

mkdir -p "$ROOT/.work"
TASK="$(mktemp -d "$ROOT/.work/community-docs-site.XXXXXX")"
cleanup() {
  status=$?
  trap - 0 1 2 15
  if [ "$status" -eq 0 ]; then
    find "$TASK" -depth -delete
  else
    echo "Community documentation site verification failed; retained $TASK" >&2
  fi
  exit "$status"
}
trap cleanup 0 1 2 15

mkdir -p "$TASK/community" "$TASK/npm-cache"
find "$COMMUNITY_ROOT/docs" "$COMMUNITY_ROOT/docs-site" -type f \
  ! -path '*/node_modules/*' \
  ! -path '*/dist/*' \
  ! -path '*/.next/*' \
  ! -path '*/.wrangler/*' \
  ! -path '*/lib/generated-docs.ts' | while IFS= read -r source; do
    relative=${source#"$COMMUNITY_ROOT"/}
    destination="$TASK/community/$relative"
    mkdir -p "$(dirname -- "$destination")"
    cp "$source" "$destination"
  done

export npm_config_cache="$TASK/npm-cache"
SITE="$TASK/community/docs-site"
npm ci --prefix "$SITE"
npm run docs:check --prefix "$SITE"
npm run lint --prefix "$SITE"
npm audit --prefix "$SITE" --audit-level=high
npm run build --prefix "$SITE"

echo "Community documentation site verification passed"
