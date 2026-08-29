ALTER TABLE tool_calls ADD COLUMN simulated_policy_json TEXT;
ALTER TABLE tool_calls ADD COLUMN central_policy_applied INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tool_calls ADD COLUMN team_configuration_sequence INTEGER;
