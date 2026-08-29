CREATE TABLE daemon_settings (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    settings_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
