-- SQLite rewrites trigger bodies when a referenced table is renamed. Migration 0035
-- rebuilt transcript_item_index, so recreate source-table triggers against its final name.
DROP TRIGGER transcript_item_index_message_insert;
DROP TRIGGER transcript_item_index_tool_call_insert;
DROP TRIGGER transcript_item_index_approval_insert;
DROP TRIGGER transcript_item_index_question_insert;
DROP TRIGGER transcript_item_index_artifact_insert;

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
