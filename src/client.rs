mod bind;
mod permission;
mod sync;

pub use bind::bind_ip;
pub use permission::check_permission;
use serde::Deserialize;
pub use sync::sync_info;

#[derive(Deserialize)]
struct ErrorResponse {
    msg: String,
    error: String,
}
