mod bind;
mod check;
mod clean;
mod sync;

pub use bind::bind_ip;
pub use check::{check_permission, check_prerequisite};
pub use clean::clean_user;
pub use sync::sync_info;

use serde::Deserialize;

#[derive(Deserialize)]
struct ErrorResponse {
    msg: String,
    error: String,
}
