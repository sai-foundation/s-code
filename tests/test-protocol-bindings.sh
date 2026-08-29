#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mkdir -p "$root/.work"
task=$(mktemp -d "$root/.work/protocol-bindings.XXXXXX")
trap 'find "$task" -depth -delete' EXIT HUP INT TERM

mkdir -p "$task/tmp" "$task/generated"
export TMPDIR="$task/tmp"
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
