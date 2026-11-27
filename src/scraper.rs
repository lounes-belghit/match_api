use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, warn};

fn nullable_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone)]
pub struct SourceSpec {
    pub competition_code: &'static str,
    pub base_url: &'static str,
    pub lang: &'static str,
    pub seasons_back: usize,
}

pub fn sources() -> Vec<SourceSpec> {
    vec![
        SourceSpec {
            competition_code: "L1",
            base_url: "https://www.lfp.dz",
            lang: "fr",
            seasons_back: 3,
        },
        SourceSpec {
            competition_code: "L2",
            base_url: "https://www.lnfa.dz",
            lang: "en",
            seasons_back: 2,
        },
        SourceSpec {
            competition_code: "L3",
            base_url: "https://www.lirf.dz",
            lang: "en",
            seasons_back: 3,
        },
    ]
}

#[derive(Debug, Clone, Deserialize)]
pub struct IdName {
    pub id: serde_json::Value,
    #[serde(default, deserialize_with = "nullable_string")]
    pub name: String,
}

impl IdName {
    pub fn id_i64(&self) -> Option<i64> {
        self.id.as_i64().or_else(|| self.id.as_str()?.parse().ok())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ClubDto {
    pub id: serde_json::Value,
    #[serde(default, deserialize_with = "nullable_string")]
    pub name: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub surname: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub image_url: String,
}

impl ClubDto {
    pub fn id_i64(&self) -> Option<i64> {
        self.id.as_i64().or_else(|| self.id.as_str()?.parse().ok())
    }

    pub fn short(&self) -> String {
        if self.surname.trim().is_empty() || self.surname == "-" {
            self.name.chars().take(10).collect()
        } else {
            self.surname.clone()
        }
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Deserialize)]
pub struct NationalityDto {
    #[serde(default, deserialize_with = "nullable_string")]
    pub name: String,
    pub imageUrl: Option<String>,
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Deserialize)]
pub struct PlayerDto {
    pub id: serde_json::Value,
    #[serde(default, deserialize_with = "nullable_string")]
    pub name: String,
    #[serde(default)]
    pub jersey: Option<i32>,
    #[serde(default)]
    pub age: Option<i32>,
    #[serde(default, deserialize_with = "nullable_string")]
    pub position: String,
    pub imageUrl: Option<String>,
    #[serde(default)]
    pub nationality: Option<NationalityDto>,
}

impl PlayerDto {
    pub fn id_i64(&self) -> Option<i64> {
        self.id.as_i64().or_else(|| self.id.as_str()?.parse().ok())
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayersResponse {
    #[serde(default, alias = "goalkeepers")]
    pub goalKeepers: Vec<PlayerDto>,
    #[serde(default)]
    pub defenders: Vec<PlayerDto>,
    #[serde(default)]
    pub midfielders: Vec<PlayerDto>,
    #[serde(default, alias = "strikers")]
    pub forwards: Vec<PlayerDto>,
    #[serde(default)]
    pub attackers: Vec<PlayerDto>,
}

impl PlayersResponse {
    pub fn into_players(self) -> Vec<PlayerDto> {
        self.goalKeepers
            .into_iter()
            .chain(self.defenders)
            .chain(self.midfielders)
            .chain(self.forwards)
            .chain(self.attackers)
            .collect()
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Deserialize)]
struct CompetitionsResponse {
    #[serde(default)]
    groups: Vec<IdName>,
    #[serde(default)]
    rounds: Vec<serde_json::Value>,
    #[serde(default)]
    selectedRoundNumber: Option<i32>,
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Deserialize)]
pub struct MatchDto {
    pub id: serde_json::Value,
    #[serde(default)]
    pub homeClub: ClubDto,
    #[serde(default)]
    pub awayClub: ClubDto,
    #[serde(default)]
    pub group: Option<IdName>,
    #[serde(default, deserialize_with = "nullable_string")]
    pub homeScore: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub awayScore: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub date: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub time: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub stade: String,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub liveTime: Option<String>,
    #[serde(default)]
    pub matchStartTimestamp: Option<i64>,
    #[serde(default, deserialize_with = "nullable_string")]
    pub round: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub decision: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub url: String,
}

impl MatchDto {
    pub fn id_i64(&self) -> Option<i64> {
        self.id.as_i64().or_else(|| self.id.as_str()?.parse().ok())
    }

