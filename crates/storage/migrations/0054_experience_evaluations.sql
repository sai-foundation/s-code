-- Immutable evaluation evidence for one experience candidate. The daemon
-- recomputes the promotion gates from the submitted counts and stores its own
-- verdict; the sealed result keeps the full submission for audit. Rows are
-- never updated, and one protocol digest per experience prevents silent
-- overwrites.
CREATE TABLE experience_evaluations (
    id TEXT PRIMARY KEY NOT NULL,
    experience_id TEXT NOT NULL REFERENCES experiences(id) ON DELETE CASCADE,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    protocol_version INTEGER NOT NULL,
    protocol_digest TEXT NOT NULL,
    eligible INTEGER NOT NULL,
    verdict_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (experience_id, protocol_digest)
);

CREATE INDEX idx_experience_evaluations_lookup
ON experience_evaluations (experience_id, eligible, created_at DESC);
