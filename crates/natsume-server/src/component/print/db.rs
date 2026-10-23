pub(super) mod execution;
pub(super) mod printers;

use diesel::{
    BoolExpressionMethods, ExpressionMethods, JoinOnDsl, NullableExpressionMethods,
    OptionalExtension, QueryDsl, RunQueryDsl,
    dsl::{count_star, exists},
    sql_types::BigInt,
};

use super::{
    GenerationFailure, PrintAdmission, PrintDocument, PrintError, PrintQueueEntry,
    PrintRegistration, PrintRequest, PrintSource, PrintState, SourceIdentity,
};
use crate::{
    db::Transaction,
    diesel_schema::{
        account_mappings, accounts, print_documents, print_requests, print_sources, seats, teams,
    },
};

// SQLite's integer SUM returns an integer or NULL; Diesel's generic SUM uses Numeric.
#[diesel::declare_sql_function]
extern "SQL" {
    #[aggregate]
    #[sql_name = "sum"]
    fn integer_sum(value: BigInt) -> Nullable<BigInt>;
}

impl From<diesel::result::Error> for PrintError {
    fn from(_: diesel::result::Error) -> Self {
        Self::Persistence
    }
}

pub(super) fn insert_source(
    transaction: &mut Transaction<'_>,
    id: &str,
    name: &str,
    hash: &[u8],
) -> Result<(), PrintError> {
    diesel::insert_into(print_sources::table)
        .values((
            print_sources::source_id.eq(id),
            print_sources::name.eq(name),
            print_sources::token_hash.eq(hash),
            print_sources::revoked.eq(0),
        ))
        .execute(transaction.connection())?;
    Ok(())
}

