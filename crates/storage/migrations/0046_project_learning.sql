CREATE TABLE learning_projects (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('off', 'learn', 'reuse')),
    generation INTEGER NOT NULL CHECK (generation >= 0)
);

CREATE TABLE project_lessons (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES learning_projects(id),
    content_key TEXT NOT NULL,
    content_json TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1)),
    CHECK (revoked = 1 OR content_json IS NOT NULL)
);
CREATE INDEX project_lessons_lookup ON project_lessons(project_id, revoked, expires_at);
CREATE UNIQUE INDEX project_lessons_deduplicate
    ON project_lessons(project_id, content_key) WHERE revoked = 0;
