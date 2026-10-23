//! Client execution and recovery, separate from the native Windows boundary.
mod api;
mod config;
mod engine;
mod error;
mod pairing;
mod state;
mod wire;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::run;

pub use config::Config;
pub use error::ClientError;
