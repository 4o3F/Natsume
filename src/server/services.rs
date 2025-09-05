mod bind;
mod ip;
mod panel;
mod report;
mod status;
mod sync;
use std::future::Ready;

use actix_web::{FromRequest, HttpRequest, error::ErrorUnauthorized, dev::Payload};
pub use bind::bind_id;
pub use bind::remove_bind;
pub use ip::get_ip;
pub use panel::spa_handler;
pub use report::report_status;
pub use status::get_status;
pub use sync::sync_info;

pub struct Authenticated;

impl FromRequest for Authenticated {
    type Error = actix_web::Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        let config = crate::GLOBAL_CONFIG.get().expect("Config not initialized");

        match req.headers().get("token") {
            Some(header_value) => {
                if let Ok(token) = header_value.to_str() {
                    if token == config.server.panel_token {
                        return std::future::ready(Ok(Authenticated));
                    }
                }
                std::future::ready(Err(ErrorUnauthorized("Invalid token")))
            }
            None => std::future::ready(Err(ErrorUnauthorized("Missing token header"))),
        }
    }
}
