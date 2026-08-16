#![forbid(unsafe_code)]
//! Natsume control server startup.

pub mod application;
pub mod audit;
pub mod commands;
pub mod config;
pub mod db;
pub mod error;
mod http;
mod logging;
pub mod openapi;
mod tls;
mod vault;

pub use commands::router;
