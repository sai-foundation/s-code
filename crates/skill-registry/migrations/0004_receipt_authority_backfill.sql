-- Receipts recorded before 0002 carry authoritative = 0. Authority is now
-- evaluated as "counted when submitted AND the evaluator still qualifies",
-- so restore the recorded flag for receipts whose evaluator was a member of
-- the skill's team when they were filed; outsiders' receipts stay community
-- receipts. Web session ids (0003) are stored only as SHA-256 digests.
UPDATE receipts SET authoritative = 1
WHERE authoritative = 0
  AND EXISTS (
    SELECT 1 FROM principals p JOIN skills s ON s.id = receipts.skill_id
    WHERE p.id = receipts.principal_id
      AND p.organization_id = s.organization_id
      AND p.team_id = s.team_id
  );
