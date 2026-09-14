ALTER TABLE sessions ADD COLUMN mode TEXT NOT NULL DEFAULT 'work' CHECK (mode IN ('chat', 'work'));
ALTER TABLE sessions ADD COLUMN work_reason TEXT;
