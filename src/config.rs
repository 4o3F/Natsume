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
    /// Server port
    pub port: u16,
    /// Token for sync authorization
    pub token: String,
    /// Enable bind service
    pub enable_bind: bool,
    /// Allow bind update
    pub enable_bind_update: bool,
    /// Enable sync service
    pub enable_sync: bool,
    /// Enable static file service
    pub enable_static_file: bool,
    /// Password for panel
    pub panel_token: String
}

#[cfg(feature = "client")]
#[derive(Deserialize, Debug)]
pub struct ClientConfig {
    /// Whether skip IP match check for bind,
    /// this need to be set to true when there are NAT between client and server.
    pub skip_ip_check: bool,
    /// Address for Natsume server, make sure it does not end with a slash
    pub server_addr: String,
    /// Path for client Caddyfile, the default one is /etc/caddy/Caddyfile
    pub caddyfile: String,
    /// Adress for DomJudge server, this will be inserted into Caddyfile as reverse proxy upstream,
    /// make sure it does not end with a slash
    pub domjudge_addr: String,
    /// Token for sync authorization, this is used to prevent unauthorized access to Natsume endpoint
    pub token: String,
    /// The system user for player, will be recreated when running clean command
    pub player_user: String,
    /// System user password for player
    pub player_user_password: String
}
