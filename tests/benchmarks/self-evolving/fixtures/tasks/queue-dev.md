Add `purge-completed DB`. Remove completed records only, in one transaction, and
print `{"removed":N}`. Repeated execution is an idempotent success with zero
removed. Do not expire leases or modify queued, leased, or dead tasks, including
their payloads, attempts, and lease tokens. Preserve all existing CLI behavior.
Add regression tests in new files; do not change or remove existing tests or
their execution/discovery configuration. Normal project documentation updates
are allowed. Do not suppress original test execution or write outside the
candidate workspace.