pub(super) fn list_sources(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<PrintSource>, PrintError> {
    // Do not select credential material for a non-secret query.
    Ok(print_sources::table
        .select((
            print_sources::source_id,
            print_sources::name,
            print_sources::revoked,
        ))
        .order(print_sources::source_id)
        .load::<(String, String, i32)>(transaction.connection())?
        .into_iter()
        .map(|(source_id, name, revoked)| PrintSource {
            source_id,
            name,
            revoked: revoked != 0,
        })
        .collect())
}

pub(super) fn list_queue(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<PrintQueueEntry>, PrintError> {
    // One metadata query across sources; polling never opens or hashes PDF files.
    Ok(print_documents::table
        .inner_join(
            print_requests::table.on(print_requests::source_id
                .eq(print_documents::source_id)
                .and(print_requests::request_id.eq(print_documents::request_id))),
        )
        .inner_join(
            print_sources::table.on(print_sources::source_id.eq(print_documents::source_id)),
        )
        .select((
            print_documents::source_id,
            print_sources::name,
            print_documents::request_id,
            print_requests::account,
            print_requests::team_name,
            print_requests::seat_label,
            print_requests::original_filename,
            print_documents::ready_sequence,
            print_documents::pages,
            print_documents::bytes,
        ))
        .order(print_documents::ready_sequence)
        .load::<(
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            i64,
            i64,
            i64,
        )>(transaction.connection())?
        .into_iter()
        .map(
            |(
                source_id,
                source_name,
                request_id,
                account,
                team_name,
                seat_label,
                original_filename,
                ready_sequence,
                pages,
                bytes,
            )| PrintQueueEntry {
                source_id,
                source_name,
                request_id,
                account,
                team_name,
                seat_label,
                original_filename,
                ready_sequence,
                pages,
                bytes,
            },
        )
        .collect())
}

pub(super) fn revoke_source(transaction: &mut Transaction<'_>, id: &str) -> Result<(), PrintError> {
    let updated = diesel::update(print_sources::table.filter(print_sources::source_id.eq(id)))
        .set(print_sources::revoked.eq(1))
        .execute(transaction.connection())?;
    if updated == 0 {
        return Err(PrintError::NotFound);
    }
    Ok(())
}

pub(super) fn authenticate(
    transaction: &mut Transaction<'_>,
    source: &SourceIdentity,
) -> Result<(), PrintError> {
    let row = print_sources::table
        .select((print_sources::token_hash, print_sources::revoked))
        .filter(print_sources::source_id.eq(&source.source_id))
        .first::<(Vec<u8>, i32)>(transaction.connection())
        .optional()?;
    if row.is_some_and(|(hash, revoked)| {
        super::hash_matches(&hash, &source.token_hash) && revoked == 0
    }) {
        Ok(())
    } else {
        Err(PrintError::Authentication)
    }
}

#[derive(diesel::Queryable)]
struct RequestRow {
    request_id: String,
    account_id: String,
    account: String,
    team_name: String,
    seat_label: Option<String>,
    original_filename: String,
    source_sha256: String,
    generation_failure: Option<String>,
}

fn find(
    transaction: &mut Transaction<'_>,
    source: &str,
    id: &str,
) -> Result<Option<RequestRow>, PrintError> {
    Ok(print_requests::table
        .select((
            print_requests::request_id,
            print_requests::account_id,
            print_requests::account,
            print_requests::team_name,
            print_requests::seat_label,
            print_requests::original_filename,
            print_requests::source_sha256,
            print_requests::generation_failure,
        ))
        .filter(print_requests::source_id.eq(source))
        .filter(print_requests::request_id.eq(id))
        .first(transaction.connection())
        .optional()?)
}

pub(super) fn request(
    transaction: &mut Transaction<'_>,
    source: &str,
    id: &str,
) -> Result<PrintRequest, PrintError> {
    let row = find(transaction, source, id)?.ok_or(PrintError::NotFound)?;
    let document = print_documents::table
        .select((
            print_documents::ready_sequence,
            print_documents::sha256,
            print_documents::bytes,
            print_documents::pages,
            print_documents::ahead_tasks,
            print_documents::ahead_pages,
            print_documents::in_progress_tasks,
        ))
        .filter(print_documents::source_id.eq(source))
        .filter(print_documents::request_id.eq(id))
        .first::<(i64, String, i64, i64, i64, i64, i64)>(transaction.connection())
        .optional()?;
    let failure = row
        .generation_failure
        .as_deref()
        .map(GenerationFailure::parse)
        .transpose()?;
    let (state, document, admission) = if let Some((
        ready_sequence,
        sha256,
        bytes,
        pages,
        ahead_tasks,
        ahead_pages,
        in_progress_tasks,
    )) = document
    {
        if failure.is_some() {
            return Err(PrintError::Persistence);
        }
        (
            PrintState::Ready,
            Some(PrintDocument {
                sha256,
                bytes,
                pages,
            }),
            Some(PrintAdmission {
                ready_sequence,
                ahead_tasks,
                ahead_pages,
                in_progress_tasks,
            }),
        )
    } else {
        (
            if failure.is_some() {
                PrintState::GenerationFailed
            } else {
                PrintState::Preparing
            },
            None,
            None,
        )
    };
    Ok(PrintRequest {
        request_id: row.request_id,
        account_id: row.account_id,
        account: row.account,
        team_name: row.team_name,
        seat_label: row.seat_label,
        state,
        document,
        admission,
        generation_failure: failure,
    })
}

pub(super) fn register(
    transaction: &mut Transaction<'_>,
    source: &str,
    input: &PrintRegistration,
) -> Result<PrintRequest, PrintError> {
    if let Some(row) = find(transaction, source, &input.request_id)? {
        if row.account != input.account
            || row.original_filename != input.original_filename
            || row.source_sha256 != input.source_sha256
        {
            return Err(PrintError::Conflict);
        }
        return request(transaction, source, &input.request_id);
    }
    let (account_id, name_zh, name_en, seat_label) = accounts::table
        .inner_join(teams::table)
        .left_join(
            account_mappings::table.on(account_mappings::account_id.eq(accounts::account_id)),
        )
        .left_join(seats::table.on(seats::seat_id.eq(account_mappings::seat_id)))
        .select((
            accounts::account_id,
            teams::name_zh,
            teams::name_en,
            seats::seat_code.nullable(),
        ))
        .filter(accounts::domjudge_username.eq(&input.account))
        .first::<(String, String, String, Option<String>)>(transaction.connection())
        .optional()?
        .ok_or(PrintError::Invalid)?;
    let team_name = if name_zh.is_empty() { name_en } else { name_zh };
    diesel::insert_into(print_requests::table)
        .values((
            print_requests::source_id.eq(source),
            print_requests::request_id.eq(&input.request_id),
            print_requests::account_id.eq(account_id),
            print_requests::account.eq(&input.account),
            print_requests::team_name.eq(team_name),
            print_requests::seat_label.eq(seat_label),
            print_requests::original_filename.eq(&input.original_filename),
            print_requests::source_sha256.eq(&input.source_sha256),
        ))
        .execute(transaction.connection())?;
    request(transaction, source, &input.request_id)
}

pub(super) fn fail(
    transaction: &mut Transaction<'_>,
    source: &str,
    id: &str,
    code: GenerationFailure,
) -> Result<PrintRequest, PrintError> {
    let current = request(transaction, source, id)?;
    match current.state {
        PrintState::Ready => return Err(PrintError::Conflict),
        PrintState::GenerationFailed if current.generation_failure != Some(code) => {
            return Err(PrintError::Conflict);
        }
        PrintState::GenerationFailed => return Ok(current),
        PrintState::Preparing => {}
    }
    diesel::update(
        print_requests::table
            .filter(print_requests::source_id.eq(source))
            .filter(print_requests::request_id.eq(id)),
    )
    .set(print_requests::generation_failure.eq(code.as_str()))
    .execute(transaction.connection())?;
    request(transaction, source, id)
}

pub(super) fn next_document(
    transaction: &mut Transaction<'_>,
    after: i64,
) -> Result<Option<(i64, String, i64)>, PrintError> {
    Ok(print_documents::table
        .select((
            print_documents::ready_sequence,
            print_documents::sha256,
            print_documents::bytes,
        ))
        .filter(print_documents::ready_sequence.gt(after))
        .order(print_documents::ready_sequence)
        .first(transaction.connection())
        .optional()?)
}

pub(super) fn admit(
    transaction: &mut Transaction<'_>,
    source: &str,
    id: &str,
    bytes: i64,
    pages: i64,
    hash: &str,
) -> Result<PrintRequest, PrintError> {
    let current = request(transaction, source, id)?;
    if let Some(document) = &current.document {
        // Exact file comparison has already completed outside the write transaction.
        return if document.sha256 == hash && document.bytes == bytes {
            Ok(current)
        } else {
            Err(PrintError::Conflict)
        };
    }
    if current.state != PrintState::Preparing {
        return Err(PrintError::Conflict);
    }
    let identity_exists = diesel::select(exists(
        accounts::table
            .filter(accounts::account_id.eq(&current.account_id))
            .filter(accounts::domjudge_username.eq(&current.account)),
    ))
    .get_result::<bool>(transaction.connection())?;
    if !identity_exists {
        return Err(PrintError::Conflict);
    }
    let now = printers::now(transaction)?;
    execution::expire(transaction, now)?;
    // Snapshot only tasks eligible ahead of this admission, not past or active attempts.
    let (ahead_tasks, ahead_pages) = print_documents::table
        .inner_join(crate::diesel_schema::print_jobs::table)
        .filter(crate::diesel_schema::print_jobs::state.eq_any(["queued", "waiting"]))
        .filter(crate::diesel_schema::print_jobs::available_at_unix_ms.le(now))
        .select((count_star(), integer_sum(print_documents::pages)))
        .get_result::<(i64, Option<i64>)>(transaction.connection())?;
    let in_progress_tasks = crate::diesel_schema::print_jobs::table
        .filter(crate::diesel_schema::print_jobs::state.eq("active"))
        .count()
        .get_result::<i64>(transaction.connection())?;
    diesel::insert_into(print_documents::table)
        .values((
            print_documents::source_id.eq(source),
            print_documents::request_id.eq(id),
            print_documents::sha256.eq(hash),
            print_documents::bytes.eq(bytes),
            print_documents::pages.eq(pages),
            print_documents::ahead_tasks.eq(ahead_tasks),
            print_documents::ahead_pages.eq(ahead_pages.unwrap_or(0)),
            print_documents::in_progress_tasks.eq(in_progress_tasks),
        ))
        .execute(transaction.connection())?;
    let sequence = print_documents::table
        .filter(print_documents::source_id.eq(source))
        .filter(print_documents::request_id.eq(id))
        .select(print_documents::ready_sequence)
        .first::<i64>(transaction.connection())?;
    execution::enqueue(transaction, sequence, now)?;
    request(transaction, source, id)
}
