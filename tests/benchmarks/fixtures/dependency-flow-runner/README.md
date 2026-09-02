# Dependency flow runner

Build a dependency-free Python workflow executor exposed as:

```sh
python3 -m flow_runner PLAN.json --jobs N --output REPORT.json
```

The plan is a JSON object with a `tasks` array. Every task has a unique non-empty
`id`, an argv-array `command`, and optional `depends_on`, `resource`, `retries`,
and positive `timeout_seconds` fields.

Requirements:

- Validate the complete graph before running anything: reject unknown keys,
  duplicate or missing dependencies, cycles, invalid commands and values.
- Run ready tasks concurrently up to `--jobs`. Dependencies must succeed first.
  Among eligible tasks, schedule by task id for deterministic behavior.
- Tasks sharing the same non-empty resource must never overlap. Other resources
  may run concurrently.
- Execute argv directly without a shell, capture UTF-8 stdout/stderr, and enforce
  per-attempt timeout. `retries: 2` permits three attempts total.
- A task that exhausts attempts is `failed`; every transitive dependent becomes
  `skipped`, while independent branches continue.
- Atomically replace the report after execution. Invalid plans leave an existing
  report untouched. Exit 0 when all tasks succeed, 1 for executed workflow
  failure, and 2 for validation/usage errors without a traceback.
- The report contains overall `status` and `tasks` in original plan order. Each
  task records `id`, `status`, `attempts`, `exit_code`, `stdout`, and `stderr`.
  Skipped tasks use zero attempts, null exit code, empty output, and a concise
  reason naming a failed dependency. Do not include timestamps or durations.

Separate validation, scheduling, and process execution. Do not change the tests.
Run `python3 -m unittest discover -s tests -v` until every test passes.
