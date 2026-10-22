use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, sql_types::BigInt};
use snafu::{OptionExt as _, ensure};

use super::super::{
    AuthenticationSnafu, ConflictSnafu, NotFoundSnafu, PrintError, PrintPrinter, hash_matches,
};
use crate::{db::Transaction, diesel_schema::print_printers as printers};

type PrinterRow = (String, String, Option<String>, i32, Option<i64>);

pub(in crate::component::print) fn now(tx: &mut Transaction<'_>) -> Result<i64, PrintError> {
    // SQLite owns time for expiry; runtime values are never interpolated into SQL.
    Ok(diesel::select(diesel::dsl::sql::<BigInt>(
        "CAST(unixepoch('subsec') * 1000 AS INTEGER)",
    ))
    .get_result(tx.connection())?)
}

fn printer(row: PrinterRow) -> PrintPrinter {
    PrintPrinter {
        printer_id: row.0,
        name: row.1,
        client_id: row.2,
        client_confirmed: row.3 != 0,
        pairing_expires_at_unix_ms: row.4,
    }
}

pub(in crate::component::print) fn insert(
    tx: &mut Transaction<'_>,
    id: &str,
    name: &str,
) -> Result<(), PrintError> {
    diesel::insert_into(printers::table)
        .values((printers::printer_id.eq(id), printers::name.eq(name)))
        .execute(tx.connection())?;
    Ok(())
}

pub(in crate::component::print) fn find(
    tx: &mut Transaction<'_>,
    id: &str,
) -> Result<PrintPrinter, PrintError> {
    printers::table
        .select((
            printers::printer_id,
            printers::name,
            printers::client_id,
            printers::client_confirmed,
            printers::pairing_expires_at_unix_ms,
        ))
        .filter(printers::printer_id.eq(id))
        .first::<PrinterRow>(tx.connection())
        .optional()?
        .map(printer)
        .context(NotFoundSnafu)
}

pub(in crate::component::print) fn list(
    tx: &mut Transaction<'_>,
) -> Result<Vec<PrintPrinter>, PrintError> {
    // Lists contain neither code hashes nor client credential material.
    Ok(printers::table
        .select((
            printers::printer_id,
            printers::name,
            printers::client_id,
            printers::client_confirmed,
            printers::pairing_expires_at_unix_ms,
        ))
        .order(printers::printer_id)
        .load::<PrinterRow>(tx.connection())?
        .into_iter()
        .map(printer)
        .collect())
}

pub(in crate::component::print) fn issue_code(
    tx: &mut Transaction<'_>,
    id: &str,
    hash: &[u8],
    now: i64,
) -> Result<i64, PrintError> {
    ensure!(find(tx, id)?.client_id.is_none(), ConflictSnafu);
    let expires = now + 600_000;
    diesel::update(printers::table.filter(printers::printer_id.eq(id)))
        .set((
            printers::pairing_code_hash.eq(Some(hash)),
            printers::pairing_expires_at_unix_ms.eq(Some(expires)),
        ))
        .execute(tx.connection())?;
    Ok(expires)
}

pub(in crate::component::print) fn pair(
    tx: &mut Transaction<'_>,
    code_hash: &[u8],
    client_id: &str,
    token_hash: &[u8],
    now: i64,
) -> Result<String, PrintError> {
    let id = printers::table
        .select(printers::printer_id)
        .filter(printers::pairing_code_hash.eq(code_hash))
        .filter(printers::pairing_expires_at_unix_ms.gt(now))
        .filter(printers::client_id.is_null())
        .first::<String>(tx.connection())
        .optional()?
        .context(AuthenticationSnafu)?;
    // Code consumption and binding share the caller's BEGIN IMMEDIATE transaction.
    diesel::update(printers::table.filter(printers::printer_id.eq(&id)))
        .set((
            printers::pairing_code_hash.eq(None::<&[u8]>),
            printers::pairing_expires_at_unix_ms.eq(None::<i64>),
            printers::client_id.eq(Some(client_id)),
            printers::client_token_hash.eq(Some(token_hash)),
            printers::client_confirmed.eq(0),
        ))
        .execute(tx.connection())?;
    Ok(id)
}

pub(in crate::component::print) fn confirm(
    tx: &mut Transaction<'_>,
    client_id: &str,
    hash: &[u8; 32],
) -> Result<PrintPrinter, PrintError> {
    let (id, saved_hash) = printers::table
        .select((printers::printer_id, printers::client_token_hash))
        .filter(printers::client_id.eq(client_id))
        .first::<(String, Option<Vec<u8>>)>(tx.connection())
        .optional()?
        .context(AuthenticationSnafu)?;
    ensure!(
        saved_hash.is_some_and(|saved| hash_matches(&saved, hash)),
        AuthenticationSnafu
    );
    diesel::update(printers::table.filter(printers::printer_id.eq(&id)))
        .set(printers::client_confirmed.eq(1))
        .execute(tx.connection())?;
    find(tx, &id)
}

pub(in crate::component::print) fn clear_binding(
    tx: &mut Transaction<'_>,
    id: &str,
) -> Result<(), PrintError> {
    let changed = diesel::update(printers::table.filter(printers::printer_id.eq(id)))
        .set((
            printers::pairing_code_hash.eq(None::<&[u8]>),
            printers::pairing_expires_at_unix_ms.eq(None::<i64>),
            printers::client_id.eq(None::<&str>),
            printers::client_token_hash.eq(None::<&[u8]>),
            printers::client_confirmed.eq(0),
        ))
        .execute(tx.connection())?;
    ensure!(changed != 0, NotFoundSnafu);
    Ok(())
}
