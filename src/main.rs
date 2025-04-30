use std::{
    fs::{self, OpenOptions},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use once_cell::sync::OnceCell;
use sha2::{Digest, Sha256};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use tracing_unwrap::{OptionExt, ResultExt};

#[macro_use]
extern crate version;

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
    },

    /// Sync player info to device
    #[cfg(feature = "client")]
    Sync {},

    /// Clean player user data
    #[cfg(feature = "client")]
    Clean {},

    /// Start monitoring device and report
    #[cfg(feature = "client")]
    Monitor {},

    /// Deal with user session
    #[cfg(feature = "client")]
    Session {
        #[arg(value_enum, help = "Operation for session (terminate, autologin)")]
        operation: SessionOperation,
    },
}

#[derive(clap::ValueEnum, Clone)]
enum SessionOperation {
    /// Terminate the user session
    Terminate,
    /// Auto login to the given user session
    AutoLogin,
}

fn main() -> ExitCode {
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
    tracing::info!("Currently in client mode version {}", version!());
    #[cfg(feature = "server")]
    tracing::info!("Currently in server mode version {}", version!());

    // Do config parse
    tracing::info!("Parsing config file...");
    let config = fs::read_to_string(cli.config).unwrap_or_log();
    let mut config: config::Config = toml::from_str(&config).unwrap_or_log();

    #[cfg(feature = "client")]
    {
        // Bind command should be run in non priviledged environment
        if !matches!(cli.command, Commands::Bind { id: _ }) {
            if client::check_permission(config.client.caddyfile.clone()) {
                tracing::info!("Client priviledge correct, procedding.")
            } else {
                tracing::error!("Client do not have root exec priviledge!!!");
                return ExitCode::FAILURE;
            }
        }

        if client::check_prerequisite() {
            tracing::info!("Client prerequisite matched, procedding.")
        } else {
            tracing::error!("Client prerequisite does not match!!!");
            return ExitCode::FAILURE;
        }
    }

    // Pre compute token hash
    #[cfg(feature = "server")]
    {
        let mut hasher = Sha256::new();
        hasher.update(config.server.token.clone());
        let hashed_token = hasher.finalize();
        let hashed_token = hex::encode(hashed_token);
        config.server.token = hashed_token;
        tracing::info!("Server sync token set to {}", config.server.token);

        let mut hasher = Sha256::new();
        hasher.update(config.server.panel_token.clone());
        let hashed_token = hasher.finalize();
        let hashed_token = hex::encode(hashed_token);
        config.server.panel_token = hashed_token;
        tracing::info!("Server panel token set to {}", config.server.panel_token);
    }
    #[cfg(feature = "client")]
    {
        let mut hasher = Sha256::new();
        hasher.update(config.client.token.clone());
        let hashed_token = hasher.finalize();
        let hashed_token = hex::encode(hashed_token);
        config.client.token = hashed_token;
        #[cfg(debug_assertions)]
        {
            tracing::info!("Client token set to {}", config.client.token);
        }
    }

    GLOBAL_CONFIG
        .set(config)
        .expect_or_log("Failed to set global config!");

    match cli.command {
        #[cfg(feature = "server")]
        Commands::Serve {} => {
            tracing::info!("Starting in server mode");
            match server::serve() {
                Ok(_) => {
                    tracing::info!("Server shutdown gracefully!");
                    return ExitCode::SUCCESS;
                }
                Err(err) => {
                    tracing::error!("Server failed with error {}", err);
                    return ExitCode::FAILURE;
                }
            }
        }
        #[cfg(feature = "server")]
        Commands::Load { data_path } => {
            tracing::info!("Staring data load");
            match server::load_data(data_path) {
                Ok(_) => {
                    tracing::info!("Data successfully loaded into database!");
                    return ExitCode::SUCCESS;
                }
                Err(err) => {
                    tracing::error!("Data load failed with error {}", err);
                    return ExitCode::FAILURE;
                }
            }
        }
        #[cfg(feature = "client")]
        Commands::Bind { id } => match client::bind_ip(id) {
            Ok(_) => {
                tracing::info!("Bind success!");
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                tracing::error!("Bind failed with error {}", err);
                return ExitCode::FAILURE;
            }
        },
        #[cfg(feature = "client")]
        Commands::Sync {} => match client::sync_info() {
            Ok(_) => {
                tracing::info!("Sync success!");
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                tracing::error!("Sync failed with error {}", err);
                return ExitCode::FAILURE;
            }
        },
        #[cfg(feature = "client")]
        Commands::Clean {} => match client::clean_user() {
            Ok(_) => {
                tracing::info!("Clean success!");
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                tracing::error!("Clean failed with error {}", err);
                return ExitCode::FAILURE;
            }
        },
        #[cfg(feature = "client")]
        Commands::Monitor {} => match client::do_monitor() {
            Ok(_) => {
                tracing::info!("Monitor graceful shutdown!");
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                tracing::error!("Monitor failed with error {}", err);
                return ExitCode::FAILURE;
            }
        },
        #[cfg(feature = "client")]
        Commands::Session { operation } => match operation {
            SessionOperation::Terminate => match client::terminate_sessions() {
                Ok(_) => {
                    tracing::info!("Terminate user session successful");
                    return ExitCode::SUCCESS;
                }
                Err(err) => {
                    tracing::error!("Terminate user session failed with error {}", err);
                    return ExitCode::FAILURE;
                }
            },
            SessionOperation::AutoLogin => match client::autologin_session() {
                Ok(_) => {
                    tracing::info!("AutoLogin for user session successful");
                    return ExitCode::SUCCESS;
                }
                Err(err) => {
                    tracing::error!("AutoLogin user session failed with error {}", err);
                    return ExitCode::FAILURE;
                }
            },
        },
    }
}
