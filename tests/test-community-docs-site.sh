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

if [ -n "${S_CODE_SHARED_NPM_CACHE:-}" ]; then
  case "$S_CODE_SHARED_NPM_CACHE" in
    "$ROOT/.work/"*) ;;
    *) echo "shared npm cache must be below $ROOT/.work" >&2; exit 2 ;;
  esac
  mkdir -p "$S_CODE_SHARED_NPM_CACHE"
  export npm_config_cache="$S_CODE_SHARED_NPM_CACHE"
else
  export npm_config_cache="$TASK/npm-cache"
fi
SITE="$TASK/community/docs-site"

# Keep the search palette inside cmdk's required provider and let responsive
# utility classes, rather than a global display declaration, control header
# links on narrow screens.
grep -F '<Command>{children}</Command>' "$SITE/components/ui/command.tsx" >/dev/null || {
  echo "documentation search dialog is missing its Command provider" >&2
  exit 1
}
grep -F 'className="header-link hidden px-2 xl:inline-flex"' "$SITE/components/site-header.tsx" >/dev/null || {
  echo "documentation privacy link is not hidden on narrow screens" >&2
  exit 1
}
if grep -E '^\.header-link[^}]*display:' "$SITE/app/globals.css" >/dev/null; then
  echo "documentation header styles override responsive display utilities" >&2
  exit 1
fi

npm ci --prefix "$SITE" --no-audit --no-fund
npm run docs:check --prefix "$SITE"
npm run lint --prefix "$SITE"
# Dependabot scans the full lockfile; keep this synchronous gate bounded to
# dependencies shipped by the documentation site.
if [ "${S_CODE_SKIP_NETWORK_AUDIT:-0}" = 1 ]; then
  echo "deferred documentation dependency audit to Dependabot and release verification"
else
  audit_attempt=1
  while ! npm audit --prefix "$SITE" --package-lock-only --omit=dev \
    --audit-level=high --fetch-timeout=60000 --fetch-retries=0; do
    if [ "$audit_attempt" -ge 3 ]; then
      echo "documentation dependency audit failed after $audit_attempt attempts" >&2
      exit 1
    fi
    audit_attempt=$((audit_attempt + 1))
    echo "retrying documentation dependency audit ($audit_attempt/3)" >&2
  done
fi
npm run build --prefix "$SITE"
npm run build:pages --prefix "$SITE"

echo "Community documentation site verification passed"
