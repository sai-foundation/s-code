CREATE TABLE turn_file_changes_0020_backup AS
SELECT * FROM turn_file_changes;

DROP TABLE turn_file_changes;

CREATE TABLE turns_0020 (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('idle','preparing_context','calling_model','awaiting_input','awaiting_approval','running_tool','completed','failed','cancelled')),
    checkpoint_json TEXT,
    error_code TEXT,
    started_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT
);

INSERT INTO turns_0020
SELECT id, session_id, organization_id, team_id, actor_id, goal_id, task_id,
       status, checkpoint_json, error_code, started_at, updated_at, completed_at
FROM turns;

DROP TABLE turns;
ALTER TABLE turns_0020 RENAME TO turns;

CREATE INDEX turns_session_started ON turns(session_id, started_at);
CREATE INDEX turns_team_status ON turns(team_id, status, updated_at);

CREATE TABLE turn_file_changes (
    turn_id TEXT NOT NULL REFERENCES turns(id),
    session_id TEXT NOT NULL REFERENCES sessions(id),
    path TEXT NOT NULL,
    before_exists INTEGER NOT NULL CHECK (before_exists IN (0, 1)),
    before_content BLOB,
    before_sha256 TEXT,
    after_sha256 TEXT NOT NULL,
    previous_after_sha256 TEXT,
    state TEXT NOT NULL CHECK (state IN ('planned', 'applied', 'undone')),
    operation_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    undone_at TEXT,
    PRIMARY KEY (turn_id, path),
    CHECK ((before_exists = 1 AND before_content IS NOT NULL AND before_sha256 IS NOT NULL)
        OR (before_exists = 0 AND before_content IS NULL AND before_sha256 IS NULL))
);

INSERT INTO turn_file_changes
SELECT turn_id, session_id, path, before_exists, before_content, before_sha256,
       after_sha256, previous_after_sha256, state, operation_id, created_at,
       updated_at, undone_at
FROM turn_file_changes_0020_backup;

DROP TABLE turn_file_changes_0020_backup;
CREATE INDEX turn_file_changes_session_turn
ON turn_file_changes(session_id, turn_id, state);

CREATE TABLE question_requests (
    id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL UNIQUE,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT NOT NULL REFERENCES turns(id),
    questions_json TEXT NOT NULL,
    allow_other INTEGER NOT NULL CHECK (allow_other IN (0, 1)),
    status TEXT NOT NULL CHECK (status IN ('pending','answered','expired','cancelled')),
    answers_json TEXT,
    requested_at TEXT NOT NULL,
    expires_at TEXT,
    answered_at TEXT,
    answered_by TEXT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0)
);

CREATE INDEX question_requests_session_requested
ON question_requests(session_id, requested_at);
CREATE INDEX question_requests_team_status
ON question_requests(team_id, status, requested_at);
