-- Keep privacy pagination independent of the size of the transcript/event log.
CREATE INDEX audit_privacy_starts ON audit_events
    (organization_id, team_id, actor_id, session_id, sequence DESC)
    WHERE event_type='privacy.request.started';
CREATE INDEX audit_privacy_outcomes ON audit_events
    (session_id, payload_item_id, sequence DESC)
    WHERE event_type IN ('privacy.request.started', 'privacy.request.finished');
