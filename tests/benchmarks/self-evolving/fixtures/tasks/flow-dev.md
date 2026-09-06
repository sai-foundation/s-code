Add `--fail-fast` to `flow_runner`. After a task exhausts its retries, start no
new tasks. Already-running tasks may finish. Report tasks that were never
started as `skipped` with zero attempts and a concise fail-fast reason,
preserving input order. Without the flag, retain existing behavior. Add
regression tests in new files; do not change or remove existing tests or their
execution/discovery configuration. Normal project documentation updates are
allowed. Do not suppress original test execution or write outside the candidate
workspace.
