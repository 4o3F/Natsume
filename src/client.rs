mod bind;
mod permission;

pub use bind::bind_ip;
pub use permission::check_permission;
use serde::Deserialize;

#[derive(Deserialize)]
struct ErrorResponse {
    msg: String
}