ALTER TABLE matches ADD COLUMN IF NOT EXISTS last_polled_at TIMESTAMPTZ;
ALTER TABLE matches ADD COLUMN IF NOT EXISTS source_updated_at TIMESTAMPTZ;

UPDATE matches SET last_polled_at = updated_at WHERE last_polled_at IS NULL;

ALTER TABLE matches ALTER COLUMN last_polled_at SET DEFAULT now();
ALTER TABLE matches ALTER COLUMN last_polled_at SET NOT NULL;

CREATE INDEX IF NOT EXISTS idx_matches_last_polled ON matches(last_polled_at DESC);
