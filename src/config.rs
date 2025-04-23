use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Config {
    #[cfg(feature = "server")]
    pub server: ServerConfig,
    #[cfg(feature = "client")]
    pub client: ClientConfig,
}

#[cfg(feature = "server")]
#[derive(Deserialize, Debug)]
pub struct ServerConfig {
    pub port: u16,
    pub token: String
}

#[cfg(feature = "client")]
#[derive(Deserialize, Debug)]
pub struct ClientConfig {
    pub server_addr: String,
    pub token: String
}
