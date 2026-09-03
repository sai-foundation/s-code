#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mkdir -p "$root/.work"
task=$(mktemp -d "$root/.work/protocol-bindings.XXXXXX")
trap 'find "$task" -depth -delete' EXIT HUP INT TERM

mkdir -p "$task/tmp" "$task/generated"
if [ -n "${OPENCODING_SHARED_TEST_TMPDIR:-}" ]; then
  case "$OPENCODING_SHARED_TEST_TMPDIR" in
    "$root/.work/"*) ;;
    *) echo "shared test TMPDIR must be below $root/.work" >&2; exit 2 ;;
  esac
  mkdir -p "$OPENCODING_SHARED_TEST_TMPDIR"
  export TMPDIR="$OPENCODING_SHARED_TEST_TMPDIR"
else
  export TMPDIR="$task/tmp"
fi
chmod 0700 "$TMPDIR"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$task/target}"

cargo run --quiet --locked --manifest-path "$root/Cargo.toml" \
  -p opencoding-protocol --bin export_web_types -- "$task/generated"
diff -ru "$root/web/generated/protocol" "$task/generated"
diff -ru "$root/web/generated/api" "$task/api"
python3 - "$task/api/openapi.json" <<'PY'
import json
import sys

document = json.load(open(sys.argv[1], encoding="utf-8"))
assert document["openapi"] == "3.1.0"
required = {
    "createSession",
    "updateSession",
    "transcriptSnapshot",
    "createTurn",
    "cancelTurn",
    "resolveApproval",
    "listDurableTasks",
    "createDurableTask",
    "listClientPresence",
    "updateClientPresence",
    "removeClientPresence",
    "teamGovernance",
}
operations = {
    operation["operationId"]
    for path in document["paths"].values()
    for operation in path.values()
    if isinstance(operation, dict) and "operationId" in operation
}
assert required <= operations, sorted(required - operations)
assert len(operations) == sum(
    1
    for path in document["paths"].values()
    for operation in path.values()
    if isinstance(operation, dict) and "operationId" in operation
)
assert document["components"]["schemas"]["Scope"]["additionalProperties"] is False
PY

echo "generated Web protocol, OpenAPI, and SDK bindings are current"
