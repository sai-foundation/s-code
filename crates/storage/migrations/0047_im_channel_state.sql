CREATE TABLE im_channel_state (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    state_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (organization_id, team_id, actor_id, channel)
);
