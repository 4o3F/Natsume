mod control_keys;
mod devices;

use diesel::{RunQueryDsl, dsl::sql, sql_types::BigInt};

use crate::db::{PersistenceError, Transaction};

pub(in crate::component::device) use self::{control_keys::*, devices::*};

pub(super) fn current_unix_ms(transaction: &mut Transaction<'_>) -> Result<i64, PersistenceError> {
    diesel::select(sql::<BigInt>("CAST(unixepoch('subsec') * 1000 AS INTEGER)"))
        .get_result::<i64>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}
