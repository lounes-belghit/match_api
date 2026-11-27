use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use chrono::{Duration, Utc};
use serde::Deserialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::crypto::sha256_hex;
use crate::models::{ApiError, ApiResult};
use crate::AppState;

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub account_id: Uuid,
    pub key_id: Uuid,
    pub key_name: String,
    pub plan: String,
    pub scopes: Vec<String>,
    pub quota: i64,
    pub requests_used: i64,
    pub rate_limit: usize,
    pub rate_remaining: usize,
}

impl AuthContext {
    pub fn require(&self, scope: &str) -> ApiResult<()> {
        if self.scopes.iter().any(|value| value == scope) {
            Ok(())
        } else {
            Err(ApiError::Forbidden(format!(
                "missing required scope: {scope}"
            )))
        }
    }

    pub fn requests_remaining(&self) -> i64 {
        (self.quota - self.requests_used).max(0)
    }

    pub fn response_headers(&self) -> Vec<(HeaderName, HeaderValue)> {
        vec![
            (
                HeaderName::from_static("x-ratelimit-limit"),
                HeaderValue::from_str(&self.rate_limit.to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("0")),
            ),
            (
                HeaderName::from_static("x-ratelimit-remaining"),
                HeaderValue::from_str(&self.rate_remaining.to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("0")),
            ),
            (
                HeaderName::from_static("x-requests-remaining"),
                HeaderValue::from_str(&self.requests_remaining().to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("0")),
            ),
        ]
    }

    pub(crate) async fn authenticate(parts: &mut Parts, app: &AppState) -> ApiResult<Self> {
        let raw = extract_api_key(&parts.headers).ok_or_else(|| {
            ApiError::Unauthorized("provide X-API-Key or Authorization: Bearer".into())
        })?;
        let hash = sha256_hex(&raw);
        let row: AuthRow = sqlx::query_as(
            "SELECT k.id AS key_id, k.account_id, k.name, a.plan, k.scopes, a.quota,
                    COALESCE(p.rate_per_min, $2::integer) AS rate_per_min
             FROM api_keys k
             JOIN accounts a ON a.id = k.account_id
             LEFT JOIN plans p ON p.name = a.plan
             WHERE k.key_hash = $1 AND k.revoked = FALSE",
        )
        .bind(&hash)
        .bind(app.config.rate_per_min as i32)
        .fetch_optional(&app.pool)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid or revoked API key".into()))?;

        let used: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(u.requests), 0)::bigint FROM usage_daily u
             JOIN api_keys k ON k.id = u.key_id
             WHERE k.account_id = $1 AND u.day >= DATE_TRUNC('month', now())::date",
        )
        .bind(row.account_id)
        .fetch_one(&app.pool)
        .await?;
        if used >= row.quota {
            return Err(ApiError::TooMany("monthly quota exceeded".into()));
        }
        let decision = app
            .limiter
            .allow(
                &format!("account:{}", row.account_id),
                row.rate_per_min as usize,
            )
            .await?;
        if !decision.allowed {
            return Err(ApiError::TooMany("rate limit exceeded".into()));
        }

        sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
            .bind(row.key_id)
            .execute(&app.pool)
            .await?;
        sqlx::query(
            "INSERT INTO usage_daily (key_id, day, requests) VALUES ($1, CURRENT_DATE, 1)
             ON CONFLICT (key_id, day) DO UPDATE SET requests = usage_daily.requests + 1",
        )
        .bind(row.key_id)
        .execute(&app.pool)
        .await?;

        Ok(Self {
            account_id: row.account_id,
            key_id: row.key_id,
            key_name: row.name,
            plan: row.plan,
            scopes: row.scopes,
            quota: row.quota,
            requests_used: used + 1,
            rate_limit: row.rate_per_min as usize,
            rate_remaining: decision.remaining,
        })
    }
}

