mod bind;
mod check;
mod sync;

pub use bind::bind_ip;
pub use check::{check_permission, check_prerequisite};
use serde::Deserialize;
pub use sync::sync_info;

#[derive(Deserialize)]
struct ErrorResponse {
    msg: String,
    error: String,
}
