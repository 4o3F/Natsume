#![forbid(unsafe_code)]
//! Natsume control server startup.

pub mod commands;
pub(crate) mod component;
pub mod config;
pub(crate) mod db;
pub(crate) mod device_control;
#[path = "../diesel/schema.rs"]
pub(crate) mod diesel_schema;
pub(crate) mod http;
pub(crate) mod logging;
pub mod openapi;
pub(crate) mod server_state;
pub(crate) mod tls;
pub(crate) mod vault;

pub use commands::router;
