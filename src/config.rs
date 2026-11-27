use anyhow::{anyhow, bail};
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub environment: String,
    pub database_url: String,
    pub redis_url: Option<String>,
    pub bind_addr: String,
    pub default_quota: i64,
    pub rate_per_min: usize,
    pub cache_ttl_secs: u64,
    pub stale_after_secs: u64,
    pub ingest_alert_after_secs: u64,
    pub session_ttl_hours: i64,
    pub app_secret: String,
    pub cors_origins: Vec<String>,
    pub cookie_secure: bool,
    pub registrations_enabled: bool,
    pub db_max_connections: u32,
    pub db_acquire_timeout_secs: u64,
    pub source_timeout_secs: u64,
    pub ingest_concurrency: usize,
    pub ingest_max_retries: u32,
    pub ingest_retry_base_ms: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let environment = env::var("APP_ENV")
            .unwrap_or_else(|_| "development".to_string())
            .trim()
            .to_lowercase();
        let production = Self::is_production_name(&environment);

        let database_url = match env::var("DATABASE_URL") {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) => bail!("DATABASE_URL must not be empty"),
            Err(_) if production => bail!("DATABASE_URL is required in production"),
            Err(_) => "postgres://matchs:matchs@localhost:55432/matchs".to_string(),
        };
        let app_secret = match env::var("APP_SECRET") {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) => bail!("APP_SECRET must not be empty"),
            Err(_) if production => bail!("APP_SECRET is required in production"),
            Err(_) => "dev-only-change-me".to_string(),
        };
        let redis_url = match env::var("REDIS_URL") {
            Ok(value) if !value.trim().is_empty() => Some(value),
            Ok(_) => bail!("REDIS_URL must not be empty"),
            Err(_) if production => bail!("REDIS_URL is required in production"),
            Err(_) => None,
        };
        let cors_origins = match env::var("CORS_ALLOWED_ORIGINS") {
            Ok(value) => parse_list(&value),
            Err(_) if production => bail!("CORS_ALLOWED_ORIGINS is required in production"),
            Err(_) => vec![
                "http://localhost:3000".to_string(),
                "http://127.0.0.1:3000".to_string(),
            ],
        };

        let config = Self {
            environment,
            database_url,
            redis_url,
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            default_quota: parse_env("DEFAULT_QUOTA", 100_000)?,
            rate_per_min: parse_env("RATE_PER_MIN", 300)?,
            cache_ttl_secs: parse_env("CACHE_TTL_SECS", 60)?,
            stale_after_secs: parse_env("STALE_AFTER_SECS", 900)?,
            ingest_alert_after_secs: parse_env("INGEST_ALERT_AFTER_SECS", 3600)?,
            session_ttl_hours: parse_env("SESSION_TTL_HOURS", 168)?,
            app_secret,
            cors_origins,
            cookie_secure: parse_bool_env("COOKIE_SECURE", production)?,
            registrations_enabled: parse_bool_env("REGISTRATIONS_ENABLED", true)?,
            db_max_connections: parse_env("DB_MAX_CONNECTIONS", 20)?,
            db_acquire_timeout_secs: parse_env("DB_ACQUIRE_TIMEOUT_SECS", 10)?,
            source_timeout_secs: parse_env("SOURCE_TIMEOUT_SECS", 20)?,
            ingest_concurrency: parse_env("INGEST_CONCURRENCY", 6)?,
            ingest_max_retries: parse_env("INGEST_MAX_RETRIES", 3)?,
            ingest_retry_base_ms: parse_env("INGEST_RETRY_BASE_MS", 500)?,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn is_production(&self) -> bool {
        Self::is_production_name(&self.environment)
    }

    fn is_production_name(value: &str) -> bool {
        matches!(value, "production" | "prod")
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.is_production() && self.app_secret.len() < 32 {
            bail!("APP_SECRET must be at least 32 characters in production");
        }
        if !self.is_production()
            && self.app_secret.contains("change-me")
            && self.app_secret.len() < 16
        {
            bail!("APP_SECRET is too short");
        }
        if self.cors_origins.is_empty() {
            bail!("CORS_ALLOWED_ORIGINS must contain at least one origin");
        }
        if self.cors_origins.iter().any(|origin| origin.trim() == "*") {
            bail!("wildcard CORS origins are not allowed");
        }
        for origin in &self.cors_origins {
            if !(origin.starts_with("https://")
                || origin.starts_with("http://localhost")
                || origin.starts_with("http://127.0.0.1"))
            {
                bail!("CORS origin must be HTTPS or localhost: {origin}");
            }
        }
        if self.default_quota <= 0 {
            bail!("DEFAULT_QUOTA must be positive");
        }
        if self.rate_per_min == 0 {
            bail!("RATE_PER_MIN must be positive");
        }
        if self.cache_ttl_secs == 0 {
            bail!("CACHE_TTL_SECS must be positive");
        }
        if self.stale_after_secs == 0 {
            bail!("STALE_AFTER_SECS must be positive");
        }
        if self.ingest_alert_after_secs == 0 {
            bail!("INGEST_ALERT_AFTER_SECS must be positive");
        }
        if self.session_ttl_hours <= 0 {
            bail!("SESSION_TTL_HOURS must be positive");
        }
        if self.db_max_connections == 0 || self.db_max_connections > 100 {
            bail!("DB_MAX_CONNECTIONS must be between 1 and 100");
        }
        if self.db_acquire_timeout_secs == 0 {
            bail!("DB_ACQUIRE_TIMEOUT_SECS must be positive");
        }
        if self.source_timeout_secs == 0 {
            bail!("SOURCE_TIMEOUT_SECS must be positive");
        }
        if self.ingest_concurrency == 0 || self.ingest_concurrency > 64 {
            bail!("INGEST_CONCURRENCY must be between 1 and 64");
        }
        if self.ingest_retry_base_ms == 0 {
            bail!("INGEST_RETRY_BASE_MS must be positive");
        }
        Ok(())
    }
}

