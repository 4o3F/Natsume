#![forbid(unsafe_code)]
//! Natsume control server startup.

pub(crate) mod audit;
pub mod commands;
pub(crate) mod component;
pub mod config;
pub(crate) mod db;
pub(crate) mod error;
pub(crate) mod http;
pub(crate) mod logging;
pub mod openapi;
pub(crate) mod schema;
pub(crate) mod tls;
pub(crate) mod vault;

pub use commands::router;
