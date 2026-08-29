CREATE TABLE background_terminals (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT NOT NULL REFERENCES turns(id),
    spec_json TEXT NOT NULL,
    permission_digest TEXT NOT NULL,
    program TEXT NOT NULL,
    argument_count INTEGER NOT NULL,
    working_directory_uri TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('starting','running','exited','failed','stopped','orphaned')),
    rows INTEGER NOT NULL,
    cols INTEGER NOT NULL,
    max_runtime_seconds INTEGER NOT NULL,
    output_base64 TEXT NOT NULL,
    output_byte_length INTEGER NOT NULL DEFAULT 0,
    output_truncated INTEGER NOT NULL DEFAULT 0,
    exit_code INTEGER,
    artifact_id TEXT,
    failure TEXT,
    created_at TEXT NOT NULL,
    started_at TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    revision INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX background_terminals_scope_updated
    ON background_terminals(organization_id, team_id, actor_id, updated_at DESC);
CREATE INDEX background_terminals_session_updated
    ON background_terminals(session_id, updated_at DESC);
