use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Script};
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::config::Config;

type MemoryCache = Arc<tokio::sync::Mutex<HashMap<String, (String, Instant)>>>;
type MemoryBuckets = Arc<tokio::sync::Mutex<HashMap<String, (usize, Instant)>>>;

#[derive(Clone)]
enum CacheInner {
    Memory(MemoryCache),
    Redis(ConnectionManager),
}

#[derive(Clone)]
pub struct Cache {
    inner: CacheInner,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            inner: CacheInner::Memory(Arc::default()),
        }
    }
}

impl Cache {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(url).context("invalid REDIS_URL")?;
        let connection = ConnectionManager::new(client)
            .await
            .context("failed to connect to Redis")?;
        Ok(Self {
            inner: CacheInner::Redis(connection),
        })
    }

    pub async fn get(&self, key: &str) -> Option<String> {
        match &self.inner {
            CacheInner::Memory(entries) => {
                let entries = entries.lock().await;
                match entries.get(key) {
                    Some((value, expires)) if *expires > Instant::now() => Some(value.clone()),
                    _ => None,
                }
            }
            CacheInner::Redis(connection) => {
                let mut connection = connection.clone();
                match connection.get::<_, Option<String>>(key).await {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::warn!(error = %err, "redis cache get failed");
                        None
                    }
                }
            }
        }
    }

    pub async fn set(&self, key: String, value: String, ttl: Duration) {
        match &self.inner {
            CacheInner::Memory(entries) => {
                let mut entries = entries.lock().await;
                entries.insert(key, (value, Instant::now() + ttl));
                if entries.len() > 20_000 {
                    let now = Instant::now();
                    entries.retain(|_, (_, expires)| *expires > now);
                }
            }
            CacheInner::Redis(connection) => {
                let mut connection = connection.clone();
                if let Err(err) = connection
                    .set_ex::<_, _, ()>(key, value, ttl.as_secs().max(1))
                    .await
                {
                    tracing::warn!(error = %err, "redis cache set failed");
                }
            }
        }
    }

    pub async fn invalidate_prefix(&self, prefix: &str) {
        match &self.inner {
            CacheInner::Memory(entries) => {
                let mut entries = entries.lock().await;
                entries.retain(|key, _| !key.starts_with(prefix));
            }
            CacheInner::Redis(connection) => {
                let mut connection = connection.clone();
                let pattern = format!("{prefix}*");
                let mut cursor = 0_u64;
                loop {
                    let result: anyhow::Result<(u64, Vec<String>)> = async {
                        let result = redis::cmd("SCAN")
                            .arg(cursor)
                            .arg("MATCH")
                            .arg(&pattern)
                            .arg("COUNT")
                            .arg(1000)
                            .query_async::<(u64, Vec<String>)>(&mut connection)
                            .await?;
                        Ok(result)
                    }
                    .await;
                    let Ok((next, keys)) = result else { break };
                    if !keys.is_empty() {
                        let mut unlink = connection.clone();
                        if let Err(err) = redis::cmd("UNLINK")
                            .arg(keys)
                            .query_async::<()>(&mut unlink)
                            .await
                        {
                            tracing::warn!(error = %err, "redis cache invalidation failed");
                            break;
                        }
                    }
                    cursor = next;
                    if cursor == 0 {
                        break;
                    }
                }
            }
        }
    }

    pub async fn ready(&self) -> anyhow::Result<()> {
        match &self.inner {
            CacheInner::Memory(_) => Ok(()),
            CacheInner::Redis(connection) => {
                let mut connection = connection.clone();
                let reply: String = redis::cmd("PING")
                    .query_async(&mut connection)
                    .await
                    .context("redis ping failed")?;
                if reply.eq_ignore_ascii_case("PONG") {
                    Ok(())
                } else {
                    Err(anyhow!("unexpected redis ping reply: {reply}"))
                }
            }
        }
    }
}

#[derive(Clone)]
enum RateLimiterInner {
    Memory(MemoryBuckets),
    Redis(ConnectionManager),
}

#[derive(Clone)]
pub struct RateLimiter {
    inner: RateLimiterInner,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            inner: RateLimiterInner::Memory(Arc::default()),
        }
    }
}

