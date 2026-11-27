CREATE TABLE IF NOT EXISTS players (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source TEXT NOT NULL,
    source_player_id BIGINT NOT NULL,
    season_id TEXT NOT NULL REFERENCES seasons(id) ON DELETE CASCADE,
    club_id UUID REFERENCES clubs(id) ON DELETE SET NULL,
    canonical_name TEXT NOT NULL,
    jersey INTEGER,
    position TEXT,
    age INTEGER,
    nationality TEXT,
    nationality_flag_url TEXT,
    photo_url TEXT,
    source_url TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (source, source_player_id, season_id)
);

CREATE TABLE IF NOT EXISTS player_stats (
    player_id UUID NOT NULL REFERENCES players(id) ON DELETE CASCADE,
    season_id TEXT NOT NULL REFERENCES seasons(id) ON DELETE CASCADE,
    group_id TEXT REFERENCES groups(id) ON DELETE CASCADE,
    stat TEXT NOT NULL CHECK (stat IN ('goals', 'appearances', 'yellow_cards', 'red_cards')),
    value INTEGER NOT NULL DEFAULT 0 CHECK (value >= 0),
    rank INTEGER CHECK (rank IS NULL OR rank > 0),
    captured_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (player_id, season_id, stat)
);

ALTER TABLE ingest_runs ADD COLUMN IF NOT EXISTS stats_seen BIGINT NOT NULL DEFAULT 0;
ALTER TABLE ingest_runs ADD COLUMN IF NOT EXISTS players_upserted BIGINT NOT NULL DEFAULT 0;
ALTER TABLE ingest_runs ADD COLUMN IF NOT EXISTS player_stats_upserted BIGINT NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_players_club_season ON players(club_id, season_id);
CREATE INDEX IF NOT EXISTS idx_players_season_name ON players(season_id, canonical_name);
CREATE INDEX IF NOT EXISTS idx_player_stats_season_stat_value ON player_stats(season_id, stat, value DESC);
CREATE INDEX IF NOT EXISTS idx_player_stats_group_season_stat ON player_stats(group_id, season_id, stat);
