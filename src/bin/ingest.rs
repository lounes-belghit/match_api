use clap::{Parser, Subcommand};
use matchs_api::{AppState, Config};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Debug, Parser)]
#[command(name = "matchs-ingest", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Backfill {
        #[arg(long, short, default_value_t = 3)]
        seasons: usize,
    },
    Live,
    Events {
        #[arg(long, short, default_value_t = 200)]
        limit: i64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env()?;
    let state = AppState::from_config(config).await?;

    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Backfill { seasons: 3 }) {
        Command::Backfill { seasons } => matchs_api::ingest::backfill(&state, seasons).await?,
        Command::Live => matchs_api::ingest::refresh_live(&state).await?,
        Command::Events { limit } => matchs_api::ingest::ingest_events(&state, limit).await?,
    }
    Ok(())
}
