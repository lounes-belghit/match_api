use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};
use uuid::Uuid;

use crate::scraper::{sources, DiscoveredSeason, MatchDto, SourceClient, SourceSpec};
use crate::{AppState, Config};

fn source_client(spec: SourceSpec, config: &Config) -> Result<SourceClient> {
    SourceClient::with_retry(
        spec,
        std::time::Duration::from_secs(config.source_timeout_secs),
        config.ingest_max_retries,
        std::time::Duration::from_millis(config.ingest_retry_base_ms),
    )
}

fn season_key(source: &str, source_id: i64) -> String {
    format!("{}:{}", source, source_id)
}

fn group_key(source: &str, source_id: i64) -> String {
    format!("{}:{}", source, source_id)
}

fn default_rounds(competition_code: &str) -> usize {
    match competition_code {
        "L1" | "L2" => 38,
        _ => 30,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct IngestStats {
    matches_seen: i64,
    matches_upserted: i64,
    events_upserted: i64,
    standings_rebuilt: i64,
}

#[derive(Debug, Clone, Default)]
struct Counters {
    stats: Arc<Mutex<IngestStats>>,
}

impl Counters {
    fn add(&self, update: impl FnOnce(&mut IngestStats)) {
        let mut stats = self.stats.lock().expect("ingest counters lock poisoned");
        update(&mut stats);
    }

    fn snapshot(&self) -> IngestStats {
        *self.stats.lock().expect("ingest counters lock poisoned")
    }
}

async fn start_run(pool: &PgPool, mode: &str) -> Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ingest_runs (id, mode, source, status) VALUES ($1, $2, 'all', 'running')",
    )
    .bind(id)
    .bind(mode)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn finish_run(
    pool: &PgPool,
    id: Uuid,
    stats: IngestStats,
    error: Option<&anyhow::Error>,
) -> Result<()> {
    let status = if error.is_some() { "failed" } else { "success" };
    let error = error.map(|error| format!("{error:#}"));
    sqlx::query(
        "UPDATE ingest_runs SET status = $2, finished_at = now(),
            duration_ms = (EXTRACT(EPOCH FROM (now() - started_at)) * 1000)::bigint,
            matches_seen = $3, matches_upserted = $4, events_upserted = $5,
            standings_rebuilt = $6, error = $7
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(stats.matches_seen)
    .bind(stats.matches_upserted)
    .bind(stats.events_upserted)
    .bind(stats.standings_rebuilt)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

async fn with_run<F, Fut>(app: &AppState, mode: &'static str, operation: F) -> Result<()>
where
    F: FnOnce(Counters) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let id = start_run(&app.pool, mode).await?;
    let counters = Counters::default();
    let result = operation(counters.clone()).await;
    let finish = finish_run(&app.pool, id, counters.snapshot(), result.as_ref().err()).await;
    match result {
        Ok(()) => finish,
        Err(error) => {
            if let Err(finish_error) = finish {
                warn!(error = %finish_error, "failed to persist ingest run");
            }
            Err(error)
        }
    }
}

pub async fn backfill(app: &AppState, seasons_limit: usize) -> Result<()> {
    with_run(app, "backfill", |counters| async move {
        backfill_inner(app, seasons_limit, &counters).await
    })
    .await
}

async fn backfill_inner(app: &AppState, seasons_limit: usize, counters: &Counters) -> Result<()> {
    for spec in sources() {
        let client = source_client(spec.clone(), &app.config)?;
        let seasons = client.discover_seasons().await?;
        let seasons: Vec<DiscoveredSeason> =
            seasons.into_iter().take(seasons_limit.max(1)).collect();
        let source = client.source_name().to_string();
        info!(source = %source, seasons = seasons.len(), "backfilling source");

        for season in seasons {
            upsert_season(&app.pool, &source, spec.competition_code, &season).await?;
            let sid = season_key(&source, season.source_id);
            let groups = client.groups(season.source_id).await?;
            let (mut rounds, _) = client.rounds(season.source_id).await?;
            if rounds.is_empty() {
                rounds = (1..=default_rounds(spec.competition_code) as i32).collect();
            }

            for group in groups {
                let Some(gid) = group.id_i64() else {
                    continue;
                };
                let gid_key = group_key(&source, gid);
                upsert_group(&app.pool, &source, spec.competition_code, gid, &group.name).await?;

                let tasks: Vec<_> = rounds
                    .iter()
                    .map(|round| {
                        let client = client.clone();
                        let round = *round;
                        async move { client.matches(season.source_id, gid, round).await }
                    })
                    .collect();

                let mut stream =
                    stream::iter(tasks).buffer_unordered(app.config.ingest_concurrency);
                while let Some(result) = stream.next().await {
                    let matches = result.with_context(|| {
                        format!(
                            "fetching {source} season {} group {}",
                            season.source_id, gid
                        )
                    })?;
                    counters.add(|stats| stats.matches_seen += matches.len() as i64);
                    for m in matches {
                        upsert_match(&app.pool, &client, &m, &sid, Some(&gid_key), &source).await?;
                        counters.add(|stats| stats.matches_upserted += 1);
                    }
                }
            }
            recompute_standings(&app.pool, &sid).await?;
            counters.add(|stats| stats.standings_rebuilt += 1);
            info!(season = %sid, "season backfill complete");
        }
    }
    app.cache.invalidate_prefix("api:").await;
    Ok(())
}

pub async fn refresh_live(app: &AppState) -> Result<()> {
    with_run(app, "live", |counters| async move {
        refresh_live_inner(app, &counters).await
    })
    .await
}

async fn refresh_live_inner(app: &AppState, counters: &Counters) -> Result<()> {
    for spec in sources() {
        let client = source_client(spec.clone(), &app.config)?;
        let Some(season) = client.discover_seasons().await?.into_iter().next() else {
            continue;
        };
        let source = client.source_name().to_string();
        upsert_season(&app.pool, &source, spec.competition_code, &season).await?;
        let sid = season_key(&source, season.source_id);
        let groups = client.groups(season.source_id).await?;
        let (rounds, current) = client.rounds(season.source_id).await?;
        let round = current
            .or_else(|| rounds.iter().copied().max())
            .unwrap_or(1);

        for group in groups {
            let Some(gid) = group.id_i64() else { continue };
            upsert_group(&app.pool, &source, spec.competition_code, gid, &group.name).await?;
            let matches = client.matches(season.source_id, gid, round).await?;
            counters.add(|stats| stats.matches_seen += matches.len() as i64);
            for m in matches {
                upsert_match(
                    &app.pool,
                    &client,
                    &m,
                    &sid,
                    Some(&group_key(&source, gid)),
                    &source,
                )
                .await?;
                counters.add(|stats| stats.matches_upserted += 1);
            }
        }
        info!(source = %source, season = %sid, round, "live refresh complete");
    }
    app.cache.invalidate_prefix("api:").await;
    Ok(())
}

pub async fn ingest_events(app: &AppState, limit: i64) -> Result<()> {
    with_run(app, "events", |counters| async move {
        ingest_events_inner(app, limit, &counters).await
    })
    .await
}

async fn ingest_events_inner(app: &AppState, limit: i64, counters: &Counters) -> Result<()> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT m.id, m.source_url FROM matches m
         WHERE m.status = 'finished'
           AND NOT EXISTS (SELECT 1 FROM match_events e WHERE e.match_id = m.id)
         ORDER BY m.kickoff_utc DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&app.pool)
    .await?;

    for (match_id, url) in rows {
        counters.add(|stats| stats.matches_seen += 1);
        let Some((spec, path)) = spec_for_url(&url) else {
            continue;
        };
        let client = source_client(spec, &app.config)?;
        let events = client.match_events(&path).await?;
        for event in events {
            let result = sqlx::query(
                "INSERT INTO match_events (match_id, minute, kind, team_side, player, detail)
                 VALUES ($1,$2,$3,NULL,NULL,$4) ON CONFLICT DO NOTHING",
            )
            .bind(match_id)
            .bind(event.minute)
            .bind(event.kind)
            .bind(event.detail)
            .execute(&app.pool)
            .await?;
            if result.rows_affected() > 0 {
                counters.add(|stats| stats.events_upserted += 1);
            }
        }
    }
    app.cache.invalidate_prefix("api:").await;
    Ok(())
}

fn spec_for_url(url: &str) -> Option<(SourceSpec, String)> {
    let spec = sources()
        .into_iter()
        .find(|s| url.starts_with(&format!("{}/{}", s.base_url, s.lang)))?;
    let path = url
        .strip_prefix(&format!("{}/{}", spec.base_url, spec.lang))?
        .to_string();
    Some((spec, path))
}

async fn upsert_season(
    pool: &PgPool,
    source: &str,
    competition: &str,
    season: &DiscoveredSeason,
) -> Result<()> {
    let id = season_key(source, season.source_id);
    sqlx::query(
        "INSERT INTO seasons (id, competition_code, label, source_season_id, source)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (id) DO UPDATE SET label = EXCLUDED.label",
    )
    .bind(id)
    .bind(competition)
    .bind(&season.label)
    .bind(season.source_id)
    .bind(source)
    .execute(pool)
    .await?;
    Ok(())
}

async fn upsert_group(
    pool: &PgPool,
    source: &str,
    competition: &str,
    id: i64,
    name: &str,
) -> Result<()> {
    let key = group_key(source, id);
    sqlx::query(
        "INSERT INTO groups (id, competition_code, source_group_id, name, source)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
    )
    .bind(key)
    .bind(competition)
    .bind(id)
    .bind(name)
    .bind(source)
    .execute(pool)
    .await?;
    Ok(())
}

async fn upsert_club(
    pool: &PgPool,
    source: &str,
    base_url: &str,
    club: &crate::scraper::ClubDto,
) -> Result<Uuid> {
    let logo = if club.image_url.is_empty() || club.image_url == "-" {
        None
    } else if club.image_url.starts_with('/') {
        Some(format!("{}{}", base_url, club.image_url))
    } else {
        Some(club.image_url.clone())
    };
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO clubs (source, source_club_id, canonical_name, short_name, logo_url)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (source, source_club_id) DO UPDATE
           SET canonical_name = EXCLUDED.canonical_name,
               short_name = EXCLUDED.short_name,
               logo_url = COALESCE(EXCLUDED.logo_url, clubs.logo_url)
         RETURNING id",
    )
    .bind(source)
    .bind(club.id_i64())
    .bind(&club.name)
    .bind(club.short())
    .bind(logo)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn upsert_match(
    pool: &PgPool,
    client: &SourceClient,
    m: &MatchDto,
    season_id: &str,
    group_id: Option<&str>,
    source: &str,
) -> Result<()> {
    let Some(match_id) = m.id_i64() else {
        return Ok(());
    };
    let Some(kickoff) = m.kickoff_utc() else {
        warn!(match_id, source, "skipping match with unknown kickoff");
        return Ok(());
    };
    let base_url = client.base_url();
    let Some(home_id) = club_id(pool, source, &base_url, &m.homeClub).await? else {
        return Ok(());
    };
    let Some(away_id) = club_id(pool, source, &base_url, &m.awayClub).await? else {
        return Ok(());
    };
    let (home_score, away_score) = m.scores();
    let venue = if m.stade.trim().is_empty() || m.stade == "-" {
        None
    } else {
        Some(m.stade.clone())
    };
    let live_minute = m.liveTime.clone().filter(|v| !v.is_empty() && v != "-");
    let source_url = client.match_url(&m.source_path());

    sqlx::query(
        "INSERT INTO matches (
            source, source_match_id, competition_code, season_id, group_id, round,
            kickoff_utc, home_club_id, away_club_id, home_score, away_score,
            status, venue, is_live, live_minute, source_url, updated_at, last_polled_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,now(),now())
         ON CONFLICT (source, source_match_id) DO UPDATE SET
            group_id = EXCLUDED.group_id,
            round = EXCLUDED.round,
            kickoff_utc = EXCLUDED.kickoff_utc,
            home_score = EXCLUDED.home_score,
            away_score = EXCLUDED.away_score,
            status = EXCLUDED.status,
            venue = EXCLUDED.venue,
            is_live = EXCLUDED.is_live,
            live_minute = EXCLUDED.live_minute,
            source_url = EXCLUDED.source_url,
            updated_at = now(),
            last_polled_at = EXCLUDED.last_polled_at",
    )
    .bind(source)
    .bind(match_id)
    .bind(client.competition_code())
    .bind(season_id)
    .bind(group_id)
    .bind(m.round_number().max(1))
    .bind(kickoff)
    .bind(home_id)
    .bind(away_id)
    .bind(home_score)
    .bind(away_score)
    .bind(m.status())
    .bind(venue)
    .bind(m.live)
    .bind(live_minute)
    .bind(source_url)
    .execute(pool)
    .await?;
    Ok(())
}

