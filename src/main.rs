use std::fs::{self, OpenOptions};

use clap::{Parser, Subcommand};
use once_cell::sync::OnceCell;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use tracing_unwrap::{OptionExt, ResultExt};

#[cfg(feature = "client")]
mod client;
mod config;
#[cfg(feature = "server")]
mod server;

static GLOBAL_CONFIG: OnceCell<config::Config> = OnceCell::new();

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, short, help = "Path for config file")]
    config: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the server
    Serve {},

    /// Bind the device to a ID
    Bind {
        #[arg(long, short, help = "ID for this device")]
        id: String,
    },
}

fn main() {
    // Create logs dir
    fs::create_dir_all("logs").unwrap();

    let log_file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(format!(
            "logs/{}.log",
            chrono::Utc::now()
                .with_timezone(&chrono::FixedOffset::east_opt(8 * 60 * 60).unwrap_or_log())
                .format("%Y-%m-%d")
        ))
        .unwrap_or_log();

    let console_layer = tracing_subscriber::fmt::layer().with_level(true);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(log_file)
        .with_ansi(true)
        .with_level(true);

    let filter_layer = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(console_layer)
        .with(file_layer)
        .init();

    let cli = Cli::parse();

    #[cfg(feature = "client")]
    {
        if client::check_suid() {
            tracing::info!("Client priviledge correct, procedding.")
        } else {
            tracing::error!("Client do not have root exec priviledge!!!");
            return;
        }
    }

    // Do config parse
    tracing::info!("Parsing config file...");
    let config = fs::read_to_string(cli.config).unwrap_or_log();
    let config: config::Config = toml::from_str(&config).unwrap_or_log();
    GLOBAL_CONFIG.set(config).unwrap_or_log();
    match cli.command {
        Commands::Serve {} => {
            #[cfg(feature = "server")]
            {
                tracing::info!("Starting in server mode");
                server::serve().unwrap_or_log();
            }
            #[cfg(feature = "client")]
            {
                tracing::error!("Client should not call serve command!");
            }
        }
        Commands::Bind { id } => {
            #[cfg(feature = "client")]
            {
                client::bind_ip(id).unwrap_or_log();
            }
            #[cfg(feature = "server")]
            {
                tracing::error!("Server should not call bind command!")
            }
        }
    }
}