#[derive(Debug, FromRow)]
struct AuthRow {
    key_id: Uuid,
    account_id: Uuid,
    name: String,
    plan: String,
    scopes: Vec<String>,
    quota: i64,
    rate_per_min: i32,
}

impl<S> FromRequestParts<S> for AuthContext
where
    S: Send + Sync,
    AppState: axum::extract::FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if let Some(context) = parts.extensions.get::<AuthContext>() {
            return Ok(context.clone());
        }
        let State(app) = State::<AppState>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::Internal("missing app state".into()))?;
        Self::authenticate(parts, &app).await
    }
}

fn extract_api_key(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    let auth = headers.get("authorization")?.to_str().ok()?;
    let token = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))?
        .trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct SessionAccount {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub plan: String,
    pub quota: i64,
    pub is_admin: bool,
}

impl SessionAccount {
    pub fn require_admin(&self) -> ApiResult<()> {
        if self.is_admin {
            Ok(())
        } else {
            Err(ApiError::Forbidden("admin access required".into()))
        }
    }
}

pub async fn session_account(app: &AppState, headers: &HeaderMap) -> Option<SessionAccount> {
    let cookie = headers.get("cookie")?.to_str().ok()?;
    let token = cookie
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(name, _)| *name == "m3_session")
        .map(|(_, value)| value.to_string())?;
    let hash = sha256_hex(&token);
    sqlx::query_as::<_, SessionAccount>(
        "SELECT a.id, a.email, a.name, a.plan, a.quota, a.is_admin
         FROM sessions s JOIN accounts a ON a.id = s.account_id
         WHERE s.token_hash = $1 AND s.expires_at > now()",
    )
    .bind(hash)
    .fetch_optional(&app.pool)
    .await
    .ok()
    .flatten()
}

pub async fn create_session(app: &AppState, account_id: Uuid) -> anyhow::Result<String> {
    let token = crate::crypto::random_token(32);
    let hash = sha256_hex(&token);
    let expires = Utc::now() + Duration::hours(app.config.session_ttl_hours);
    sqlx::query("DELETE FROM sessions WHERE expires_at < now()")
        .execute(&app.pool)
        .await?;
    sqlx::query("INSERT INTO sessions (token_hash, account_id, expires_at) VALUES ($1,$2,$3)")
        .bind(hash)
        .bind(account_id)
        .bind(expires)
        .execute(&app.pool)
        .await?;
    Ok(token)
}

pub async fn delete_session(app: &AppState, headers: &HeaderMap) -> anyhow::Result<()> {
    let Some(cookie) = headers.get("cookie").and_then(|v| v.to_str().ok()) else {
        return Ok(());
    };
    let Some(token) = cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == "m3_session").then(|| value.to_string())
    }) else {
        return Ok(());
    };
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
        .bind(sha256_hex(&token))
        .execute(&app.pool)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct KeyScope {
    pub name: String,
    pub scope: String,
}

#[cfg(test)]
mod tests {
    use super::AuthContext;
    use uuid::Uuid;

    fn context(scopes: &[&str]) -> AuthContext {
        AuthContext {
            account_id: Uuid::nil(),
            key_id: Uuid::nil(),
            key_name: "test".into(),
            plan: "free".into(),
            scopes: scopes.iter().map(|value| value.to_string()).collect(),
            quota: 100,
            requests_used: 10,
            rate_limit: 300,
            rate_remaining: 290,
        }
    }

    #[test]
    fn scope_require_accepts_granted_scope() {
        assert!(context(&["read:matches"]).require("read:matches").is_ok());
    }

    #[test]
    fn scope_require_rejects_missing_scope() {
        assert!(context(&["read:matches"])
            .require("read:standings")
            .is_err());
    }

    #[test]
    fn auth_context_exposes_usage_headers() {
        let headers = context(&["read:matches"]).response_headers();
        assert_eq!(headers.len(), 3);
        assert_eq!(headers[0].0, "x-ratelimit-limit");
        assert_eq!(headers[2].0, "x-requests-remaining");
    }
}
