//! First-install setup for Natsume Server, providing PKI, configuration, and deployment primitives in P2.
#![cfg(target_os = "linux")]

pub mod config;
pub mod deployment;
pub mod pki;
