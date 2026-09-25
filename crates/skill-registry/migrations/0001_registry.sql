-- Remote skill registry: server-authenticated principals, shared skills
-- with team or public visibility, immutable population receipts and a
-- bounded audit log. Tokens are stored only as SHA-256 digests. Status moves
-- only through the deterministic gate or an explicit deprecation.
CREATE TABLE principals (
    id TEXT PRIMARY KEY NOT NULL,
    display_name TEXT NOT NULL,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    disabled INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE skills (
    id TEXT PRIMARY KEY NOT NULL,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    publisher_id TEXT NOT NULL REFERENCES principals(id),
    publisher_display TEXT NOT NULL,
    visibility TEXT NOT NULL CHECK (visibility IN ('team', 'public')),
    lesson TEXT NOT NULL,
    applicability TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    sanitization_version INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('candidate', 'verified', 'deprecated')),
    deprecation_reason TEXT,
    parent_skill_id TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    provenance_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    verified_at TEXT,
    deprecated_at TEXT,
    UNIQUE (organization_id, team_id, content_digest)
);

CREATE INDEX idx_skills_visibility
ON skills (visibility, status, created_at DESC);

CREATE INDEX idx_skills_team
ON skills (organization_id, team_id, status, created_at DESC);

CREATE TABLE receipts (
    id TEXT PRIMARY KEY NOT NULL,
    skill_id TEXT NOT NULL REFERENCES skills(id),
    principal_id TEXT NOT NULL REFERENCES principals(id),
    principal_display TEXT NOT NULL,
    evaluator_json TEXT NOT NULL,
    independent INTEGER NOT NULL,
    protocol_version INTEGER NOT NULL,
    protocol_digest TEXT NOT NULL,
    complete INTEGER NOT NULL,
    safety TEXT NOT NULL CHECK (safety IN ('clean', 'failed', 'incomplete')),
    task_family TEXT,
    model TEXT,
    verdict_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (skill_id, principal_id, protocol_digest)
);

CREATE INDEX idx_receipts_skill
ON receipts (skill_id, created_at ASC);

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    skill_id TEXT,
    principal_id TEXT,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
