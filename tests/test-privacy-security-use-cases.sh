#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
mkdir -p "$ROOT/.work"
task="$(mktemp -d "$ROOT/.work/privacy-security.XXXXXX")"
trap 'find "$task" -depth -delete' EXIT HUP INT TERM

mkdir -p "$task/logs" "$task/runtime" "$task/tmp"
if [ -n "${S_CODE_SHARED_TEST_TMPDIR:-}" ]; then
  case "$S_CODE_SHARED_TEST_TMPDIR" in
    "$ROOT/.work/"*) ;;
    *) echo "shared test TMPDIR must be below $ROOT/.work" >&2; exit 2 ;;
  esac
  mkdir -p "$S_CODE_SHARED_TEST_TMPDIR"
  export TMPDIR="$S_CODE_SHARED_TEST_TMPDIR"
else
  export TMPDIR="$task/tmp"
fi
chmod 0700 "$TMPDIR"
export S_CODE_RUNTIME_DIR="$task/runtime"
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  export CARGO_TARGET_DIR="$task/target"
fi

run_case() {
  case_id="$1"
  package="$2"
  test_name="$3"
  log="$task/logs/$case_id.log"
  cargo test --locked -p "$package" "tests::$test_name" -- --exact --nocapture >"$log" 2>&1
  if ! grep -F 'test result: ok. 1 passed;' "$log" >/dev/null; then
    cat "$log" >&2
    echo "$case_id did not execute exactly one passing behavior test" >&2
    exit 1
  fi
  printf '%s\n' "$case_id PASS $package::$test_name"
}

case "$(uname -s)" in
  Darwin)
    run_case PS-001 s-code-platform-runtime seatbelt_denies_network_by_default
    run_case PS-002 s-code-platform-runtime seatbelt_allows_workspace_and_denies_external_writes
    ;;
  Linux)
    run_case PS-001 s-code-platform-runtime bubblewrap_denies_network_by_default
    run_case PS-002 s-code-platform-runtime bubblewrap_allows_workspace_and_denies_external_writes
    ;;
  *)
    echo "unsupported privacy/security sandbox platform: $(uname -s)" >&2
    exit 1
    ;;
esac

run_case PS-003 s-code-tool-runtime blocks_parent_traversal_and_secrets
run_case PS-004 s-code-daemon browser_requests_enforce_origin_csrf_and_csp
run_case PS-005 s-code-daemon workspace_permission_auto_approves_local_sandboxed_commands
run_case PS-006 s-code-daemon tool_activity_display_names_the_target_without_exposing_credentials
run_case PS-007 s-code-daemon turn_can_skip_secondary_title_model_disclosure
run_case PS-008 s-code-audit signed_metadata_batch_verifies_without_persisting_event_content
run_case PS-009 s-code-storage database_files_are_private_and_symlinks_are_rejected
run_case PS-010 s-code-platform-runtime timeout_kills_the_command_process_group

printf '%s\n' 'privacy/security use cases: 10/10 PASS'
