-- Forum, step one: immutable, evidence-backed challenges against an exact
-- skill version. A challenge is bounded, sanitized, derived-untrusted text
-- plus an optional reference to a receipt on the same skill. Text alone never
-- changes a skill's status; only the referenced receipt, through the
-- ordinary gate, can.
CREATE TABLE challenges (
    id TEXT PRIMARY KEY NOT NULL,
    skill_id TEXT NOT NULL REFERENCES skills(id),
    content_digest TEXT NOT NULL,
    skill_version INTEGER NOT NULL,
    challenger_id TEXT NOT NULL REFERENCES principals(id),
    challenger_display TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('applicability_failure', 'negative_transfer', 'safety_concern', 'correctness_failure', 'generalization_failure')),
    claim TEXT NOT NULL,
    applicability TEXT,
    evidence_receipt_id TEXT REFERENCES receipts(id),
    claim_digest TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'addressed')),
    created_at TEXT NOT NULL,
    addressed_at TEXT,
    addressed_by_skill_id TEXT,
    UNIQUE (skill_id, challenger_id, claim_digest)
);

CREATE INDEX idx_challenges_skill ON challenges (skill_id, created_at DESC);
