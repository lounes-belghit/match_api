use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Competition {
    pub code: String,
    pub name: String,
    pub tier: i32,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Season {
    pub id: String,
    pub competition_code: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FootballGroup {
    pub id: String,
    pub competition_code: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Club {
    pub id: Uuid,
    pub name: String,
    pub short_name: String,
    pub logo_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Match {
    pub id: Uuid,
    pub competition_code: String,
    pub season_id: String,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    pub round: i32,
    pub kickoff_utc: DateTime<Utc>,
    pub home: Club,
    pub away: Club,
    pub home_score: Option<i32>,
    pub away_score: Option<i32>,
    pub status: String,
    pub venue: Option<String>,
    pub is_live: bool,
    pub live_minute: Option<String>,
    pub source: String,
    pub source_url: String,
    pub updated_at: DateTime<Utc>,
    pub last_polled_at: DateTime<Utc>,
    pub source_updated_at: Option<DateTime<Utc>>,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchEvent {
    pub id: i64,
    pub minute: Option<i32>,
    pub kind: String,
    pub team_side: Option<String>,
    pub player: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Standing {
    pub club: Club,
    pub position: i32,
    pub played: i32,
    pub wins: i32,
    pub draws: i32,
    pub losses: i32,
    pub goals_for: i32,
    pub goals_against: i32,
    pub goal_difference: i32,
    pub points: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormOutcome {
    pub match_id: Uuid,
    pub kickoff_utc: DateTime<Utc>,
    pub opponent: Club,
    pub venue: String,
    pub result: String,
    pub goals_for: i32,
    pub goals_against: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamForm {
    pub club: Club,
    pub played: i32,
    pub wins: i32,
    pub draws: i32,
    pub losses: i32,
    pub goals_for: i32,
    pub goals_against: i32,
    pub form: Vec<FormOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadToHead {
    pub club: Club,
    pub opponent: Club,
    pub played: i32,
    pub club_wins: i32,
    pub draws: i32,
    pub opponent_wins: i32,
    pub club_goals: i32,
    pub opponent_goals: i32,
    pub matches: Vec<Match>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub page: u32,
    pub per_page: u32,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized(String),
    Forbidden(String),
    NotFound(String),
    TooMany(String),
    Internal(String),
}

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::TooMany(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::Unauthorized(_) => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::TooMany(_) => "rate_limited",
            Self::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let message = match &self {
            Self::BadRequest(m)
            | Self::Unauthorized(m)
            | Self::Forbidden(m)
            | Self::NotFound(m)
            | Self::TooMany(m)
            | Self::Internal(m) => m.clone(),
        };
        let mut response = (
            status,
            Json(ErrorBody {
                error: self.code().to_string(),
                message,
            }),
        )
            .into_response();
        if status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert("retry-after", "60".parse().unwrap());
        }
        response
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!(error = %err, "internal error");
        Self::Internal("internal server error".to_string())
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(error = %err, "database error");
        Self::Internal("internal server error".to_string())
    }
}
