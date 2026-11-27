use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use axum::http::{header, HeaderName, HeaderValue, Method};
use clap::{Parser, Subcommand};
use matchs_api::crypto::{generate_api_key, hash_password};
use matchs_api::{api, dashboard, AppState, Config};
use sqlx::FromRow;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Debug, Parser)]
#[command(name = "matchs-server", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
    CheckConfig,
    InitAdmin {
        #[arg(long)]
        email: String,
        #[arg(long, default_value = "Admin")]
        name: String,
        #[arg(long)]
        password_file: Option<PathBuf>,
        #[arg(long)]
        password_stdin: bool,
    },
    CreateApiKey {
        #[arg(long)]
        email: String,
        #[arg(long, default_value = "bootstrap")]
        name: String,
        #[arg(long)]
        scopes: Option<String>,
        #[arg(long)]
        quota: Option<i64>,
    },
    Plans,
    AssignPlan {
        #[arg(long)]
        email: String,
        #[arg(long)]
        plan: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(std::io::stderr),
        )
        .init();

    let config = Config::from_env()?;
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config).await,
        Command::CheckConfig => {
            println!("config ok: {}", config.environment);
            Ok(())
        }
        Command::InitAdmin {
            email,
            name,
            password_file,
            password_stdin,
        } => init_admin(config, email, name, password_file, password_stdin).await,
        Command::CreateApiKey {
            email,
            name,
            scopes,
            quota,
        } => create_key(config, email, name, scopes, quota).await,
        Command::Plans => list_plans(config).await,
        Command::AssignPlan { email, plan } => assign_plan(config, email, plan).await,
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let state = AppState::from_config(config.clone()).await?;
    let origins = config
        .cors_origins
        .iter()
        .map(|origin| {
            HeaderValue::from_str(origin).with_context(|| format!("invalid CORS origin: {origin}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("x-api-key"),
        ])
        .allow_credentials(true)
        .max_age(Duration::from_secs(3600));
    let app = api::router(state.clone())
        .merge(dashboard::router())
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, env = %config.environment, "server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn init_admin(
    config: Config,
    email: String,
    name: String,
    password_file: Option<PathBuf>,
    password_stdin: bool,
) -> anyhow::Result<()> {
    let password = if password_stdin {
        let mut value = String::new();
        std::io::stdin().read_to_string(&mut value)?;
        value.trim_end().to_string()
    } else if let Some(path) = password_file {
        std::fs::read_to_string(path)
            .context("failed to read admin password file")?
            .trim_end()
            .to_string()
    } else {
        std::env::var("ADMIN_PASSWORD")
            .context("provide --password-file, --password-stdin or ADMIN_PASSWORD")?
    };
    if password.len() < 12 {
        anyhow::bail!("admin password must be at least 12 characters");
    }
    let state = AppState::from_config(config).await?;
    let hash = hash_password(&password)?;
    let plan: PlanRow = sqlx::query_as("SELECT * FROM plans WHERE name = 'pro'")
        .fetch_one(&state.pool)
        .await?;
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (email, name, password_hash, plan, quota)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (email) DO UPDATE SET
             password_hash = EXCLUDED.password_hash,
             plan = EXCLUDED.plan,
             quota = EXCLUDED.quota
         RETURNING id",
    )
    .bind(email.trim().to_lowercase())
    .bind(name)
    .bind(hash)
    .bind(&plan.name)
    .bind(plan.monthly_quota)
    .fetch_one(&state.pool)
    .await?;
    tracing::info!(%id, "admin account ready");
    Ok(())
}

async fn create_key(
    config: Config,
    email: String,
    name: String,
    scopes: Option<String>,
    quota: Option<i64>,
) -> anyhow::Result<()> {
    let state = AppState::from_config(config).await?;
    let account: (uuid::Uuid, i64) =
        sqlx::query_as("SELECT id, quota FROM accounts WHERE email = $1")
            .bind(email.trim().to_lowercase())
            .fetch_optional(&state.pool)
            .await?
            .context("account not found")?;
    let account_id = account.0;
    let allowed = ["read:matches", "read:standings", "read:clubs"];
    let mut selected: Vec<String> = scopes
        .unwrap_or_else(|| allowed.join(","))
        .split(',')
        .map(str::trim)
        .filter(|scope| allowed.contains(scope))
        .map(str::to_string)
        .collect();
    selected.sort();
    selected.dedup();
    if selected.is_empty() {
        anyhow::bail!("no valid scopes provided");
    }
    let effective_quota = quota.unwrap_or(account.1);
    if effective_quota <= 0 {
        anyhow::bail!("quota must be positive");
    }
    let generated = generate_api_key();
    sqlx::query(
        "INSERT INTO api_keys (account_id, name, key_prefix, key_hash, scopes, quota)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(account_id)
    .bind(name)
    .bind(&generated.prefix)
    .bind(&generated.hash)
    .bind(&selected)
    .bind(effective_quota)
    .execute(&state.pool)
    .await?;
    println!("{}", generated.raw);
    Ok(())
}

#[derive(Debug, FromRow)]
struct PlanRow {
    name: String,
    monthly_quota: i64,
    rate_per_min: i32,
    description: String,
}

async fn list_plans(config: Config) -> anyhow::Result<()> {
    let state = AppState::from_config(config).await?;
    let plans: Vec<PlanRow> = sqlx::query_as(
        "SELECT name, monthly_quota, rate_per_min, description FROM plans ORDER BY monthly_quota",
    )
    .fetch_all(&state.pool)
    .await?;
    for plan in plans {
        println!(
            "{}\t{} requests/month\t{} requests/min\t{}",
            plan.name, plan.monthly_quota, plan.rate_per_min, plan.description
        );
    }
    Ok(())
}

async fn assign_plan(config: Config, email: String, plan: String) -> anyhow::Result<()> {
    let state = AppState::from_config(config).await?;
    let plan: PlanRow = sqlx::query_as(
        "SELECT name, monthly_quota, rate_per_min, description FROM plans WHERE name = $1",
    )
    .bind(plan)
    .fetch_optional(&state.pool)
    .await?
    .context("unknown plan")?;
    let mut tx = state.pool.begin().await?;
    let account_id: uuid::Uuid = sqlx::query_scalar(
        "UPDATE accounts SET plan = $2, quota = $3 WHERE email = $1 RETURNING id",
    )
    .bind(email.trim().to_lowercase())
    .bind(&plan.name)
    .bind(plan.monthly_quota)
    .fetch_optional(&mut *tx)
    .await?
    .context("account not found")?;
    sqlx::query("UPDATE api_keys SET quota = $2 WHERE account_id = $1")
        .bind(account_id)
        .bind(plan.monthly_quota)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    println!(
        "account {account_id} assigned to {} ({} requests/month, {} requests/min)",
        plan.name, plan.monthly_quota, plan.rate_per_min
    );
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                let _ = signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
