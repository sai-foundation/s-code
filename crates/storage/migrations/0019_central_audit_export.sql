CREATE TABLE central_audit_export_cursors (
    source_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    source_sequence INTEGER NOT NULL CHECK(source_sequence >= 0),
    local_sequence INTEGER NOT NULL CHECK(local_sequence >= 0),
    chain_head TEXT NOT NULL CHECK(length(chain_head) = 64),
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX central_audit_export_scope
    ON central_audit_export_cursors(organization_id, team_id, source_id);

CREATE TABLE central_audit_export_pending (
    source_id TEXT PRIMARY KEY REFERENCES central_audit_export_cursors(source_id),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL CHECK(length(payload_sha256) = 64),
    envelope_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
