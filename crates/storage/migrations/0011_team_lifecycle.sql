ALTER TABLE team_ownership ADD COLUMN archived_at TEXT;
CREATE INDEX team_ownership_active_team
    ON team_ownership(organization_id, team_id, archived_at);
