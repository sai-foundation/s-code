-- Shared skill shop: sanitized, bounded lessons published explicitly from an
-- approved and evaluated local experience and shared within one
-- organization/team. The publisher stays recorded as the row's actor. Lesson
-- and applicability are sealed like other sensitive payloads and never
-- change after publication; only the daemon-computed status moves.
CREATE TABLE skills (
    id TEXT PRIMARY KEY NOT NULL,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    source_experience_id TEXT NOT NULL,
    lesson TEXT NOT NULL,
    applicability TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    sanitization_version INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('candidate', 'verified', 'deprecated')),
    deprecation_reason TEXT,
    parent_skill_id TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    verified_at TEXT,
    deprecated_at TEXT,
    retrieved_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE (organization_id, team_id, source_experience_id)
);

CREATE INDEX idx_skills_shop
ON skills (organization_id, team_id, status, created_at DESC);
