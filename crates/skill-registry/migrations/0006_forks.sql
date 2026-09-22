-- Forum, step two: immutable forks and lineage. A fork is a new candidate
-- skill whose parent, version, scope and visibility the registry derives;
-- it never edits its parent. Supersession columns are added here so the
-- artifact shape is complete; they are set only by the comparative gate.
ALTER TABLE skills ADD COLUMN forked_by_id TEXT REFERENCES principals(id);
ALTER TABLE skills ADD COLUMN forked_by_display TEXT;
ALTER TABLE skills ADD COLUMN responding_to_challenge_id TEXT REFERENCES challenges(id);
ALTER TABLE skills ADD COLUMN superseded_by TEXT REFERENCES skills(id);
ALTER TABLE skills ADD COLUMN superseded_at TEXT;

CREATE INDEX idx_skills_parent ON skills (parent_skill_id, created_at ASC);
