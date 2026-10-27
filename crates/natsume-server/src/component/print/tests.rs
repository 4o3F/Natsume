mod execution;
mod pairing;
mod persistence;

use std::sync::{Arc, Condvar, Mutex};

use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl, connection::SimpleConnection as _};
use lopdf::{Document, Object, Stream, dictionary};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::*;
use crate::{
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::{account_mappings, accounts, organizations, print_documents, seats, teams},
};

type TestResult = Result<(), crate::test_error::TestError>;

struct Fixture {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
    database: Database,
    print: PrintComponent,
    source: SourceIdentity,
    token: String,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = directory.path().join("print.sqlite3");
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|e| panic!("database: {e}"));
        database
            .write(|transaction| {
                diesel::insert_into(accounts::table)
                    .values((
                        accounts::account_id.eq("account-1"),
                        accounts::domjudge_username.eq("team-1"),
                        accounts::credential_revision.eq(1),
                    ))
                    .execute(transaction.connection())?;
                diesel::insert_into(organizations::table)
                    .values((
                        organizations::organization_id.eq(1),
                        organizations::name_key.eq("school"),
                        organizations::name_zh.eq(""),
                        organizations::name_en.eq("School"),
                        organizations::country.eq("CHN"),
                    ))
                    .execute(transaction.connection())?;
                diesel::insert_into(teams::table)
                    .values((
                        teams::account_id.eq("account-1"),
                        teams::organization_id.eq(1),
                        teams::name_zh.eq("队伍"),
                        teams::name_en.eq("Team"),
                        teams::category.eq("participant"),
                    ))
                    .execute(transaction.connection())?;
                diesel::insert_into(seats::table)
                    .values((seats::seat_id.eq("seat-1"), seats::seat_code.eq("A-01")))
                    .execute(transaction.connection())?;
                diesel::insert_into(account_mappings::table)
                    .values((
                        account_mappings::seat_id.eq("seat-1"),
                        account_mappings::account_id.eq("account-1"),
                    ))
                    .execute(transaction.connection())?;
                Ok::<_, PrintError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("roster: {error:?}"));
        let (source, token) = add_source(&database).await;
        let print =
            PrintComponent::for_test(database.clone(), path.with_extension("print-documents"))
                .unwrap_or_else(|error| panic!("print storage: {error}"));
        Self {
            _directory: directory,
            path,
            print,
            database,
            source,
            token,
        }
    }
    fn documents_path(&self) -> std::path::PathBuf {
        self.path.with_extension("print-documents")
    }

    async fn register(&self) -> PrintRequest {
        self.print
            .register(self.source.clone(), registration())
            .await
            .unwrap_or_else(|e| panic!("register: {e:?}"))
    }
}

async fn add_source(database: &Database) -> (SourceIdentity, String) {
    let source_id = Uuid::now_v7().to_string();
    let token = format!("nps1.{source_id}.{}", "ab".repeat(32));
    let token_hash = Sha256::digest(token.as_bytes()).into();
    let source = SourceIdentity {
        source_id,
        token_hash,
    };
    let identity = source.clone();
    database
        .write(move |tx| db::insert_source(tx, &identity.source_id, "judge", &identity.token_hash))
        .await
        .unwrap_or_else(|e| panic!("source: {e:?}"));
    (source, token)
}

fn registration() -> PrintRegistration {
    PrintRegistration {
        request_id: Uuid::now_v7().to_string(),
        account: "team-1".into(),
        original_filename: "main.cpp".into(),
        source_sha256: "12".repeat(32),
    }
}

pub(crate) fn pdf(pages: i64) -> Vec<u8> {
    save_pdf(&mut pdf_document(pages))
}

pub(crate) fn pdf_with_size(size: usize) -> Vec<u8> {
    let mut doc = pdf_document(1);
    let filler = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
    for _ in 0..10 {
        let bytes = save_pdf(&mut doc);
        if bytes.len() == size {
            return bytes;
        }
        let stream = doc
            .objects
            .get_mut(&filler)
            .and_then(|o| o.as_stream_mut().ok())
            .unwrap_or_else(|| panic!("filler"));
        let length = stream.content.len() + size - bytes.len();
        stream.set_content(vec![b' '; length]);
    }
    panic!("exact-size PDF fixture did not converge");
}

