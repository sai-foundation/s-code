-- Verified experience memory: quarantined, actor-owned, project-scoped
-- lessons derived from execution evidence. Rows start as candidates and only
-- an explicit decision can approve or reject them. Lesson and evidence are
-- sealed like other sensitive payloads.
CREATE TABLE experiences (
    id TEXT PRIMARY KEY NOT NULL,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    workspace_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('candidate', 'approved', 'rejected')),
    lesson TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    source_session_id TEXT NOT NULL,
    source_turn_id TEXT NOT NULL,
    model TEXT NOT NULL,
    source_revision TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT,
    decided_at TEXT,
    decided_by TEXT,
    retrieved_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_experiences_retrieval
ON experiences (organization_id, team_id, actor_id, workspace_key, status, created_at DESC);
