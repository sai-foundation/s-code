-- Authorized evaluators: a server-side capability that lets a principal's
-- receipts affect the status of skills outside its own team (public skills).
-- Receipts record whether they were authoritative when submitted. Receipts
-- recorded before this migration are kept as community receipts.
ALTER TABLE principals ADD COLUMN authorized_evaluator INTEGER NOT NULL DEFAULT 0;
ALTER TABLE receipts ADD COLUMN authoritative INTEGER NOT NULL DEFAULT 0;
