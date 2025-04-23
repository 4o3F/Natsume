use std::fs::{self, OpenOptions};

use clap::{Parser, Subcommand};
use once_cell::sync::OnceCell;
use sha2::{Digest, Sha256};
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

    #[arg(
        long,
        short,
        help = "Path for config file",
        global = true,
        default_value = "/etc/natsume/config.toml"
    )]
    config: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the server
    #[cfg(feature = "server")]
    Serve {},

    /// Load ID info into database
    #[cfg(feature = "server")]
    Load {
        #[arg(short, long, help = "CSV file containing id,username,password")]
        data_path: String,
    },

    /// Bind the device to a ID
    #[cfg(feature = "client")]
    Bind {
        #[arg(long, short, help = "ID for this device")]
        id: String,

        #[arg(long, short, help = "Skip check", default_value = "false", action = clap::ArgAction::SetTrue)]
        skip_check: bool,
    },

    /// Sync player info to device
    #[cfg(feature = "client")]
    Sync {},
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

    // Do config parse
    tracing::info!("Parsing config file...");
    let config = fs::read_to_string(cli.config).unwrap_or_log();
    let mut config: config::Config = toml::from_str(&config).unwrap_or_log();
    // Pre compute token hash
    #[cfg(feature = "server")]
    {
        let mut hasher = Sha256::new();
        hasher.update(config.server.token.clone());
        let hashed_token = hasher.finalize();
        let hashed_token = hex::encode(hashed_token);
        config.server.token = hashed_token;
        tracing::info!("Server token set to {}", config.server.token);
    }
    #[cfg(feature = "client")]
    {
        let mut hasher = Sha256::new();
        hasher.update(config.client.token.clone());
        let hashed_token = hasher.finalize();
        let hashed_token = hex::encode(hashed_token);
        config.client.token = hashed_token;
        tracing::info!("Client token set to {}", config.client.token);
    }

    #[cfg(feature = "client")]
    {
        if client::check_permission(config.client.caddyfile.clone()) {
            tracing::info!("Client priviledge correct, procedding.")
        } else {
            tracing::error!("Client do not have root exec priviledge!!!");
            return;
        }
    }

    GLOBAL_CONFIG.set(config).unwrap_or_log();

    match cli.command {
        #[cfg(feature = "server")]
        Commands::Serve {} => {
            tracing::info!("Starting in server mode");
            server::serve().unwrap_or_log();
        }
        #[cfg(feature = "server")]
        Commands::Load { data_path } => {
            tracing::info!("Staring data load");
            server::load_data(data_path).unwrap_or_log();
        }
        #[cfg(feature = "client")]
        Commands::Bind { id, skip_check } => {
            client::bind_ip(id, skip_check).unwrap_or_log();
        }
        #[cfg(feature = "client")]
        Commands::Sync {} => {
            client::sync_info().unwrap_or_log();
        }
    }
}
