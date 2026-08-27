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
CREATE INDEX turn_file_changes_session_turn ON turn_file_changes(session_id, turn_id, state);