fn pdf_document(pages: i64) -> Document {
    let mut doc = Document::with_version("1.7");
    let root = doc.new_object_id();
    let content = doc.add_object(Stream::new(dictionary! {}, b"q Q".to_vec()));
    let mut kids = Vec::new();
    for _ in 0..pages {
        let id = doc
            .add_object(dictionary! { "Type" => "Page", "Parent" => root, "Contents" => content });
        kids.push(Object::Reference(id));
    }
    doc.objects.insert(root, dictionary! { "Type" => "Pages", "Count" => pages, "Kids" => kids, "MediaBox" => vec![0.into(), 0.into(), 595.2756.into(), 841.8898.into()] }.into());
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
    doc.trailer.set("Root", catalog);
    doc
}

fn save_pdf(doc: &mut Document) -> Vec<u8> {
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .unwrap_or_else(|e| panic!("PDF serialization: {e}"));
    out
}

#[tokio::test]
async fn registration_is_exact_idempotent_and_source_scoped() {
    let f = Fixture::new().await;
    let input = registration();
    let first = f
        .print
        .register(f.source.clone(), input.clone())
        .await
        .unwrap_or_else(|e| panic!("register: {e:?}"));
    assert_eq!(first.team_name, "队伍");
    assert_eq!(first.seat_label.as_deref(), Some("A-01"));
    assert_eq!(first.state, PrintState::Preparing);
    assert!(
        first.document.is_none() && first.admission.is_none() && first.generation_failure.is_none()
    );
    assert!(
        matches!(f.print.register(f.source.clone(), input.clone()).await, Ok(actual) if actual == first)
    );
    for changed in [
        PrintRegistration {
            account: "Team-1".into(),
            ..input.clone()
        },
        PrintRegistration {
            original_filename: "other.cpp".into(),
            ..input.clone()
        },
        PrintRegistration {
            source_sha256: "34".repeat(32),
            ..input.clone()
        },
    ] {
        assert!(matches!(
            f.print.register(f.source.clone(), changed).await,
            Err(PrintError::Conflict)
        ));
    }
    let (other, _) = add_source(&f.database).await;
    assert!(matches!(
        f.print.get(other.clone(), input.request_id.clone()).await,
        Err(PrintError::NotFound)
    ));
    assert!(matches!(
        f.print
            .upload(other.clone(), input.request_id.clone(), pdf(1))
            .await,
        Err(PrintError::NotFound)
    ));
    assert!(matches!(
        f.print
            .generation_failure(
                other.clone(),
                input.request_id.clone(),
                PrintGenerationFailure {
                    code: GenerationFailure::CompileFailed
                }
            )
            .await,
        Err(PrintError::NotFound)
    ));
    assert!(f.print.register(other.clone(), input.clone()).await.is_ok());
    assert!(
        f.print
            .upload(other, input.request_id.clone(), pdf(1))
            .await
            .is_ok()
    );
    assert_eq!(
        f.print
            .get(f.source.clone(), input.request_id.clone())
            .await
            .unwrap_or_else(|e| panic!("source isolation: {e:?}"))
            .state,
        PrintState::Preparing
    );
    for account in [" team-1", "TEAM-1", "1", "A-01", "队伍"] {
        assert!(matches!(
            f.print
                .register(
                    f.source.clone(),
                    PrintRegistration {
                        account: account.into(),
                        ..registration()
                    }
                )
                .await,
            Err(PrintError::Invalid)
        ));
    }
}

