-- Immutable population evaluation receipts for one shared skill. The daemon
-- recomputes every verdict from the submitted counts; the sealed result keeps
-- the full submission for audit. One receipt per evaluator and protocol.
CREATE TABLE skill_evaluations (
    id TEXT PRIMARY KEY NOT NULL,
    skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    evaluator_json TEXT NOT NULL,
    independent INTEGER NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('direct', 'imported')),
    protocol_version INTEGER NOT NULL,
    protocol_digest TEXT NOT NULL,
    complete INTEGER NOT NULL,
    safety TEXT NOT NULL CHECK (safety IN ('clean', 'failed', 'incomplete')),
    verdict_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (skill_id, actor_id, protocol_digest)
);

CREATE INDEX idx_skill_evaluations_lookup
ON skill_evaluations (skill_id, created_at DESC);
