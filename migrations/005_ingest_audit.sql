CREATE TABLE IF NOT EXISTS ingest_runs (
    id UUID PRIMARY KEY,
    mode TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'all',
    status TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed')),
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    duration_ms BIGINT,
    matches_seen BIGINT NOT NULL DEFAULT 0,
    matches_upserted BIGINT NOT NULL DEFAULT 0,
    events_upserted BIGINT NOT NULL DEFAULT 0,
    standings_rebuilt BIGINT NOT NULL DEFAULT 0,
    error TEXT
);

CREATE INDEX IF NOT EXISTS idx_ingest_runs_started_at ON ingest_runs(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_ingest_runs_mode_status ON ingest_runs(mode, status, started_at DESC);
