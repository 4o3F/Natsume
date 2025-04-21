use actix_web::{App, HttpServer};
use serde::Serialize;
use tracing_unwrap::OptionExt;

mod services;
mod database;

#[actix_web::main]
pub async fn serve() -> std::io::Result<()> {
    let server_config = super::GLOBAL_CONFIG.get().unwrap_or_log();

    HttpServer::new(|| App::new().service(services::get_ip))
        .bind(("0.0.0.0", server_config.server.port))?
        .run()
        .await
}

#[derive(Serialize)]
struct ErrorResponse {
    msg: String,
}

impl ErrorResponse {
    fn new(msg: String) -> Self {
        ErrorResponse { msg }
    }
}
