# Durable task queue

Build a dependency-free Python CLI backed by SQLite. The public command is
`python3 -m durable_queue` and must support:

```text
init DB
enqueue DB TASK_ID PAYLOAD_JSON [--priority N] [--max-attempts N] [--now SECONDS]
claim DB WORKER --lease-seconds N [--now SECONDS]
ack DB TASK_ID WORKER LEASE_TOKEN
fail DB TASK_ID WORKER LEASE_TOKEN --error MESSAGE [--now SECONDS]
stats DB [--now SECONDS]
```

Requirements:

- Initialize the schema idempotently and enable SQLite WAL mode.
- Enqueue JSON values, not just objects. A repeated task id with equivalent
  payload and options is an idempotent success with `created: false`; conflicting
  reuse is an error and must not alter the task.
- Claim exactly one eligible task atomically across concurrent processes. Higher
  priority wins, then lower enqueue sequence. A claim returns the decoded payload,
  a one-based attempt number, lease owner, opaque lease token, and expiry.
- Expired leases become eligible again. An old owner or token can never ack or
  fail a renewed lease.
- `fail` requeues while attempts remain and otherwise moves the task to `dead`.
  Completed and dead tasks are never claimed.
- `stats` atomically expires stale leases first and reports `queued`, `leased`,
  `completed`, `dead`, and `total` counts.
- `--now` is a deterministic finite timestamp override; otherwise use current
  Unix time. Lease durations, priorities, and max attempts must be validated.
- Every successful command prints one compact JSON value plus a newline. An empty
  claim prints `null`. User errors print a concise message to stderr, return 2,
  and never print a traceback.

Keep storage logic separate from argument parsing. Do not change the tests. Run
`python3 -m unittest discover -s tests -v` until every test passes.
