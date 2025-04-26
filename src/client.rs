mod bind;
mod check;
mod clean;
mod sync;
mod monitor;
mod session;

pub use bind::bind_ip;
pub use check::{check_permission, check_prerequisite};
pub use clean::clean_user;
pub use sync::sync_info;
pub use monitor::do_monitor;
pub use session::{terminate_sessions, autologin_session};

use serde::Deserialize;

#[derive(Deserialize)]
struct ErrorResponse {
    msg: String,
    error: String,
}
