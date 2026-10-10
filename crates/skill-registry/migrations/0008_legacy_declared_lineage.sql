-- Forum hardening, step one. Before the forum, a publication could name a
-- parent and a version. Those were claims, not forks: they bypassed the fork
-- rules (the parent must be visible, the content must differ, visibility is
-- derived). Keep each claim for the record, but only skills created through
-- the fork endpoint (forked_by_id set) take part in lineage, comparisons and
-- supersession, and no supersession may point at a claimed child.
ALTER TABLE skills ADD COLUMN legacy_declared_parent_id TEXT;
UPDATE skills
    SET legacy_declared_parent_id = parent_skill_id, parent_skill_id = NULL
    WHERE forked_by_id IS NULL AND parent_skill_id IS NOT NULL;
UPDATE skills
    SET superseded_by = NULL, superseded_at = NULL
    WHERE superseded_by IN (SELECT id FROM skills WHERE forked_by_id IS NULL);
