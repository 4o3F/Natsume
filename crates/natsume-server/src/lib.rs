// Specta collection allows its generated unsafe constructors locally.
// Handwritten unsafe code remains denied throughout this crate.
#![deny(unsafe_code)]
#![cfg(target_os = "linux")]
//! Natsume control server startup.

pub(crate) mod background;
pub mod commands;
pub(crate) mod component;
pub mod config;
pub(crate) mod db;
pub(crate) mod device_control;
#[path = "../diesel/business/schema.rs"]
pub(crate) mod diesel_schema;
pub(crate) mod http;
pub(crate) mod journal;
pub(crate) mod journal_query;
#[path = "../diesel/journal/schema.rs"]
pub(crate) mod journal_schema;
pub(crate) mod lifecycle;
pub(crate) mod logging;
pub(crate) mod server_state;
pub(crate) mod task;
pub(crate) mod tls;
pub(crate) mod vault;

pub use commands::router;

#[cfg(test)]
pub(crate) mod test_error;
