#!/usr/bin/env sh
# Show the self-evolution lifecycle: one agent's correction becoming a Skill
# another agent reuses, and a poisoned candidate that never leaves quarantine.
#
#   scripts/demo-self-evolve.sh            deterministic replay, no provider
#   scripts/demo-self-evolve.sh --live     against a running local daemon
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mode=replay
[ "${1:-}" = "--live" ] && mode=live
[ "${1:-}" = "--replay" ] && mode=replay
exec python3 "$root/tools/demo/self_evolve_demo.py" --mode "$mode"