fn env_or(name: &str, default: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => default.to_string(),
    }
}

fn parse_env<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value
            .trim()
            .parse::<T>()
            .map_err(|err| anyhow!("invalid {name}: {err}")),
        Err(_) => Ok(default),
    }
}

fn parse_bool_env(name: &str, default: bool) -> anyhow::Result<bool> {
    match env::var(name) {
        Ok(value) => match value.trim().to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            other => bail!("invalid {name}: {other}"),
        },
        Err(_) => Ok(default),
    }
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Config;

    fn config() -> Config {
        Config {
            environment: "production".into(),
            database_url: "postgres://u:p@localhost/db".into(),
            redis_url: Some("redis://localhost:6379".into()),
            bind_addr: "0.0.0.0:8080".into(),
            default_quota: 100_000,
            rate_per_min: 300,
            cache_ttl_secs: 60,
            stale_after_secs: 900,
            ingest_alert_after_secs: 3600,
            session_ttl_hours: 168,
            app_secret: "x".repeat(32),
            cors_origins: vec!["https://example.com".into()],
            cookie_secure: true,
            registrations_enabled: true,
            db_max_connections: 20,
            db_acquire_timeout_secs: 10,
            source_timeout_secs: 20,
            ingest_concurrency: 6,
            ingest_max_retries: 3,
            ingest_retry_base_ms: 500,
        }
    }

    #[test]
    fn production_config_validates() {
        assert!(config().validate().is_ok());
    }

    #[test]
    fn production_config_rejects_short_secret() {
        let mut config = config();
        config.app_secret = "short".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn production_config_rejects_wildcard_cors() {
        let mut config = config();
        config.cors_origins = vec!["*".into()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn production_config_rejects_http_origin() {
        let mut config = config();
        config.cors_origins = vec!["http://example.com".into()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn production_config_rejects_zero_ingest_alert_window() {
        let mut config = config();
        config.ingest_alert_after_secs = 0;
        assert!(config.validate().is_err());
    }
}
