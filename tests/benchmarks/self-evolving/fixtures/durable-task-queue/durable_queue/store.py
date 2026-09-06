"""All row mutations execute inside a single immediate SQLite transaction."""
from contextlib import contextmanager
import json
import secrets
import sqlite3

SCHEMA = """
CREATE TABLE IF NOT EXISTS tasks (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL UNIQUE,
    payload TEXT NOT NULL,
    priority INTEGER NOT NULL,
    max_attempts INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'queued',
    created_at REAL NOT NULL,
    lease_owner TEXT,
    lease_token TEXT,
    lease_expires_at REAL,
    last_error TEXT
)
"""


@contextmanager
def transaction(path):
    connection = sqlite3.connect(path, timeout=15, isolation_level=None)
    connection.row_factory = sqlite3.Row
    try:
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute("BEGIN IMMEDIATE")
        connection.execute(SCHEMA)
        yield connection
        connection.commit()
    except BaseException:
        connection.rollback()
        raise
    finally:
        connection.close()


def enqueue(connection, task_id, payload, priority, max_attempts, now):
    serialized = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False)
    current = connection.execute("SELECT * FROM tasks WHERE task_id=?", (task_id,)).fetchone()
    if current is not None:
        if (current["payload"], current["priority"], current["max_attempts"]) != (serialized, priority, max_attempts):
            raise ValueError(f"task {task_id!r} already exists with conflicting data")
        return {"created": False, "task_id": task_id}
    connection.execute("INSERT INTO tasks(task_id,payload,priority,max_attempts,created_at) VALUES(?,?,?,?,?)", (task_id, serialized, priority, max_attempts, now))
    return {"created": True, "task_id": task_id}


def expire_leases(connection, now):
    connection.execute("""UPDATE tasks SET status=CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'queued' END,
        lease_owner=NULL, lease_token=NULL, lease_expires_at=NULL
        WHERE status='leased' AND lease_expires_at <= ?""", (now,))


def claim(connection, worker, lease_seconds, now):
    expire_leases(connection, now)
    row = connection.execute("SELECT * FROM tasks WHERE status='queued' AND attempts < max_attempts ORDER BY priority DESC, sequence LIMIT 1").fetchone()
    if row is None:
        return None
    token, expiry, attempt = secrets.token_hex(16), now + lease_seconds, row["attempts"] + 1
    connection.execute("UPDATE tasks SET status='leased', attempts=?, lease_owner=?, lease_token=?, lease_expires_at=? WHERE task_id=?", (attempt, worker, token, expiry, row["task_id"]))
    return {"task_id": row["task_id"], "payload": json.loads(row["payload"]), "attempt": attempt, "lease_owner": worker, "lease_token": token, "lease_expires_at": expiry}


def require_lease(connection, task_id, worker, token):
    row = connection.execute("SELECT * FROM tasks WHERE task_id=?", (task_id,)).fetchone()
    if row is None or row["status"] != "leased" or row["lease_owner"] != worker or row["lease_token"] != token:
        raise ValueError(f"task {task_id!r} has no matching lease")
    return row


def ack(connection, task_id, worker, token):
    require_lease(connection, task_id, worker, token)
    connection.execute("UPDATE tasks SET status='completed', lease_owner=NULL, lease_token=NULL, lease_expires_at=NULL WHERE task_id=?", (task_id,))
    return {"status": "completed", "task_id": task_id}


def fail(connection, task_id, worker, token, error, now):
    expire_leases(connection, now)
    row = require_lease(connection, task_id, worker, token)
    status = "dead" if row["attempts"] >= row["max_attempts"] else "queued"
    connection.execute("UPDATE tasks SET status=?, lease_owner=NULL, lease_token=NULL, lease_expires_at=NULL, last_error=? WHERE task_id=?", (status, error, task_id))
    return {"status": status, "task_id": task_id}


def stats(connection, now):
    expire_leases(connection, now)
    counts = dict.fromkeys(("queued", "leased", "completed", "dead"), 0)
    counts.update(dict(connection.execute("SELECT status,COUNT(*) FROM tasks GROUP BY status")))
    counts["total"] = sum(counts.values())
    return counts
