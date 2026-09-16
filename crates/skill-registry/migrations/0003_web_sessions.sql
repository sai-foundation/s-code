-- Web sessions: the browsable shop authenticates a browser through an
-- opaque, random, server-side session created by one explicit login with a
-- registry token. The token itself never enters a cookie, and the session
-- id is stored only as its SHA-256 digest. Sessions are bounded, revocable,
-- and stop working as soon as their principal is disabled.
CREATE TABLE web_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    principal_id TEXT NOT NULL REFERENCES principals(id),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_web_sessions_principal ON web_sessions (principal_id);
