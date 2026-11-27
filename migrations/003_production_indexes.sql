CREATE INDEX IF NOT EXISTS idx_sessions_account ON sessions(account_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);
CREATE INDEX IF NOT EXISTS idx_api_keys_last_used ON api_keys(last_used_at);
CREATE INDEX IF NOT EXISTS idx_usage_daily_day ON usage_daily(day);
CREATE INDEX IF NOT EXISTS idx_matches_status ON matches(status);
CREATE INDEX IF NOT EXISTS idx_match_events_match ON match_events(match_id, minute);
CREATE INDEX IF NOT EXISTS idx_standings_season_group ON standings(season_id, group_key);