#[tokio::test]
async fn registration_snapshots_team_display_and_optional_seat() {
    let f = Fixture::new().await;
    let input = registration();
    assert!(
        f.print
            .register(f.source.clone(), input.clone())
            .await
            .is_ok()
    );
    f.database
        .write(|transaction| {
            diesel::update(teams::table)
                .set(teams::name_zh.eq(""))
                .execute(transaction.connection())?;
            diesel::delete(account_mappings::table).execute(transaction.connection())?;
            Ok::<_, PrintError>(())
        })
        .await
        .unwrap_or_else(|e| panic!("roster update: {e:?}"));
    let new = f.register().await;
    assert_eq!(new.team_name, "Team");
    assert!(new.seat_label.is_none());
    let old = f
        .print
        .get(f.source.clone(), input.request_id)
        .await
        .unwrap_or_else(|e| panic!("get: {e:?}"));
    assert_eq!(old.team_name, "队伍");
    assert_eq!(old.seat_label.as_deref(), Some("A-01"));
}

#[tokio::test]
async fn failure_and_document_transitions_are_closed_and_replay_first_results() {
    let f = Fixture::new().await;
    let failed = f.register().await.request_id;
    for code in [
        GenerationFailure::CompileFailed,
        GenerationFailure::CompileFailed,
    ] {
        let fact = f
            .print
            .generation_failure(
                f.source.clone(),
                failed.clone(),
                PrintGenerationFailure { code },
            )
            .await
            .unwrap_or_else(|e| panic!("failure: {e:?}"));
        assert_eq!(fact.state, PrintState::GenerationFailed);
        assert_eq!(fact.generation_failure, Some(code));
        assert!(fact.document.is_none() && fact.admission.is_none());
    }
    assert!(matches!(
        f.print
            .generation_failure(
                f.source.clone(),
                failed.clone(),
                PrintGenerationFailure {
                    code: GenerationFailure::CompileTimeout
                }
            )
            .await,
        Err(PrintError::Conflict)
    ));
    assert!(matches!(
        f.print.upload(f.source.clone(), failed, pdf(1)).await,
        Err(PrintError::Conflict)
    ));
    let ready = f.register().await.request_id;
    let bytes = pdf(3);
    let first = f
        .print
        .upload(f.source.clone(), ready.clone(), bytes.clone())
        .await
        .unwrap_or_else(|e| panic!("upload: {e:?}"));
    assert_eq!(first.state, PrintState::Ready);
    assert_eq!(
        first.document.as_ref().map(|d| (d.bytes, d.pages)),
        Some((i64::try_from(bytes.len()).unwrap_or_default(), 3))
    );
    assert!(
        matches!(f.print.upload(f.source.clone(), ready.clone(), bytes).await, Ok(actual) if actual == first)
    );
    assert!(matches!(
        f.print
            .upload(f.source.clone(), ready.clone(), pdf(2))
            .await,
        Err(PrintError::Conflict)
    ));
    assert!(matches!(
        f.print
            .generation_failure(
                f.source.clone(),
                ready,
                PrintGenerationFailure {
                    code: GenerationFailure::WrapperDeadline
                }
            )
            .await,
        Err(PrintError::Conflict)
    ));
}

#[tokio::test]
async fn fifo_uses_committed_documents_across_sources_excluding_other_states() {
    let f = Fixture::new().await;
    let first_registered = f.register().await.request_id;
    let _preparing = f.register().await;
    let failed = f.register().await.request_id;
    assert!(
        f.print
            .generation_failure(
                f.source.clone(),
                failed,
                PrintGenerationFailure {
                    code: GenerationFailure::DocumentTooLarge
                }
            )
            .await
            .is_ok()
    );
    let (other, _) = add_source(&f.database).await;
    let other_id = registration().request_id;
    assert!(
        f.print
            .register(
                other.clone(),
                PrintRegistration {
                    request_id: other_id.clone(),
                    ..registration()
                }
            )
            .await
            .is_ok()
    );
    let ahead = f
        .print
        .upload(other, other_id, pdf(4))
        .await
        .unwrap_or_else(|e| panic!("admit: {e:?}"));
    assert_eq!(
        ahead.admission,
        Some(PrintAdmission {
            ready_sequence: 1,
            ahead_tasks: 0,
            ahead_pages: 0,
            in_progress_tasks: 0
        })
    );
    let second = f
        .print
        .upload(f.source.clone(), first_registered.clone(), pdf(2))
        .await
        .unwrap_or_else(|e| panic!("admit: {e:?}"));
    assert_eq!(
        second.admission,
        Some(PrintAdmission {
            ready_sequence: 2,
            ahead_tasks: 1,
            ahead_pages: 4,
            in_progress_tasks: 0
        })
    );
    let third = f.register().await.request_id;
    assert!(
        f.print
            .upload(f.source.clone(), third, pdf(1))
            .await
            .is_ok()
    );
    let reopened = Database::connect_and_migrate(&DatabaseConfig::new(&f.path, false))
        .await
        .unwrap_or_else(|e| panic!("restart: {e:?}"));
    let print = PrintComponent::load(reopened, f.documents_path())
        .await
        .unwrap_or_else(|error| panic!("print storage reopen: {error}"));
    assert!(
        matches!(print.get(f.source.clone(), first_registered.clone()).await, Ok(actual) if actual == second)
    );
    assert_eq!(
        print
            .upload(f.source, first_registered, pdf(2))
            .await
            .unwrap_or_else(|e| panic!("replay: {e:?}"))
            .admission
            .map(|a| a.ahead_tasks),
        Some(1)
    );
}