impl RateLimiter {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(url).context("invalid REDIS_URL")?;
        let connection = ConnectionManager::new(client)
            .await
            .context("failed to connect to Redis")?;
        Ok(Self {
            inner: RateLimiterInner::Redis(connection),
        })
    }

    pub async fn allow(&self, key: &str, max: usize) -> anyhow::Result<RateDecision> {
        match &self.inner {
            RateLimiterInner::Memory(buckets) => {
                let mut buckets = buckets.lock().await;
                let now = Instant::now();
                let entry = buckets.entry(key.to_string()).or_insert((max, now));
                let elapsed = now.duration_since(entry.1).as_secs_f64();
                entry.0 =
                    ((entry.0 as f64) + elapsed * (max as f64 / 60.0)).min(max as f64) as usize;
                entry.1 = now;
                if entry.0 > 0 {
                    entry.0 -= 1;
                    Ok(RateDecision {
                        allowed: true,
                        remaining: entry.0,
                    })
                } else {
                    Ok(RateDecision {
                        allowed: false,
                        remaining: 0,
                    })
                }
            }
            RateLimiterInner::Redis(connection) => {
                let script = Script::new(
                    "local current = redis.call('INCR', KEYS[1])
                      if current == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end
                      return current",
                );
                let mut connection = connection.clone();
                let current: i64 = script
                    .key(key)
                    .arg(60_i64)
                    .invoke_async(&mut connection)
                    .await
                    .context("redis rate limit check failed")?;
                let allowed = current <= max as i64;
                Ok(RateDecision {
                    allowed,
                    remaining: max.saturating_sub(current.max(0) as usize),
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateDecision {
    pub allowed: bool,
    pub remaining: usize,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub cache: Cache,
    pub limiter: RateLimiter,
}

impl AppState {
    pub async fn from_config(config: Config) -> anyhow::Result<Self> {
        let pool = connect(&config).await?;
        migrate(&pool).await?;
        let (cache, limiter) = match &config.redis_url {
            Some(url) => (Cache::connect(url).await?, RateLimiter::connect(url).await?),
            None => (Cache::default(), RateLimiter::default()),
        };
        Ok(Self {
            pool,
            config,
            cache,
            limiter,
        })
    }

    pub async fn ready(&self) -> anyhow::Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .context("database readiness check failed")?;
        self.cache.ready().await?;
        Ok(())
    }
}

pub async fn connect(config: &Config) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(Duration::from_secs(config.db_acquire_timeout_secs))
        .connect(&config.database_url)
        .await?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    seed(pool).await?;
    Ok(())
}

async fn seed(pool: &PgPool) -> anyhow::Result<()> {
    let rows = [
        ("L1", "Algerian Ligue 1", 1, "lfp"),
        ("L2", "Algerian Ligue 2", 2, "lnfa"),
        ("L3", "Algerian Inter-regions (D3)", 3, "lirf"),
    ];
    for (code, name, tier, source) in rows {
        sqlx::query(
            "INSERT INTO competitions (code, name, tier, source) VALUES ($1,$2,$3,$4)
             ON CONFLICT (code) DO UPDATE SET name = EXCLUDED.name, tier = EXCLUDED.tier, source = EXCLUDED.source",
        )
        .bind(code)
        .bind(name)
        .bind(tier)
        .bind(source)
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Cache, RateLimiter};
    use std::time::Duration;

    #[tokio::test]
    async fn memory_cache_round_trip() {
        let cache = Cache::default();
        cache
            .set("k".into(), "v".into(), Duration::from_secs(60))
            .await;
        assert_eq!(cache.get("k").await.as_deref(), Some("v"));
        cache.invalidate_prefix("k").await;
        assert_eq!(cache.get("k").await, None);
        assert!(cache.ready().await.is_ok());
    }

    #[tokio::test]
    async fn memory_rate_limiter_enforces_max() {
        let limiter = RateLimiter::default();
        assert!(limiter.allow("key", 2).await.unwrap().allowed);
        assert!(limiter.allow("key", 2).await.unwrap().allowed);
        assert!(!limiter.allow("key", 2).await.unwrap().allowed);
    }
}
