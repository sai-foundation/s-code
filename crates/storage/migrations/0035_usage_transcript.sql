DROP TRIGGER transcript_item_index_event_insert;

ALTER TABLE transcript_item_index RENAME TO transcript_item_index_before_usage;

CREATE TABLE transcript_item_index (
    item_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT NOT NULL,
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'message',
            'tool_call',
            'approval',
            'question',
            'artifact',
            'plan',
            'context_compaction',
            'model_reroute',
            'usage',
            'agent_status',
            'hook'
        )
    ),
    source_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);

INSERT INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT item_id, session_id, turn_id, source_kind, source_id, created_at
FROM transcript_item_index_before_usage;

DROP TABLE transcript_item_index_before_usage;

CREATE UNIQUE INDEX transcript_item_index_source
ON transcript_item_index(source_kind, source_id);

CREATE INDEX transcript_item_index_session_created
ON transcript_item_index(session_id, created_at DESC, item_id DESC);

INSERT OR IGNORE INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT json_extract(payload_json, '$.item_id'), session_id, turn_id,
       'usage', id, created_at
FROM audit_events
WHERE event_type = 'turn.usage'
  AND session_id IS NOT NULL
  AND turn_id IS NOT NULL
  AND json_type(payload_json, '$.item_id') = 'text'
  AND json_extract(payload_json, '$.item_id') <> '';

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
        AND json_type(NEW.payload_json, '$.item_id') = 'text'
        AND json_extract(NEW.payload_json, '$.item_id') <> ''
    )
    OR NEW.event_type = 'model.route.fallback'
 )
BEGIN
    INSERT OR IGNORE INTO transcript_item_index
        (item_id, session_id, turn_id, source_kind, source_id, created_at)
    VALUES (
        CASE
            WHEN NEW.event_type = 'model.route.fallback' THEN NEW.id
            ELSE json_extract(NEW.payload_json, '$.item_id')
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
            )
                THEN json_extract(NEW.payload_json, '$.item_id')
            ELSE NEW.id
        END,
        NEW.created_at
    );
END;
