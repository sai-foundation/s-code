#!/usr/bin/env sh
# Show the self-evolution lifecycle: one agent's correction becoming a Skill
# another agent reuses, and a poisoned candidate that never leaves quarantine.
#
#   scripts/demo-self-evolve.sh                     deterministic replay, no provider
#   scripts/demo-self-evolve.sh --live [options]    against a running local daemon
#
# Live mode reads the product's own scoped endpoints, so it needs the scope its
# state belongs to, either as options or in the environment:
#
#   scripts/demo-self-evolve.sh --live --organization ORG --team TEAM --actor ACTOR
#   scripts/demo-self-evolve.sh --live --registry URL --registry-token TOKEN
#
# Nothing in live mode is simulated: a stage that cannot be shown stops the demo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mode=replay
if [ "${1:-}" = "--live" ]; then
  mode=live
  shift
elif [ "${1:-}" = "--replay" ]; then
  mode=replay
  shift
fi
exec python3 "$root/tools/demo/self_evolve_demo.py" --mode "$mode" "$@"