#[tokio::test]
async fn admission_snapshot_preserves_page_totals_above_i32() -> Result<(), PrintError> {
    let f = Fixture::new().await;
    let first = f.register().await.request_id;
    f.print
        .upload(f.source.clone(), first.clone(), pdf(1))
        .await?;
    let total_pages = i64::from(i32::MAX) + 1;
    // Exercise the aggregate's integer width without constructing an enormous PDF.
    f.database
        .write(move |transaction| {
            diesel::update(print_documents::table.filter(print_documents::request_id.eq(first)))
                .set(print_documents::pages.eq(total_pages))
                .execute(transaction.connection())?;
            Ok::<_, PrintError>(())
        })
        .await
        .map_err(TransactionError::into_error)?;
    let next = f.register().await.request_id;
    assert_eq!(
        f.print.upload(f.source, next, pdf(1)).await?.admission,
        Some(PrintAdmission {
            ready_sequence: 2,
            ahead_tasks: 1,
            ahead_pages: total_pages,
            in_progress_tasks: 0
        }),
    );
    Ok(())
}

#[tokio::test]
async fn deleted_recreated_account_cannot_retarget_preparing_but_ready_survives() {
    let f = Fixture::new().await;
    let preparing = f.register().await.request_id;
    let ready = f.register().await.request_id;
    let committed = f
        .print
        .upload(f.source.clone(), ready.clone(), pdf(1))
        .await
        .unwrap_or_else(|e| panic!("admit: {e:?}"));
    f.database
        .write(|transaction| {
            diesel::delete(accounts::table).execute(transaction.connection())?;
            diesel::insert_into(accounts::table)
                .values((
                    accounts::account_id.eq("new-account"),
                    accounts::domjudge_username.eq("team-1"),
                    accounts::credential_revision.eq(1),
                ))
                .execute(transaction.connection())?;
            diesel::insert_into(teams::table)
                .values((
                    teams::account_id.eq("new-account"),
                    teams::organization_id.eq(1),
                    teams::name_zh.eq("新队伍"),
                    teams::name_en.eq("New Team"),
                    teams::category.eq("participant"),
                ))
                .execute(transaction.connection())?;
            Ok::<_, PrintError>(())
        })
        .await
        .unwrap_or_else(|error| panic!("replace account: {error:?}"));
    assert!(matches!(
        f.print.upload(f.source.clone(), preparing, pdf(1)).await,
        Err(PrintError::Conflict)
    ));
    assert!(
        matches!(f.print.get(f.source.clone(), ready.clone()).await, Ok(actual) if actual == committed)
    );
    assert!(f.print.upload(f.source, ready, pdf(1)).await.is_ok());
}

