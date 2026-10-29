//! First-install setup for Natsume Server, providing PKI, configuration, and deployment primitives.
#![cfg(target_os = "linux")]

pub mod config;
pub mod deployment;
pub mod pki;

pub mod account;
pub mod cli;
pub mod handoff;
pub mod health;
pub mod interaction;
pub mod preflight;
pub mod probe;
pub mod process;
pub mod service;

pub mod host;
pub mod wizard;
