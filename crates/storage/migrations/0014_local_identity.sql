CREATE TABLE local_identity (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    device_id TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL
);
