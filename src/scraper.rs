use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Deserializer};
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
}

#[derive(Debug, Clone)]
pub struct RawEvent {
    pub minute: Option<i32>,
    pub kind: String,
    pub detail: Option<String>,
}
