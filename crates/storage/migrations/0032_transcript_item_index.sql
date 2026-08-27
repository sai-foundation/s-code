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
            'model_reroute'
        )
    ),
    source_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE UNIQUE INDEX transcript_item_index_source
ON transcript_item_index(source_kind, source_id);

CREATE INDEX transcript_item_index_session_created
ON transcript_item_index(session_id, created_at DESC, item_id DESC);

INSERT OR IGNORE INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT id, session_id, turn_id, 'message', id, created_at
FROM messages
WHERE role <> 'system';

INSERT OR IGNORE INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT id, session_id, turn_id, 'tool_call', id, created_at
FROM tool_calls;

INSERT OR IGNORE INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT approvals.id, tool_calls.session_id, tool_calls.turn_id,
       'approval', approvals.id, approvals.requested_at
FROM approvals
INNER JOIN tool_calls ON tool_calls.id = approvals.tool_call_id;

INSERT OR IGNORE INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT item_id, session_id, turn_id, 'question', id, requested_at
FROM question_requests;

INSERT OR IGNORE INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT item_id, session_id, turn_id, 'artifact', id, created_at
FROM artifacts;

INSERT OR IGNORE INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT json_extract(payload_json, '$.item_id'), session_id, turn_id,
       'plan', json_extract(payload_json, '$.item_id'), MIN(created_at)
FROM audit_events
WHERE event_type = 'plan.updated'
  AND session_id IS NOT NULL
  AND turn_id IS NOT NULL
  AND json_type(payload_json, '$.item_id') = 'text'
  AND json_extract(payload_json, '$.item_id') <> ''
GROUP BY session_id, json_extract(payload_json, '$.item_id');

INSERT OR IGNORE INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT json_extract(payload_json, '$.item_id'), session_id, turn_id,
       'context_compaction', id, created_at
FROM audit_events
WHERE event_type = 'context.compacted'
  AND session_id IS NOT NULL
  AND turn_id IS NOT NULL
  AND json_type(payload_json, '$.item_id') = 'text'
  AND json_extract(payload_json, '$.item_id') <> '';

INSERT OR IGNORE INTO transcript_item_index
    (item_id, session_id, turn_id, source_kind, source_id, created_at)
SELECT id, session_id, turn_id, 'model_reroute', id, created_at
FROM audit_events
WHERE event_type = 'model.route.fallback'
  AND session_id IS NOT NULL
  AND turn_id IS NOT NULL;

CREATE TRIGGER transcript_item_index_message_insert
AFTER INSERT ON messages
WHEN NEW.role <> 'system'
BEGIN
    INSERT OR IGNORE INTO transcript_item_index
        (item_id, session_id, turn_id, source_kind, source_id, created_at)
    VALUES (NEW.id, NEW.session_id, NEW.turn_id, 'message', NEW.id, NEW.created_at);
END;

CREATE TRIGGER transcript_item_index_tool_call_insert
AFTER INSERT ON tool_calls
BEGIN
    INSERT OR IGNORE INTO transcript_item_index
        (item_id, session_id, turn_id, source_kind, source_id, created_at)
    VALUES (NEW.id, NEW.session_id, NEW.turn_id, 'tool_call', NEW.id, NEW.created_at);
END;

CREATE TRIGGER transcript_item_index_approval_insert
AFTER INSERT ON approvals
BEGIN
    INSERT OR IGNORE INTO transcript_item_index
        (item_id, session_id, turn_id, source_kind, source_id, created_at)
    SELECT NEW.id, tool_calls.session_id, tool_calls.turn_id,
           'approval', NEW.id, NEW.requested_at
    FROM tool_calls
    WHERE tool_calls.id = NEW.tool_call_id;
END;

CREATE TRIGGER transcript_item_index_question_insert
AFTER INSERT ON question_requests
BEGIN
    INSERT OR IGNORE INTO transcript_item_index
        (item_id, session_id, turn_id, source_kind, source_id, created_at)
    VALUES (
        NEW.item_id, NEW.session_id, NEW.turn_id,
        'question', NEW.id, NEW.requested_at
    );
END;

CREATE TRIGGER transcript_item_index_artifact_insert
AFTER INSERT ON artifacts
BEGIN
    INSERT OR IGNORE INTO transcript_item_index
        (item_id, session_id, turn_id, source_kind, source_id, created_at)
    VALUES (
        NEW.item_id, NEW.session_id, NEW.turn_id,
        'artifact', NEW.id, NEW.created_at
    );
END;

CREATE TRIGGER transcript_item_index_event_insert
AFTER INSERT ON audit_events
WHEN NEW.session_id IS NOT NULL
 AND NEW.turn_id IS NOT NULL
 AND (
    (
        NEW.event_type IN ('plan.updated', 'context.compacted')
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
            ELSE 'model_reroute'
        END,
        CASE
            WHEN NEW.event_type = 'plan.updated'
                THEN json_extract(NEW.payload_json, '$.item_id')
            ELSE NEW.id
        END,
        NEW.created_at
    );
END;
