-- Internal context checkpoints share the messages table for ordering and
-- encryption, but they are not user-visible Transcript items. Keep the index
-- allowlisted so future internal roles fail closed instead of breaking paged
-- projection consistency.
DELETE FROM transcript_item_index
WHERE source_kind = 'message'
  AND source_id IN (
      SELECT id
      FROM messages
      WHERE role NOT IN (
          'user',
          'assistant',
          'tool',
          'reasoning_summary_streaming',
          'reasoning_summary'
      )
  );

DROP TRIGGER transcript_item_index_message_insert;

CREATE TRIGGER transcript_item_index_message_insert
AFTER INSERT ON messages
WHEN NEW.role IN (
    'user',
    'assistant',
    'tool',
    'reasoning_summary_streaming',
    'reasoning_summary'
)
BEGIN
    INSERT OR IGNORE INTO transcript_item_index
        (item_id, session_id, turn_id, source_kind, source_id, created_at)
    VALUES (NEW.id, NEW.session_id, NEW.turn_id, 'message', NEW.id, NEW.created_at);
END;
