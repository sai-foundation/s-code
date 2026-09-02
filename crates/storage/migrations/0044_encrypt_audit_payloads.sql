CREATE TABLE storage_encryption_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    key_id TEXT NOT NULL,
    verification TEXT NOT NULL
);

ALTER TABLE audit_events ADD COLUMN payload_item_id TEXT;
ALTER TABLE audit_events ADD COLUMN payload_tool_call_id TEXT;
ALTER TABLE audit_events ADD COLUMN usage_input_units INTEGER;
ALTER TABLE audit_events ADD COLUMN usage_output_units INTEGER;
ALTER TABLE audit_events ADD COLUMN usage_model_calls INTEGER;
ALTER TABLE audit_events ADD COLUMN usage_tool_calls INTEGER;

DROP TRIGGER audit_events_no_update;
DROP TRIGGER audit_events_no_delete;

UPDATE audit_events
SET payload_item_id = json_extract(payload_json, '$.item_id'),
    payload_tool_call_id = json_extract(payload_json, '$.tool_call_id'),
    usage_input_units = json_extract(payload_json, '$.input_units'),
    usage_output_units = json_extract(payload_json, '$.output_units'),
    usage_model_calls = json_extract(payload_json, '$.model_calls'),
    usage_tool_calls = json_extract(payload_json, '$.tool_calls')
WHERE json_valid(payload_json);

CREATE TRIGGER audit_events_no_update
BEFORE UPDATE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit events are append-only');
END;

CREATE TRIGGER audit_events_no_delete
BEFORE DELETE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit events are append-only');
END;

DROP TRIGGER transcript_item_index_event_insert;

CREATE TRIGGER transcript_item_index_event_insert
AFTER INSERT ON audit_events
WHEN NEW.session_id IS NOT NULL
 AND NEW.turn_id IS NOT NULL
 AND (
    (
        NEW.event_type IN (
            'plan.updated',
            'context.compacted',
            'turn.usage',
            'agent.status',
            'hook.started',
            'hook.completed',
            'hook.failed'
        )
        AND NEW.payload_item_id IS NOT NULL
        AND NEW.payload_item_id <> ''
    )
    OR NEW.event_type = 'model.route.fallback'
 )
BEGIN
    INSERT OR IGNORE INTO transcript_item_index
        (item_id, session_id, turn_id, source_kind, source_id, created_at)
    VALUES (
        CASE
            WHEN NEW.event_type = 'model.route.fallback' THEN NEW.id
            ELSE NEW.payload_item_id
        END,
        NEW.session_id,
        NEW.turn_id,
        CASE NEW.event_type
            WHEN 'plan.updated' THEN 'plan'
            WHEN 'context.compacted' THEN 'context_compaction'
            WHEN 'turn.usage' THEN 'usage'
            WHEN 'agent.status' THEN 'agent_status'
            WHEN 'hook.started' THEN 'hook'
            WHEN 'hook.completed' THEN 'hook'
            WHEN 'hook.failed' THEN 'hook'
            ELSE 'model_reroute'
        END,
        CASE
            WHEN NEW.event_type IN (
                'plan.updated',
                'agent.status',
                'hook.started',
                'hook.completed',
                'hook.failed'
            ) THEN NEW.payload_item_id
            ELSE NEW.id
        END,
        NEW.created_at
    );
END;
