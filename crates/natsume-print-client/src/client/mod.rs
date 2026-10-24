//! Client execution and recovery, separate from the native Windows boundary.
mod api;
mod application;
mod config;
mod engine;
mod error;
#[cfg(any(windows, test))]
mod gui;
mod pairing;
mod state;
mod wire;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use gui::run as run_gui;
#[cfg(windows)]
pub use windows::run;

pub use config::Config;
pub use error::ClientError;
