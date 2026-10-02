-- Durable binding between an approved experience and the evaluation that
-- approved it. The approval event already named the evaluation, but an event
-- is published after the decision commits: a failure there left an approved,
-- retrievable lesson with nothing on record saying what approved it. The
-- binding is written in the same transaction as the status transition, so no
-- evidence-gated approval can exist without it.
ALTER TABLE experiences
ADD COLUMN approval_evaluation_id TEXT REFERENCES experience_evaluations(id);
