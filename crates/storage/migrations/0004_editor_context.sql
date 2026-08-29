CREATE TABLE editor_contexts (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    client_instance_id TEXT NOT NULL,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    context_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(session_id, client_instance_id)
);
CREATE INDEX editor_contexts_session_updated ON editor_contexts(session_id, updated_at DESC);
