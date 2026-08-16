use std::{env, path::PathBuf};

use natsume_server::{commands, config::ServerConfig};
use snafu::Snafu;

const CONFIG_PATH_ENVIRONMENT: &str = "NATSUME_TEST_SERVER_CONFIG";

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("bootstrap driver configuration is unavailable"))]
    Arguments,

    #[snafu(display("bootstrap driver configuration is invalid"))]
    Configuration,

    #[snafu(display("server bootstrap failed"))]
    Bootstrap,
}

#[tokio::main]
#[snafu::report]
async fn main() -> Result<(), Error> {
    let config_path = env::var_os(CONFIG_PATH_ENVIRONMENT)
        .map(PathBuf::from)
        .ok_or(Error::Arguments)?;
    let config = ServerConfig::load_from(&config_path).map_err(|_| Error::Configuration)?;
    commands::bootstrap(config)
        .await
        .map_err(|_| Error::Bootstrap)
}
