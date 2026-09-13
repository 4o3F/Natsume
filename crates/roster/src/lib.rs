//! Fixed roster workbook input and local school logo validation.
//!
//! This crate owns no database, organization IDs, server configuration or network
//! connections. Diagnostics never contain workbook cell contents.

mod error;
mod logos;
mod workbook;

pub use error::{InputError, InputErrorKind};
pub use logos::{LogoDirectory, LogoError, LogoMatch, validate_logo};
pub use workbook::{MAX_WORKBOOK_BYTES, Organization, Roster, Team, parse_xlsx};
