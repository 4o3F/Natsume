//! Photo client business logic, independent of graphical interfaces.
#![cfg(windows)]

pub mod capture;
pub mod capture_session;
mod diesel_schema;
mod hot_folder;
pub mod initialization;
pub mod roster;
pub mod workspace;
