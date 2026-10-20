use diesel::{
    OptionalExtension as _, QueryableByName, RunQueryDsl as _, sql_query,
    sql_types::{BigInt, Binary, Nullable, Text},
};

use super::{
    GenerationFailure, PrintAdmission, PrintDocument, PrintError, PrintRegistration, PrintRequest,
    PrintSource, PrintState, SourceIdentity,
};
use crate::db::Transaction;

pub(super) fn insert_source(
    tx: &mut Transaction<'_>,
    id: &str,
    name: &str,
    hash: &[u8],
) -> Result<(), PrintError> {
    sql_query(
        "INSERT INTO print_sources (source_id, name, token_hash, revoked) VALUES (?, ?, ?, 0)",
    )
    .bind::<Text, _>(id)
    .bind::<Text, _>(name)
    .bind::<Binary, _>(hash)
    .execute(tx.connection())?;
    Ok(())
}

pub(super) fn list_sources(tx: &mut Transaction<'_>) -> Result<Vec<PrintSource>, PrintError> {
    // Do not select credential material for a non-secret query.
    #[derive(QueryableByName)]
    struct PublicSource {
        #[diesel(sql_type = Text)]
        source_id: String,
        #[diesel(sql_type = Text)]
        name: String,
        #[diesel(sql_type = BigInt)]
        revoked: i64,
    }
    Ok(
        sql_query("SELECT source_id, name, revoked FROM print_sources ORDER BY source_id")
            .load::<PublicSource>(tx.connection())?
            .into_iter()
            .map(|r| PrintSource {
                source_id: r.source_id,
                name: r.name,
                revoked: r.revoked != 0,
            })
            .collect(),
    )
}

pub(super) fn revoke_source(tx: &mut Transaction<'_>, id: &str) -> Result<(), PrintError> {
    let n = sql_query("UPDATE print_sources SET revoked = 1 WHERE source_id = ?")
        .bind::<Text, _>(id)
        .execute(tx.connection())?;
    if n == 0 {
        return Err(PrintError::NotFound);
    }
    Ok(())
}

pub(super) fn authenticate(
    tx: &mut Transaction<'_>,
    source: &SourceIdentity,
) -> Result<(), PrintError> {
    #[derive(QueryableByName)]
    struct Credential {
        #[diesel(sql_type = Binary)]
        token_hash: Vec<u8>,
        #[diesel(sql_type = BigInt)]
        revoked: i64,
    }
    let row = sql_query("SELECT token_hash, revoked FROM print_sources WHERE source_id = ?")
        .bind::<Text, _>(&source.source_id)
        .get_result::<Credential>(tx.connection())
        .optional()?;
    if row.is_some_and(|r| super::hash_matches(&r.token_hash, &source.token_hash) && r.revoked == 0)
    {
        Ok(())
    } else {
        Err(PrintError::Authentication)
    }
}

#[derive(QueryableByName)]
struct RequestRow {
    #[diesel(sql_type = Text)]
    request_id: String,
    #[diesel(sql_type = Text)]
    account_id: String,
    #[diesel(sql_type = Text)]
    account: String,
    #[diesel(sql_type = Text)]
    team_name: String,
    #[diesel(sql_type = Nullable<Text>)]
    seat_label: Option<String>,
    #[diesel(sql_type = Text)]
    original_filename: String,
    #[diesel(sql_type = Text)]
    source_sha256: String,
    #[diesel(sql_type = Nullable<Text>)]
    generation_failure: Option<String>,
}

#[derive(QueryableByName)]
struct DocumentRow {
    #[diesel(sql_type = BigInt)]
    ready_sequence: i64,
    #[diesel(sql_type = Text)]
    sha256: String,
    #[diesel(sql_type = BigInt)]
    bytes: i64,
    #[diesel(sql_type = BigInt)]
    pages: i64,
    #[diesel(sql_type = BigInt)]
    ahead_tasks: i64,
    #[diesel(sql_type = BigInt)]
    ahead_pages: i64,
}

fn find(
    tx: &mut Transaction<'_>,
    source: &str,
    id: &str,
) -> Result<Option<RequestRow>, PrintError> {
    Ok(sql_query("SELECT request_id, account_id, account, team_name, seat_label, original_filename, source_sha256, generation_failure FROM print_requests WHERE source_id = ? AND request_id = ?")
        .bind::<Text, _>(source).bind::<Text, _>(id).get_result(tx.connection()).optional()?)
}

