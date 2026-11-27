-- Initial schema for the Algerian matches API
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE IF NOT EXISTS accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    plan TEXT NOT NULL DEFAULT 'free',
    quota BIGINT NOT NULL DEFAULT 10000,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    key_prefix TEXT NOT NULL UNIQUE,
    key_hash TEXT NOT NULL UNIQUE,
    scopes TEXT[] NOT NULL DEFAULT ARRAY['read:matches','read:standings'],
    quota BIGINT NOT NULL DEFAULT 10000,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_api_keys_account ON api_keys(account_id);

CREATE TABLE IF NOT EXISTS usage_daily (
    key_id UUID NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    day DATE NOT NULL,
    requests BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (key_id, day)
);

CREATE TABLE IF NOT EXISTS competitions (
    code TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    tier INTEGER NOT NULL,
    source TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS seasons (
    id TEXT PRIMARY KEY,
    competition_code TEXT NOT NULL REFERENCES competitions(code),
    label TEXT NOT NULL,
    source_season_id INTEGER NOT NULL,
    source TEXT NOT NULL,
    UNIQUE (competition_code, source_season_id, source)
);

CREATE TABLE IF NOT EXISTS groups (
    id TEXT PRIMARY KEY,
    competition_code TEXT NOT NULL REFERENCES competitions(code),
    source_group_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    UNIQUE (source, source_group_id)
);

CREATE TABLE IF NOT EXISTS clubs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source TEXT NOT NULL,
    source_club_id INTEGER NOT NULL,
    canonical_name TEXT NOT NULL,
    short_name TEXT NOT NULL,
    logo_url TEXT,
    UNIQUE (source, source_club_id)
);

CREATE TABLE IF NOT EXISTS matches (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source TEXT NOT NULL,
    source_match_id BIGINT NOT NULL,
    competition_code TEXT NOT NULL REFERENCES competitions(code),
    season_id TEXT NOT NULL REFERENCES seasons(id),
    group_id TEXT REFERENCES groups(id),
    round INTEGER NOT NULL,
    kickoff_utc TIMESTAMPTZ NOT NULL,
    home_club_id UUID NOT NULL REFERENCES clubs(id),
    away_club_id UUID NOT NULL REFERENCES clubs(id),
    home_score INTEGER,
    away_score INTEGER,
    status TEXT NOT NULL DEFAULT 'scheduled',
    venue TEXT,
    is_live BOOLEAN NOT NULL DEFAULT FALSE,
    live_minute TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (source, source_match_id)
);
CREATE INDEX IF NOT EXISTS idx_matches_season ON matches(season_id, round);
CREATE INDEX IF NOT EXISTS idx_matches_kickoff ON matches(kickoff_utc);
CREATE INDEX IF NOT EXISTS idx_matches_live ON matches(is_live) WHERE is_live;
CREATE INDEX IF NOT EXISTS idx_matches_home ON matches(home_club_id);
CREATE INDEX IF NOT EXISTS idx_matches_away ON matches(away_club_id);

CREATE TABLE IF NOT EXISTS match_events (
    id BIGSERIAL PRIMARY KEY,
    match_id UUID NOT NULL REFERENCES matches(id) ON DELETE CASCADE,
    minute INTEGER,
    kind TEXT NOT NULL,
    team_side TEXT,
    player TEXT,
    detail TEXT,
    UNIQUE (match_id, minute, kind, player)
);

CREATE TABLE IF NOT EXISTS standings (
    season_id TEXT NOT NULL REFERENCES seasons(id),
    group_id TEXT,
    group_key TEXT NOT NULL DEFAULT '',
    club_id UUID NOT NULL REFERENCES clubs(id),
    matchday INTEGER NOT NULL,
    position INTEGER NOT NULL,
    played INTEGER NOT NULL,
    wins INTEGER NOT NULL,
    draws INTEGER NOT NULL,
    losses INTEGER NOT NULL,
    goals_for INTEGER NOT NULL,
    goals_against INTEGER NOT NULL,
    goal_difference INTEGER NOT NULL,
    points INTEGER NOT NULL,
    captured_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (season_id, group_key, club_id, matchday)
);
