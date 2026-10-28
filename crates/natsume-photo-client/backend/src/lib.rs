//! Photo client business logic, independent of graphical interfaces.
#![cfg(windows)]

pub mod capture;
mod diesel_schema;
pub mod hot_folder;
pub mod initialization;
pub mod roster;
pub mod workspace;
