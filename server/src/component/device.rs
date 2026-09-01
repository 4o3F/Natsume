mod db;
mod enrollment;
mod lifecycle;
mod types;

use crate::db::Database;

pub(crate) use self::types::{DeviceError, DeviceId};

pub(crate) struct DeviceComponent {
    #[allow(dead_code)]
    database: Database,
}

impl DeviceComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }
}