pub(super) fn request(
    tx: &mut Transaction<'_>,
    source: &str,
    id: &str,
) -> Result<PrintRequest, PrintError> {
    let row = find(tx, source, id)?.ok_or(PrintError::NotFound)?;
    let doc = sql_query("SELECT ready_sequence, sha256, bytes, pages, ahead_tasks, ahead_pages FROM print_documents WHERE source_id = ? AND request_id = ?")
        .bind::<Text, _>(source).bind::<Text, _>(id).get_result::<DocumentRow>(tx.connection()).optional()?;
    let failure = row
        .generation_failure
        .as_deref()
        .map(GenerationFailure::parse)
        .transpose()?;
    let (state, document, admission) = if let Some(doc) = doc {
        if failure.is_some() {
            return Err(PrintError::Persistence);
        }
        (
            PrintState::Ready,
            Some(PrintDocument {
                sha256: doc.sha256,
                bytes: doc.bytes,
                pages: doc.pages,
            }),
            Some(PrintAdmission {
                ready_sequence: doc.ready_sequence,
                ahead_tasks: doc.ahead_tasks,
                ahead_pages: doc.ahead_pages,
                in_progress_tasks: 0,
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
        protocol_version: 1,
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
    tx: &mut Transaction<'_>,
    source: &str,
    input: &PrintRegistration,
) -> Result<PrintRequest, PrintError> {
    #[derive(QueryableByName)]
    struct Roster {
        #[diesel(sql_type = Text)]
        account_id: String,
        #[diesel(sql_type = Text)]
        team_name: String,
        #[diesel(sql_type = Nullable<Text>)]
        seat_label: Option<String>,
    }
    if let Some(row) = find(tx, source, &input.request_id)? {
        if row.account != input.account
            || row.original_filename != input.original_filename
            || row.source_sha256 != input.source_sha256
        {
            return Err(PrintError::Conflict);
        }
        return request(tx, source, &input.request_id);
    }
    let roster = sql_query("SELECT a.account_id, CASE WHEN t.name_zh != '' THEN t.name_zh ELSE t.name_en END AS team_name, s.seat_code AS seat_label FROM accounts a JOIN teams t ON t.account_id = a.account_id LEFT JOIN account_mappings m ON m.account_id = a.account_id LEFT JOIN seats s ON s.seat_id = m.seat_id WHERE a.domjudge_username = ?")
        .bind::<Text, _>(&input.account).get_result::<Roster>(tx.connection()).optional()?.ok_or(PrintError::Invalid)?;
    sql_query("INSERT INTO print_requests (source_id, request_id, account_id, account, team_name, seat_label, original_filename, source_sha256) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind::<Text, _>(source).bind::<Text, _>(&input.request_id).bind::<Text, _>(&roster.account_id)
        .bind::<Text, _>(&input.account).bind::<Text, _>(&roster.team_name).bind::<Nullable<Text>, _>(&roster.seat_label)
        .bind::<Text, _>(&input.original_filename).bind::<Text, _>(&input.source_sha256).execute(tx.connection())?;
    request(tx, source, &input.request_id)
}

pub(super) fn fail(
    tx: &mut Transaction<'_>,
    source: &str,
    id: &str,
    code: GenerationFailure,
) -> Result<PrintRequest, PrintError> {
    let current = request(tx, source, id)?;
    match current.state {
        PrintState::Ready => return Err(PrintError::Conflict),
        PrintState::GenerationFailed if current.generation_failure != Some(code) => {
            return Err(PrintError::Conflict);
        }
        PrintState::GenerationFailed => return Ok(current),
        PrintState::Preparing => {}
    }
    sql_query(
        "UPDATE print_requests SET generation_failure = ? WHERE source_id = ? AND request_id = ?",
    )
    .bind::<Text, _>(code.as_str())
    .bind::<Text, _>(source)
    .bind::<Text, _>(id)
    .execute(tx.connection())?;
    request(tx, source, id)
}

pub(super) fn admit(
    tx: &mut Transaction<'_>,
    source: &str,
    id: &str,
    pdf: &[u8],
    pages: i64,
    hash: &str,
) -> Result<PrintRequest, PrintError> {
    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type = BigInt)]
        n: i64,
    }
    let current = request(tx, source, id)?;
    if let Some(doc) = &current.document {
        // Compare exact bytes as well as the digest: only identical content is a replay.
        #[derive(QueryableByName)]
        struct StoredPdf {
            #[diesel(sql_type = Binary)]
            pdf: Vec<u8>,
        }
        let stored =
            sql_query("SELECT pdf FROM print_documents WHERE source_id = ? AND request_id = ?")
                .bind::<Text, _>(source)
                .bind::<Text, _>(id)
                .get_result::<StoredPdf>(tx.connection())?;
        return if doc.sha256 == hash && stored.pdf == pdf {
            Ok(current)
        } else {
            Err(PrintError::Conflict)
        };
    }
    if current.state != PrintState::Preparing {
        return Err(PrintError::Conflict);
    }
    let identity = sql_query(
        "SELECT count(*) AS n FROM accounts WHERE account_id = ? AND domjudge_username = ?",
    )
    .bind::<Text, _>(&current.account_id)
    .bind::<Text, _>(&current.account)
    .get_result::<Count>(tx.connection())?;
    if identity.n != 1 {
        return Err(PrintError::Conflict);
    }
    // All I1 documents are unclaimed. Aggregate and insertion share BEGIN IMMEDIATE.
    sql_query("INSERT INTO print_documents (source_id, request_id, pdf, sha256, bytes, pages, ahead_tasks, ahead_pages) SELECT ?, ?, ?, ?, ?, ?, count(*), coalesce(sum(pages), 0) FROM print_documents")
        .bind::<Text, _>(source).bind::<Text, _>(id).bind::<Binary, _>(pdf).bind::<Text, _>(hash)
        .bind::<BigInt, _>(i64::try_from(pdf.len()).map_err(|_| PrintError::TooLarge)?).bind::<BigInt, _>(pages).execute(tx.connection())?;
    request(tx, source, id)
}