async fn club_id(
    pool: &PgPool,
    source: &str,
    base_url: &str,
    club: &crate::scraper::ClubDto,
) -> Result<Option<Uuid>> {
    if club.id_i64().is_none() {
        return Ok(None);
    }
    Ok(Some(upsert_club(pool, source, base_url, club).await?))
}

#[derive(Default, Clone, Copy)]
struct TeamStats {
    played: i32,
    wins: i32,
    draws: i32,
    losses: i32,
    gf: i32,
    ga: i32,
}

impl TeamStats {
    fn points(&self) -> i32 {
        self.wins * 3 + self.draws
    }

    fn gd(&self) -> i32 {
        self.gf - self.ga
    }
}

#[derive(Debug, FromRow)]
struct MatchStatRow {
    group_id: Option<String>,
    home_club_id: Uuid,
    away_club_id: Uuid,
    home_score: Option<i32>,
    away_score: Option<i32>,
}

async fn recompute_standings(pool: &PgPool, season_id: &str) -> Result<()> {
    let rows: Vec<MatchStatRow> = sqlx::query_as(
        "SELECT group_id, home_club_id, away_club_id, home_score, away_score
         FROM matches WHERE season_id = $1",
    )
    .bind(season_id)
    .fetch_all(pool)
    .await?;

    let mut table: HashMap<(String, Uuid), TeamStats> = HashMap::new();
    for row in rows {
        let group_key = row.group_id.unwrap_or_default();
        let home = row.home_club_id;
        let away = row.away_club_id;
        table.entry((group_key.clone(), home)).or_default();
        table.entry((group_key.clone(), away)).or_default();
        let (Some(hs), Some(as_)) = (row.home_score, row.away_score) else {
            continue;
        };
        let home_stats = table.get_mut(&(group_key.clone(), home)).unwrap();
        home_stats.played += 1;
        home_stats.gf += hs;
        home_stats.ga += as_;
        match hs.cmp(&as_) {
            std::cmp::Ordering::Greater => home_stats.wins += 1,
            std::cmp::Ordering::Less => home_stats.losses += 1,
            std::cmp::Ordering::Equal => home_stats.draws += 1,
        }
        let away_stats = table.get_mut(&(group_key, away)).unwrap();
        away_stats.played += 1;
        away_stats.gf += as_;
        away_stats.ga += hs;
        match as_.cmp(&hs) {
            std::cmp::Ordering::Greater => away_stats.wins += 1,
            std::cmp::Ordering::Less => away_stats.losses += 1,
            std::cmp::Ordering::Equal => away_stats.draws += 1,
        }
    }

    let round: i32 =
        sqlx::query_scalar("SELECT COALESCE(MAX(round), 0) FROM matches WHERE season_id = $1")
            .bind(season_id)
            .fetch_one(pool)
            .await?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM standings WHERE season_id = $1")
        .bind(season_id)
        .execute(&mut *tx)
        .await?;

    let mut entries: Vec<_> = table.into_iter().collect();
    entries.sort_by(|(ka, sa), (kb, sb)| {
        sb.points()
            .cmp(&sa.points())
            .then(sb.gd().cmp(&sa.gd()))
            .then(sb.gf.cmp(&sa.gf))
            .then(kb.1.cmp(&ka.1))
    });
    let mut position = 0;
    let mut previous: Option<(i32, i32, i32)> = None;
    for ((group_key, club_id), stats) in entries {
        let signature = (stats.points(), stats.gd(), stats.gf);
        if previous != Some(signature) {
            position += 1;
            previous = Some(signature);
        }
        sqlx::query(
            "INSERT INTO standings (
                season_id, group_id, group_key, club_id, matchday, position,
                played, wins, draws, losses, goals_for, goals_against, goal_difference, points
             ) VALUES ($1,NULLIF($2,''),$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        )
        .bind(season_id)
        .bind(&group_key)
        .bind(&group_key)
        .bind(club_id)
        .bind(round.max(1))
        .bind(position)
        .bind(stats.played)
        .bind(stats.wins)
        .bind(stats.draws)
        .bind(stats.losses)
        .bind(stats.gf)
        .bind(stats.ga)
        .bind(stats.gd())
        .bind(stats.points())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
