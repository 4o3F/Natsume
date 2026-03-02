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
    /// Path to PEM-encoded CA certificate for issuing server TLS certificates
    pub tls_ca_cert_path: String,
    /// Path to PEM-encoded CA private key for issuing server TLS certificates
    pub tls_ca_key_path: String,
    /// Path to PEM-encoded issued server certificate chain (leaf + CA)
    #[serde(default = "default_tls_cert_path")]
    pub tls_cert_path: String,
    /// Path to PEM-encoded issued server private key
    #[serde(default = "default_tls_key_path")]
    pub tls_key_path: String,
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
    pub panel_token: String,
}

#[cfg(feature = "server")]
fn default_tls_cert_path() -> String {
    "./cert/server-cert.pem".to_string()
}

#[cfg(feature = "server")]
fn default_tls_key_path() -> String {
    "./cert/server-key.pem".to_string()
}

#[cfg(feature = "client")]
#[derive(Deserialize, Debug)]
pub struct ClientConfig {
    /// Whether skip IP match check for bind,
    /// this need to be set to true when there are NAT between client and server.
    pub skip_ip_check: bool,
    /// Address for Natsume server, make sure it does not end with a slash
    pub server_addr: String,
    /// Path to PEM-encoded CA public certificate used to verify the server certificate
    pub tls_ca_cert_path: String,
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
    pub player_user_password: String,
}
