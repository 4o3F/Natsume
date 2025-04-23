mod bind;
mod permission;
mod sync;

pub use bind::bind_ip;
pub use permission::check_permission;
pub use sync::sync_info;
use serde::Deserialize;

#[derive(Deserialize)]
struct ErrorResponse {
    msg: String
}