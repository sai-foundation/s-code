ALTER TABLE tool_calls ADD COLUMN parent_tool_call_id TEXT REFERENCES tool_calls(id);
CREATE INDEX tool_calls_parent_idx ON tool_calls(parent_tool_call_id);
