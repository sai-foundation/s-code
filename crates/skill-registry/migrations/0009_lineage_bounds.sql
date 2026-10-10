-- Forum hardening, step two. Every skill records the root of its lineage
-- tree (itself for a root) and its depth below that root, so the depth of a
-- fork, the forks of one skill and the size of one tree are bounded with
-- index lookups, and a whole tree is read with one query.
ALTER TABLE skills ADD COLUMN lineage_root_id TEXT REFERENCES skills(id);
ALTER TABLE skills ADD COLUMN lineage_depth INTEGER NOT NULL DEFAULT 0 CHECK (lineage_depth >= 0);
WITH RECURSIVE chain(id, root, depth) AS (
    SELECT id, id, 0 FROM skills WHERE parent_skill_id IS NULL
    UNION ALL
    SELECT skills.id, chain.root, chain.depth + 1
    FROM skills JOIN chain ON skills.parent_skill_id = chain.id
)
UPDATE skills SET
    lineage_root_id = (SELECT root FROM chain WHERE chain.id = skills.id),
    lineage_depth = COALESCE((SELECT depth FROM chain WHERE chain.id = skills.id), 0);
CREATE INDEX idx_skills_lineage_root ON skills (lineage_root_id, lineage_depth, created_at);