#[tokio::test]
async fn revoked_source_is_rechecked_in_each_operation_even_with_stale_identity() {
    let f = Fixture::new().await;
    let id = f.register().await.request_id;
    assert!(f.print.authenticate(&f.token).await.is_ok());
    for token in [
        format!("{}x", f.token),
        f.token.to_uppercase(),
        "operator-cookie".into(),
        format!("nps1.{}.{}", Uuid::now_v7(), "ab".repeat(32)),
    ] {
        assert!(matches!(
            f.print.authenticate(&token).await,
            Err(PrintError::Authentication)
        ));
    }
    let sid = f.source.source_id.clone();
    f.database
        .write(move |tx| db::revoke_source(tx, &sid))
        .await
        .unwrap_or_else(|e| panic!("revoke: {e:?}"));
    assert!(matches!(
        f.print.authenticate(&f.token).await,
        Err(PrintError::Authentication)
    ));
    assert!(matches!(
        f.print.register(f.source.clone(), registration()).await,
        Err(PrintError::Authentication)
    ));
    assert!(matches!(
        f.print.get(f.source.clone(), id.clone()).await,
        Err(PrintError::Authentication)
    ));
    assert!(matches!(
        f.print.upload(f.source.clone(), id.clone(), pdf(1)).await,
        Err(PrintError::Authentication)
    ));
    assert!(matches!(
        f.print
            .generation_failure(
                f.source,
                id,
                PrintGenerationFailure {
                    code: GenerationFailure::CompileFailed
                }
            )
            .await,
        Err(PrintError::Authentication)
    ));
}

