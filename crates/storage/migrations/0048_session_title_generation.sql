-- Naming provenance must commit with the title, independently of event delivery.
ALTER TABLE sessions ADD COLUMN title_generation TEXT NOT NULL DEFAULT 'unset'
    CHECK (title_generation IN ('unset', 'pending', 'complete', 'manual'));
