-- Forum hardening, step three. A comparison records two authorities: the
-- link authority that counts toward the supersession gate (`authoritative`)
-- and the parent authority whose safety evidence vetoes the parent's link
-- (`parent_authority`). Rows recorded before held the parent authority in
-- `authoritative`; it stays there as the recorded flag and is re-checked
-- live against the lineage whenever the gate runs.
ALTER TABLE comparisons ADD COLUMN parent_authority INTEGER NOT NULL DEFAULT 0 CHECK (parent_authority IN (0, 1));
UPDATE comparisons SET parent_authority = authoritative;
-- Links recorded under older rules, including those that still point at
-- a deprecated successor, are re-judged by the store when it opens, which
-- releases them with an event and reopens the challenges they addressed.
CREATE INDEX idx_skills_superseded_by ON skills (superseded_by);
CREATE INDEX idx_comparisons_principal ON comparisons (fork_id, principal_id);
