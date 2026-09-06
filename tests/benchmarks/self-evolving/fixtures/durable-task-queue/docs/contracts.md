# Behavior contracts

- Schema initialization is idempotent and uses SQLite WAL. `store.transaction`
  wraps one operation in `BEGIN IMMEDIATE` with commit or rollback. Failed
  validation and ownership errors must leave logical rows unchanged.
- Task IDs and workers are nonempty. Priority is an integer (default 0), and
  max attempts is a positive integer (default 3). Enqueue accepts JSON values,
  not only objects. Object key order is irrelevant to idempotency; conflicting
  payload, priority, or max attempts returns 2 without changing the old task.
- Time-aware operations accept `--now`, a finite Unix timestamp override. Finite
  zero, negative, and fractional timestamps are valid. Otherwise use current
  Unix time. Lease duration must be positive and finite. Validate numeric options
  before opening/creating a database. `clock` centralizes numeric parsing.
- `claim` and `stats` expire leases with expiry <= now. Expiry returns a task to
  queued or marks it dead when no attempts remain. Each claim uses a new token.
  An ack matches the currently stored owner/token; it has no time argument.
  Fail first checks time and must reject expired or stale leases. Transactions
  roll back all expiry effects if a fail operation is rejected.
- Enqueue returns `{created, task_id}`. Claims return `{task_id, payload,
  attempt, lease_owner, lease_token, lease_expires_at}` or null. Ack/fail returns
  `{status, task_id}`. Stats includes all four states and total. Every success
  emits one compact JSON line; user errors return 2, stderr only, no traceback.
- Completed/dead records persist and cannot be claimed. Payload and attempt
  counts survive all ownership changes.
