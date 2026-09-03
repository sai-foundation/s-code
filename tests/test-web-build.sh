#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
TASK_DIR="$ROOT/.work/test-web-build"
BEFORE="$TASK_DIR/app.js"

case "$TASK_DIR" in
  "$ROOT/.work/"*) ;;
  *) echo "refusing unsafe task directory: $TASK_DIR" >&2; exit 2 ;;
esac

[ ! -e "$TASK_DIR" ] || find "$TASK_DIR" -depth -delete
mkdir -p "$TASK_DIR"
trap 'find "$TASK_DIR" -depth -delete' EXIT HUP INT TERM

cp "$ROOT/web/app.js" "$BEFORE"
"$ROOT/tests/test-web-style.sh"
grep -F 'class="artifact-layout is-empty"' "$ROOT/web/index.html" >/dev/null
grep -F 'id="artifact-detail" class="artifact-detail" aria-live="polite" hidden' "$ROOT/web/index.html" >/dev/null
grep -F 'class="extension-layout is-empty"' "$ROOT/web/index.html" >/dev/null
grep -F 'id="extension-detail" class="extension-detail" aria-live="polite" hidden' "$ROOT/web/index.html" >/dev/null
grep -F 'classList.toggle("is-empty", catalogEmpty)' "$ROOT/web/src/pages/workspace-library.ts" >/dev/null
grep -F 'classList.toggle("is-empty", catalogEmpty)' "$ROOT/web/src/pages/extensions.ts" >/dev/null
npm test --prefix "$ROOT/web"
npm run build --prefix "$ROOT/web"
node --check "$ROOT/web/app.js"
if ! cmp -s "$BEFORE" "$ROOT/web/app.js"; then
  echo "web/app.js is stale; run: npm run build --prefix web" >&2
  exit 1
fi

echo "Web TypeScript types and checked-in Vite bundle are synchronized"