#[tokio::test]
async fn admission_keeps_durable_file_but_rolls_back_metadata_and_sequence_on_database_failure() {
    let f = Fixture::new().await;
    let id = f.register().await.request_id;
    f.database
        .write(|transaction| {
            transaction
                .connection()
                .batch_execute(
                    "CREATE TRIGGER fail_admission AFTER INSERT ON print_documents \
         BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )
                .map_err(PersistenceError::from)
        })
        .await
        .unwrap_or_else(|error| panic!("trigger: {error:?}"));
    assert!(matches!(
        f.print.upload(f.source.clone(), id.clone(), pdf(1)).await,
        Err(PrintError::Persistence { .. })
    ));
    assert_eq!(
        f.print
            .get(f.source.clone(), id.clone())
            .await
            .unwrap_or_else(|e| panic!("get: {e:?}"))
            .state,
        PrintState::Preparing
    );
    f.database
        .write(|tx| {
            tx.connection()
                .batch_execute("DROP TRIGGER fail_admission;")
                .map_err(PersistenceError::from)
        })
        .await
        .unwrap_or_else(|e| panic!("drop trigger: {e:?}"));
    assert_eq!(
        f.print
            .upload(f.source, id, pdf(1))
            .await
            .unwrap_or_else(|e| panic!("admit: {e:?}"))
            .admission
            .map(|a| a.ready_sequence),
        Some(1)
    );
}

#[tokio::test]
async fn cancelled_admission_caller_does_not_cancel_started_atomic_commit() {
    let f = Fixture::new().await;
    let id = f.register().await.request_id;
    let (entered, started) = tokio::sync::oneshot::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_gate = gate.clone();
    let database = f.database.clone();
    let source = f.source.clone();
    let request_id = id.clone();
    let bytes = pdf(2);
    let hash = hex::encode(Sha256::digest(&bytes));
    f.print
        .documents
        .put(&hash, &bytes)
        .unwrap_or_else(|error| panic!("persist document: {error}"));
    let length = i64::try_from(bytes.len()).unwrap_or_default();
    let task = tokio::spawn(async move {
        database
            .write(move |tx| {
                db::authenticate(tx, &source)?;
                let result = db::admit(tx, &source.source_id, &request_id, length, 2, &hash)?;
                let _ = entered.send(());
                let guard = worker_gate
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                drop(
                    worker_gate
                        .1
                        .wait_while(guard, |released| !*released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
                Ok::<_, PrintError>(result)
            })
            .await
    });
    started.await.unwrap_or_else(|e| panic!("started: {e}"));
    task.abort();
    assert!(task.await.is_err());
    *gate
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    gate.1.notify_all();
    // BEGIN IMMEDIATE waits for the released transaction to commit or roll back.
    f.database
        .write(|_| Ok::<_, PrintError>(()))
        .await
        .unwrap_or_else(|error| panic!("wait for admission transaction: {error:?}"));
    let result = f
        .print
        .get(f.source.clone(), id)
        .await
        .unwrap_or_else(|error| panic!("get: {error:?}"));
    assert_eq!(result.state, PrintState::Ready);
    assert_eq!(result.document.map(|document| document.pages), Some(2));
}

#[tokio::test]
async fn concurrent_admissions_allocate_unique_fifo_snapshots() {
    let f = Fixture::new().await;
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let id = f.register().await.request_id;
        let print = PrintComponent::for_test(f.database.clone(), f.documents_path())
            .unwrap_or_else(|error| panic!("print storage: {error}"));
        let source = f.source.clone();
        tasks.push(tokio::spawn(async move {
            print.upload(source, id, pdf(2)).await
        }));
    }
    let mut admissions = Vec::new();
    for task in tasks {
        admissions.push(
            task.await
                .unwrap_or_else(|e| panic!("task: {e:?}"))
                .unwrap_or_else(|e| panic!("admit: {e:?}"))
                .admission
                .unwrap_or_else(|| panic!("admission")),
        );
    }
    admissions.sort_by_key(|a| a.ready_sequence);
    for (index, admission) in admissions.iter().enumerate() {
        let n = i64::try_from(index).unwrap_or_default();
        assert_eq!(
            (
                admission.ready_sequence,
                admission.ahead_tasks,
                admission.ahead_pages
            ),
            (n + 1, n, n * 2)
        );
    }
}

#[test]
fn parser_accepts_real_page_trees_and_rejects_invalid_active_and_external_objects() {
    assert!(matches!(super::pdf::validate(&pdf(3)), Ok(actual) if actual == 3));
    assert!(matches!(
        super::pdf::validate(&pdf(0)),
        Err(PrintError::Invalid)
    ));
    for bytes in [
        b"%PDF-1.7\n/Type /Page\n%%EOF".to_vec(),
        pdf(1)[..80].to_vec(),
    ] {
        assert!(matches!(
            super::pdf::validate(&bytes),
            Err(PrintError::Invalid)
        ));
    }
    for (key, value) in [
        ("OpenAction", Object::Integer(1)),
        ("AA", dictionary! {}.into()),
        ("EF", dictionary! {}.into()),
        ("F", Object::string_literal("external.pdf")),
        ("URI", Object::string_literal("https://example.test")),
        ("Type", Object::Name(b"Filespec".to_vec())),
        ("S", Object::Name(b"JavaScript".to_vec())),
        ("Annots", vec![].into()),
        ("Ref", dictionary! {}.into()),
    ] {
        let mut doc = pdf_document(1);
        let indirect = doc.add_object(value);
        doc.add_object(dictionary! {key => indirect});
        assert!(
            matches!(
                super::pdf::validate(&save_pdf(&mut doc)),
                Err(PrintError::Invalid)
            ),
            "key {key}"
        );
    }
    let mut encrypted = pdf_document(1);
    let encryption = encrypted.add_object(dictionary! {});
    encrypted.trailer.set("Encrypt", encryption);
    assert!(matches!(
        super::pdf::validate(&save_pdf(&mut encrypted)),
        Err(PrintError::Invalid)
    ));
    for (key, value) in [
        ("Count", Object::Integer(2)),
        (
            "MediaBox",
            vec![0.into(), 0.into(), 612.into(), 792.into()].into(),
        ),
        (
            "CropBox",
            vec![0.into(), 0.into(), 10.into(), 10.into()].into(),
        ),
    ] {
        let mut doc = pdf_document(1);
        let root = doc
            .catalog()
            .and_then(|c| c.get(b"Pages"))
            .and_then(Object::as_reference)
            .unwrap_or_else(|e| panic!("root: {e}"));
        doc.get_dictionary_mut(root)
            .unwrap_or_else(|e| panic!("tree: {e}"))
            .set(key, value);
        assert!(
            matches!(
                super::pdf::validate(&save_pdf(&mut doc)),
                Err(PrintError::Invalid)
            ),
            "key {key}"
        );
    }
}

#[tokio::test]
async fn registration_rejects_invalid_ids_and_display_paths() -> TestResult {
    let f = Fixture::new().await;
    for filename in ["../main.cpp", "a\\b", "a\n.cpp", "a\0.cpp"] {
        assert!(matches!(
            f.print
                .register(
                    f.source.clone(),
                    PrintRegistration {
                        original_filename: filename.into(),
                        ..registration()
                    }
                )
                .await,
            Err(PrintError::Invalid)
        ));
    }
    for id in [
        Uuid::new_v5(&Uuid::NAMESPACE_URL, b"a").to_string(),
        Uuid::now_v7().to_string().to_uppercase(),
        "not-an-id".into(),
    ] {
        assert!(matches!(
            f.print
                .register(
                    f.source.clone(),
                    PrintRegistration {
                        request_id: id,
                        ..registration()
                    }
                )
                .await,
            Err(PrintError::Invalid)
        ));
    }
    assert!(matches!(
        f.print
            .register(
                f.source,
                PrintRegistration {
                    original_filename: "中".repeat(342),
                    ..registration()
                }
            )
            .await,
        Err(PrintError::Invalid)
    ));
    Ok(())
}

#[test]
fn parser_rejects_page_tree_cycles_dangling_references_and_corrupt_stream_lengths() {
    let mut doc = pdf_document(1);
    let root = doc
        .catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .unwrap_or_else(|e| panic!("root: {e}"));
    let mut cyclic = doc.clone();
    cyclic
        .get_dictionary_mut(root)
        .unwrap_or_else(|e| panic!("root: {e}"))
        .set("Kids", vec![Object::Reference(root)]);
    assert!(matches!(
        super::pdf::validate(&save_pdf(&mut cyclic)),
        Err(PrintError::Invalid)
    ));
    let mut duplicate = doc.clone();
    let dict = duplicate
        .get_dictionary_mut(root)
        .unwrap_or_else(|e| panic!("root: {e}"));
    let kid = dict
        .get(b"Kids")
        .and_then(Object::as_array)
        .unwrap_or_else(|e| panic!("kids: {e}"))[0]
        .clone();
    dict.set("Kids", vec![kid.clone(), kid]);
    dict.set("Count", 2);
    assert!(matches!(
        super::pdf::validate(&save_pdf(&mut duplicate)),
        Err(PrintError::Invalid)
    ));
    doc.add_object(dictionary! {"Missing" => Object::Reference((999, 0))});
    assert!(matches!(
        super::pdf::validate(&save_pdf(&mut doc)),
        Err(PrintError::Invalid)
    ));
    let mut malformed = pdf(1);
    let marker = b"/Length 3";
    let offset = malformed
        .windows(marker.len())
        .position(|v| v == marker)
        .unwrap_or_else(|| panic!("length marker"));
    malformed[offset + marker.len() - 1] = b'9';
    assert!(matches!(
        super::pdf::validate(&malformed),
        Err(PrintError::Invalid)
    ));
    let mut bad_xref = pdf(1);
    let marker = b"startxref\n";
    let offset = bad_xref
        .windows(marker.len())
        .rposition(|v| v == marker)
        .unwrap_or_else(|| panic!("xref marker"))
        + marker.len();
    bad_xref[offset] = b'0';
    assert!(matches!(
        super::pdf::validate(&bad_xref),
        Err(PrintError::Invalid)
    ));
}

#[test]
fn parser_rejects_encryption_even_when_empty_password_is_automatically_decrypted() {
    for user_password in ["", "secret"] {
        let mut doc = pdf_document(1);
        doc.trailer.set(
            "ID",
            vec![
                Object::string_literal("print-test-id"),
                Object::string_literal("print-test-id"),
            ],
        );
        let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
            document: &doc,
            owner_password: "owner",
            user_password,
            key_length: 128,
            permissions: lopdf::Permissions::PRINTABLE,
        })
        .unwrap_or_else(|e| panic!("encryption state: {e}"));
        doc.encrypt(&state)
            .unwrap_or_else(|e| panic!("encrypt: {e}"));
        assert!(matches!(
            super::pdf::validate(&save_pdf(&mut doc)),
            Err(PrintError::Invalid)
        ));
    }
}
