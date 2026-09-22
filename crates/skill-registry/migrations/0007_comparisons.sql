-- Forum, step three: comparative receipts. An evaluator runs the parent and
-- a fork on matched held-out tasks and reports raw counts for both arms; the
-- registry computes the comparison and applies the deterministic
-- supersession gate in the same transaction. A client never submits a
-- winner, a score or a supersession.
CREATE TABLE comparisons (
    id TEXT PRIMARY KEY NOT NULL,
    fork_id TEXT NOT NULL REFERENCES skills(id),
    parent_id TEXT NOT NULL REFERENCES skills(id),
    principal_id TEXT NOT NULL REFERENCES principals(id),
    principal_display TEXT NOT NULL,
    evaluator_json TEXT NOT NULL,
    independent INTEGER NOT NULL,
    authoritative INTEGER NOT NULL,
    protocol_version INTEGER NOT NULL,
    protocol_digest TEXT NOT NULL,
    complete INTEGER NOT NULL,
    fork_safety TEXT NOT NULL CHECK (fork_safety IN ('clean', 'failed', 'incomplete')),
    task_family TEXT,
    model TEXT,
    verdict_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (fork_id, principal_id, protocol_digest)
);

CREATE INDEX idx_comparisons_fork ON comparisons (fork_id, created_at ASC);
