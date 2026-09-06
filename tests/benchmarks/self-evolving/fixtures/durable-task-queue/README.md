# Durable task queue

A small Python 3.10+ SQLite work queue. Only the standard library is required.

```text
python3 -m durable_queue init DB
python3 -m durable_queue enqueue DB TASK_ID PAYLOAD_JSON [--priority N] [--max-attempts N] [--now SECONDS]
python3 -m durable_queue claim DB WORKER --lease-seconds N [--now SECONDS]
python3 -m durable_queue ack DB TASK_ID WORKER LEASE_TOKEN
python3 -m durable_queue fail DB TASK_ID WORKER LEASE_TOKEN --error MESSAGE [--now SECONDS]
python3 -m durable_queue stats DB [--now SECONDS]
python3 -m unittest discover -s tests -v
```

Enqueue JSON payloads; claim one at a time by descending priority, then insertion
order. A claim creates an opaque ownership token and consumes one attempt.
Failed or expired work can be retried until its limit. See
[behavior contracts](docs/contracts.md). Add regression files under `tests/`;
do not change the existing tests.
