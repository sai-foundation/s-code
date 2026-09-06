# Behavior contracts

- `plan.validate` is a pure complete-graph check. Reject unknown keys, invalid
  values, duplicate task IDs, duplicate dependency entries, missing dependencies,
  and cycles before running any command. An empty task list is valid.
- IDs are unique nonempty strings. Command is a nonempty argv array of strings,
  with no NUL and a nonempty executable. `depends_on` is a list of task IDs.
  Resource, when present, is a nonempty string. Retries is an integer >= 0,
  excluding booleans. Timeout, when present, is positive and finite. Jobs is a
  positive integer. A diamond graph may share an ancestor across different tasks.
- `scheduler.run` starts eligible tasks in task-ID order, up to the job limit.
  Dependencies must succeed. Tasks sharing a resource cannot overlap, including
  retries. Independent work continues after failures. Transitive dependents of
  failed work are skipped. Validation and execution are separate phases.
- `process.execute` executes argv directly with no shell. Capture UTF-8 output
  (replace invalid bytes). Retries counts additional attempts. Report the final
  attempt's output and exit code. A timeout is a failed attempt with a nonzero
  exit code and a stderr explanation; it can be retried.
- `report.publish` atomically replaces the output only after execution. Preserve
  existing bytes on validation failure. Exit 0 for all success, 1 for a valid but
  failed workflow, and 2 for validation/usage errors (stderr, no traceback).
- Reports have `status` and `tasks` in original input order. Each task has `id`,
  `status`, `attempts`, `exit_code`, `stdout`, and `stderr`. Skipped tasks have
  zero attempts, null exit code, empty output, and a reason naming a failed
  dependency. No timestamps or durations. Successful empty plans have no tasks.
