CREATE TABLE turn_inputs (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    target_turn_id TEXT NOT NULL REFERENCES turns(id),
    resulting_turn_id TEXT REFERENCES turns(id),
    mode TEXT NOT NULL CHECK (mode IN ('steer','queue')),
    content_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending','processing','consumed','cancelled')),
    idempotency_key TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    consumed_at TEXT,
    cancelled_at TEXT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    UNIQUE (organization_id, team_id, idempotency_key)
);

CREATE UNIQUE INDEX turn_inputs_one_active_steer
ON turn_inputs(target_turn_id)
WHERE mode = 'steer' AND status IN ('pending','processing');

CREATE INDEX turn_inputs_session_status_created
ON turn_inputs(session_id, status, created_at);

CREATE INDEX turn_inputs_team_status_created
ON turn_inputs(team_id, status, created_at);