    pub fn round_number(&self) -> i32 {
        let digits: String = self.round.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.parse().unwrap_or(0)
    }

    pub fn kickoff_utc(&self) -> Option<DateTime<Utc>> {
        if let Some(ts) = self.matchStartTimestamp {
            if let Some(dt) = DateTime::from_timestamp(ts, 0) {
                return Some(dt);
            }
        }
        let parts: Vec<&str> = self.date.split_whitespace().collect();
        if parts.len() < 4 {
            return None;
        }
        let day: u32 = parts[1].parse().ok()?;
        let month = parse_month(parts[2])?;
        let year: i32 = parts[3].parse().ok()?;
        let clock: Vec<&str> = self.time.split(':').collect();
        if clock.len() < 2 {
            return None;
        }
        let hour: u32 = clock[0].parse().ok()?;
        let minute: u32 = clock[1].parse().ok()?;
        let naive = NaiveDate::from_ymd_opt(year, month, day)?
            .and_time(NaiveTime::from_hms_opt(hour, minute, 0)?);
        let algiers = FixedOffset::east_opt(3600)?
            .from_local_datetime(&naive)
            .single()?;
        Some(algiers.with_timezone(&Utc))
    }

    pub fn status(&self) -> &'static str {
        let played = self.homeScore.trim() != "-" && !self.homeScore.trim().is_empty();
        if self.live {
            "live"
        } else if played {
            "finished"
        } else if self.decision.to_uppercase().contains("POSTPONED") {
            "postponed"
        } else {
            "scheduled"
        }
    }

    pub fn scores(&self) -> (Option<i32>, Option<i32>) {
        let parse = |v: &str| -> Option<i32> {
            let v = v.trim();
            if v.is_empty() || v == "-" {
                None
            } else {
                v.parse().ok()
            }
        };
        (parse(&self.homeScore), parse(&self.awayScore))
    }

    pub fn source_path(&self) -> String {
        if !self.url.is_empty() {
            return self.url.clone();
        }
        format!("/match/{}", self.id.as_i64().unwrap_or_default())
    }
}

fn parse_month(value: &str) -> Option<u32> {
    let normalized = value.trim().trim_matches('.').to_lowercase();
    let months = [
        ("janvier", 1),
        ("january", 1),
        ("février", 2),
        ("fevrier", 2),
        ("february", 2),
        ("mars", 3),
        ("march", 3),
        ("avril", 4),
        ("april", 4),
        ("mai", 5),
        ("may", 5),
        ("juin", 6),
        ("june", 6),
        ("juillet", 7),
        ("july", 7),
        ("août", 8),
        ("aout", 8),
        ("august", 8),
        ("septembre", 9),
        ("september", 9),
        ("octobre", 10),
        ("october", 10),
        ("novembre", 11),
        ("november", 11),
        ("décembre", 12),
        ("decembre", 12),
        ("december", 12),
    ];
    months
        .iter()
        .find(|(name, _)| *name == normalized)
        .map(|(_, n)| *n)
}

#[derive(Debug, Clone)]
pub struct DiscoveredSeason {
    pub source_id: i64,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct SourceClient {
    spec: SourceSpec,
    http: Client,
    max_retries: u32,
    retry_base: Duration,
}

impl SourceClient {
    pub fn new(spec: SourceSpec, timeout: Duration) -> Result<Self> {
        Self::with_retry(spec, timeout, 3, Duration::from_millis(500))
    }

