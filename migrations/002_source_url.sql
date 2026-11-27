ALTER TABLE matches ADD COLUMN IF NOT EXISTS source_url TEXT;

UPDATE matches
SET source_url = CASE source
    WHEN 'lfp' THEN 'https://www.lfp.dz/fr/match/' || source_match_id
    WHEN 'lnfa' THEN 'https://www.lnfa.dz/en/match/' || source_match_id
    WHEN 'lirf' THEN 'https://www.lirf.dz/en/match/' || source_match_id
    ELSE NULL
END
WHERE source_url IS NULL;