    pub fn with_retry(
        spec: SourceSpec,
        timeout: Duration,
        max_retries: u32,
        retry_base: Duration,
    ) -> Result<Self> {
        let http = Client::builder()
            .timeout(timeout)
            .user_agent("matchs-api/1.0")
            .build()?;
        Ok(Self {
            spec,
            http,
            max_retries,
            retry_base,
        })
    }

    pub fn competition_code(&self) -> &str {
        self.spec.competition_code
    }

    pub fn source_name(&self) -> &str {
        match self.spec.competition_code {
            "L1" => "lfp",
            "L2" => "lnfa",
            _ => "lirf",
        }
    }

    pub fn base_url(&self) -> String {
        self.spec.base_url.to_string()
    }

    pub fn match_url(&self, path: &str) -> String {
        format!("{}/{}{}", self.spec.base_url, self.spec.lang, path)
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}{}", self.spec.base_url, self.spec.lang, path)
    }

    async fn get_text(&self, url: &str) -> Result<String> {
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            match self.http.get(url).send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return response
                            .text()
                            .await
                            .with_context(|| format!("reading response from {url}"));
                    }
                    let error = anyhow!("HTTP {status} from {url}");
                    if status.is_server_error() || status.as_u16() == 429 {
                        last_error = Some(error);
                    } else {
                        return Err(error);
                    }
                }
                Err(error) => last_error = Some(anyhow!("request to {url} failed: {error}")),
            }
            if attempt < self.max_retries {
                let delay = self.retry_base * 2_u32.pow(attempt);
                tokio::time::sleep(delay).await;
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("request to {url} failed")))
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let url = self.url(path);
        let body = self.get_text(&url).await?;
        serde_json::from_str::<T>(&body).with_context(|| format!("invalid JSON from {url}"))
    }

    pub async fn discover_seasons(&self) -> Result<Vec<DiscoveredSeason>> {
        let url = self.url("/calendar");
        let html = self.get_text(&url).await?;
        let doc = Html::parse_document(&html);
        let selector = Selector::parse("#calendar-season-selector .dropdown-item")
            .map_err(|e| anyhow!("bad selector: {e}"))?;
        let mut seasons = Vec::new();
        for element in doc.select(&selector) {
            let Some(value) = element.value().attr("data-value") else {
                continue;
            };
            let Ok(source_id) = value.parse::<i64>() else {
                continue;
            };
            let label = element.text().collect::<String>().trim().to_string();
            if !label.is_empty() {
                seasons.push(DiscoveredSeason { source_id, label });
            }
        }
        seasons.sort_by_key(|s| std::cmp::Reverse(s.source_id));
        seasons.truncate(self.spec.seasons_back);
        if seasons.is_empty() {
            return Err(anyhow!("no seasons discovered from {url}"));
        }
        Ok(seasons)
    }

    pub async fn groups(&self, season_id: i64) -> Result<Vec<IdName>> {
        let response: CompetitionsResponse = self
            .get_json(&format!("/calendar-competitions-data?season={season_id}"))
            .await?;
        Ok(response.groups)
    }

    pub async fn matches(
        &self,
        season_id: i64,
        group_id: i64,
        round: i32,
    ) -> Result<Vec<MatchDto>> {
        let path = format!(
            "/calendar-data?division={}&season={}&matchday={}",
            group_id, season_id, round
        );
        let matches: Vec<MatchDto> = self.get_json(&path).await?;
        Ok(matches)
    }

    pub async fn rounds(&self, season_id: i64) -> Result<(Vec<i32>, Option<i32>)> {
        let response: CompetitionsResponse = self
            .get_json(&format!("/calendar-competitions-data?season={season_id}"))
            .await?;
        let mut rounds: Vec<i32> = response
            .rounds
            .iter()
            .filter_map(|value| {
                if let Some(v) = value.as_i64() {
                    return Some(v as i32);
                }
                value
                    .get("roundNumber")
                    .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                    .map(|v| v as i32)
            })
            .collect();
        rounds.sort_unstable();
        rounds.dedup();
        Ok((rounds, response.selectedRoundNumber))
    }

    pub async fn match_events(&self, match_path: &str) -> Result<Vec<RawEvent>> {
        let url = format!("{}/{}{}", self.spec.base_url, self.spec.lang, match_path);
        let html = match self.get_text(&url).await {
            Ok(html) => html,
            Err(err) => {
                warn!(error = %err, %url, "match detail unavailable");
                debug!(%url, "skipping event extraction after repeated source failures");
                return Ok(Vec::new());
            }
        };
        let doc = Html::parse_document(&html);
        let selector =
            Selector::parse(".match-event-tile").map_err(|e| anyhow!("bad selector: {e}"))?;
        let time_selector =
            Selector::parse(".match-event-time").map_err(|e| anyhow!("bad selector: {e}"))?;
        let desc_selector =
            Selector::parse(".match-event-desc").map_err(|e| anyhow!("bad selector: {e}"))?;
        let mut events = Vec::new();
        for element in doc.select(&selector) {
            let classes = element.value().classes().collect::<Vec<_>>().join(" ");
            let raw_time = element
                .select(&time_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            let minute = raw_time
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok();
            let detail = element
                .select(&desc_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            let kind = if classes.contains("goal-match-event")
                || detail.to_lowercase().contains("goal")
            {
                "goal"
            } else if classes.contains("yellow") || detail.to_lowercase().contains("yellow card") {
                "yellow_card"
            } else if classes.contains("red") || detail.to_lowercase().contains("red card") {
                "red_card"
            } else if detail.to_lowercase().contains("substitution") {
                "substitution"
            } else {
                continue;
            };
            events.push(RawEvent {
                minute,
                kind: kind.to_string(),
                detail: if detail.is_empty() {
                    None
                } else {
                    Some(detail)
                },
            });
        }
        Ok(events)
    }

    pub async fn clubs(&self, group_id: i64) -> Result<Vec<ClubDto>> {
        self.get_json(&format!("/clubs-data?division={group_id}"))
            .await
    }

    pub async fn players(&self, club_id: i64, season_id: i64) -> Result<Vec<PlayerDto>> {
        let response: PlayersResponse = self
            .get_json(&format!("/club/{club_id}/players-data?season={season_id}"))
            .await?;
        Ok(response.into_players())
    }

    pub fn player_url(&self, source_id: i64) -> String {
        self.url(&format!("/player/{source_id}"))
    }

    pub async fn player_stats(&self, group_id: i64) -> Result<Vec<RawPlayerStat>> {
        let path = match self.spec.competition_code {
            "L1" => "/statistics".to_string(),
            "L2" => format!("/statistics/group/{group_id}"),
            _ => format!("/statistics?ajax=1&group={group_id}"),
        };
        let url = self.url(&path);
        let html = self.get_text(&url).await?;
        parse_player_stats(&html)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPlayerStat {
    pub source_player_id: Option<i64>,
    pub name: String,
    pub club_logo_path: Option<String>,
    pub club_name: Option<String>,
    pub stat: &'static str,
    pub value: i32,
    pub rank: Option<i32>,
    pub jersey: Option<i32>,
}

pub fn parse_player_stats(html: &str) -> Result<Vec<RawPlayerStat>> {
    let doc = Html::parse_document(html);
    let mut raw = Vec::new();
    raw.extend(parse_stat_cards(&doc)?);
    raw.extend(parse_incident_cards(&doc)?);
    raw.extend(parse_stat_tables(&doc)?);
    Ok(dedupe_player_stats(raw))
}

fn parse_stat_cards(doc: &Html) -> Result<Vec<RawPlayerStat>> {
    let anchors = Selector::parse("a[href*='/player/']")?;
    let card = Selector::parse(".stat-player-card")?;
    let name = Selector::parse(".player-name.flex-grow-1, .player-name-club > .player-name")?;
    let rank = Selector::parse(".player-rank")?;
    let value = Selector::parse(".player-goals-stat > span, .player-stat > span")?;
    let club_logo = Selector::parse(".player-club-logo img")?;
    let club_name = Selector::parse(".player-club-logo span")?;
    let mut stats = Vec::new();

    for anchor in doc.select(&anchors) {
        let Some(element) = anchor.select(&card).next() else {
            continue;
        };
        let classes = format!("{} {}", ancestor_classes(anchor), anchor_classes(&anchor));
        let Some(stat) = stat_kind(&classes) else {
            continue;
        };
        let Some(source_player_id) = player_id_from_href(&classes, &anchor) else {
            continue;
        };
        let Some(name) = normalized_child(&element, &name) else {
            continue;
        };
        let Some(value) = normalized_child(&element, &value).and_then(|text| parse_i32(&text))
        else {
            continue;
        };
        let logo = element
            .select(&club_logo)
            .next()
            .and_then(|img| logo_path(img.value().attr("src")));
        let club = element
            .select(&club_name)
            .next()
            .map(|el| el.text().collect::<String>())
            .map(normalize_text)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                element
                    .select(&club_logo)
                    .next()
                    .and_then(|img| img.value().attr("alt"))
                    .map(|value| normalize_text(value.to_string()))
                    .filter(|value| !value.is_empty())
            });
        stats.push(RawPlayerStat {
            source_player_id: Some(source_player_id),
            name,
            club_logo_path: logo,
            club_name: club,
            stat,
            value,
            rank: normalized_child(&element, &rank).and_then(|text| parse_i32(&text)),
            jersey: None,
        });
    }
    Ok(stats)
}

fn parse_incident_cards(doc: &Html) -> Result<Vec<RawPlayerStat>> {
    let cards = Selector::parse(".players-stats-lower .stat-player-incident")?;
    let title = Selector::parse(".incident-title")?;
    let name = Selector::parse(".incident-info .player-name")?;
    let value = Selector::parse(".incident-count")?;
    let mut stats = Vec::new();

    for element in doc.select(&cards) {
        let Some(kind) = normalized_child(&element, &title).and_then(|text| stat_kind(&text))
        else {
            continue;
        };
        let Some(name) = normalized_child(&element, &name) else {
            continue;
        };
        let Some(value) = normalized_child(&element, &value).and_then(|text| parse_i32(&text))
        else {
            continue;
        };
        stats.push(RawPlayerStat {
            source_player_id: None,
            name,
            club_logo_path: None,
            club_name: None,
            stat: kind,
            value,
            rank: None,
            jersey: None,
        });
    }
    Ok(stats)
}

fn parse_stat_tables(doc: &Html) -> Result<Vec<RawPlayerStat>> {
    let tables = Selector::parse(".statistics-table table.table")?;
    let headers = Selector::parse("thead th[scope='col']")?;
    let rows = Selector::parse("tbody tr.statistics-row")?;
    let rank = Selector::parse("span.statistics-index")?;
    let name = Selector::parse("span.statistics-player-name")?;
    let cells_selector = Selector::parse("td")?;
    let mut stats = Vec::new();

    for table in doc.select(&tables) {
        let Some(stat) = table
            .select(&headers)
            .find_map(|element| stat_kind(&element.text().collect::<String>()))
        else {
            continue;
        };
        for row in table.select(&rows) {
            let cells: Vec<_> = row.select(&cells_selector).collect();
            let Some(name) = normalized_child(&row, &name) else {
                continue;
            };
            let Some(value) = cells
                .get(1)
                .map(|cell| cell.text().collect::<String>())
                .map(normalize_text)
                .and_then(|text| parse_i32(&text))
            else {
                continue;
            };
            stats.push(RawPlayerStat {
                source_player_id: None,
                name,
                club_logo_path: None,
                club_name: cells
                    .first()
                    .map(|cell| cell.text().collect::<String>())
                    .map(normalize_text)
                    .filter(|text| !text.is_empty()),
                stat,
                value,
                rank: normalized_child(&row, &rank).and_then(|text| parse_i32(&text)),
                jersey: cells
                    .get(2)
                    .map(|cell| cell.text().collect::<String>())
                    .and_then(|text| parse_i32(&text)),
            });
        }
    }
    Ok(stats)
}

fn dedupe_player_stats(items: Vec<RawPlayerStat>) -> Vec<RawPlayerStat> {
    let mut best: HashMap<(String, &'static str), RawPlayerStat> = HashMap::new();
    for item in items {
        let key = (normalize_text(item.name.to_lowercase()), item.stat);
        if let Some(current) = best.get_mut(&key) {
            merge_player_stat(current, item);
        } else {
            best.insert(key, item);
        }
    }
    let mut stats: Vec<_> = best.into_values().collect();
    stats.sort_by(|a, b| {
        a.stat
            .cmp(b.stat)
            .then(a.rank.unwrap_or(i32::MAX).cmp(&b.rank.unwrap_or(i32::MAX)))
            .then(a.name.cmp(&b.name))
    });
    stats
}

fn merge_player_stat(current: &mut RawPlayerStat, incoming: RawPlayerStat) {
    current.source_player_id = current.source_player_id.or(incoming.source_player_id);
    if incoming.club_logo_path.is_some() {
        current.club_logo_path = incoming.club_logo_path;
    }
    if incoming.club_name.is_some() {
        current.club_name = incoming.club_name;
    }
    current.rank = current.rank.or(incoming.rank);
    current.jersey = current.jersey.or(incoming.jersey);
    current.value = current.value.max(incoming.value);
}

fn normalized_child(element: &scraper::ElementRef, selector: &Selector) -> Option<String> {
    element
        .select(selector)
        .next()
        .map(|el| el.text().collect::<String>())
        .map(normalize_text)
        .filter(|value| !value.is_empty())
}

fn normalize_text(value: String) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_i32(value: &str) -> Option<i32> {
    let digits: String = value.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn stat_kind(value: &str) -> Option<&'static str> {
    let value = value.to_lowercase();
    if value.contains("goal") {
        Some("goals")
    } else if value.contains("yellow") {
        Some("yellow_cards")
    } else if value.contains("red") {
        Some("red_cards")
    } else if value.contains("appearance") || value.contains("participation") {
        Some("appearances")
    } else {
        None
    }
}

fn anchor_classes(element: &scraper::ElementRef) -> String {
    element.value().classes().collect::<Vec<_>>().join(" ")
}

fn ancestor_classes(element: scraper::ElementRef) -> String {
    let mut classes = String::new();
    let mut current = Some(element);
    for _ in 0..12 {
        let Some(element) = current else { break };
        classes.push_str(&anchor_classes(&element));
        classes.push(' ');
        current = element.parent().and_then(scraper::ElementRef::wrap);
    }
    classes
}

fn player_id_from_href(classes: &str, anchor: &scraper::ElementRef) -> Option<i64> {
    let href = anchor.value().attr("href")?;
    if !href.contains("/player/") {
        return None;
    }
    stat_kind(classes)?;
    href.rsplit('/')
        .next()
        .and_then(|segment| segment.split('?').next())
        .and_then(|segment| segment.parse().ok())
}

pub fn logo_path(value: Option<&str>) -> Option<String> {
    let value = value?;
    let marker = value.find("/clubs-logos/")?;
    let path = &value[marker..];
    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct RawEvent {
    pub minute: Option<i32>,
    pub kind: String,
    pub detail: Option<String>,
}
