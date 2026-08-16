use std::{fs, path::PathBuf};

use argon2::{
    Algorithm, Argon2, KeyId, Params, ParamsBuilder, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    audit::CorrelationId,
    db::{Database, DatabaseConfig, operator as db_operator},
};

use super::{
    DUMMY_PASSWORD_PHC, OperatorCredentials, OperatorError, OperatorIdentity, OperatorPassword,
    OperatorRole, PASSWORD_VERIFICATION_CONCURRENCY, PASSWORD_VERIFICATION_GATE,
    SESSION_CREDENTIAL_LENGTH, SessionCredential, SessionCredentialHash, authenticate_session,
    decode_lower_hex, hash_password, require_admin, sign_in, verify_password_once,
};

const GATE_OBSERVATION_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);
const GATE_RELEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

static PASSWORD_VERIFICATION_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) struct PasswordVerificationTestGuard {
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl PasswordVerificationTestGuard {
    /// Serialises password-verification tests so their expensive work does
    /// not contend and distort timing-sensitive evidence.
    pub(crate) async fn acquire() -> Self {
        Self {
            _guard: PASSWORD_VERIFICATION_TEST_LOCK.lock().await,
        }
    }
}

pub(crate) const fn operator_identity(operator_id: Uuid, role: OperatorRole) -> OperatorIdentity {
    OperatorIdentity { operator_id, role }
}

pub(crate) fn session_credential_hash(
    raw: &[u8; SESSION_CREDENTIAL_LENGTH],
) -> SessionCredentialHash {
    SessionCredentialHash(Zeroizing::new(Sha256::digest(raw).into()))
}

#[test]
fn persisted_roles_are_closed() -> Result<(), TestFailure> {
    if OperatorRole::Admin.as_persisted() != "admin"
        || OperatorRole::Viewer.as_persisted() != "viewer"
        || OperatorRole::from_persisted("admin") != Ok(OperatorRole::Admin)
        || OperatorRole::from_persisted("viewer") != Ok(OperatorRole::Viewer)
        || OperatorRole::from_persisted("owner") != Err(OperatorError::InvalidPersistedRole)
    {
        return Err(TestFailure::PersistedRolesWereNotClosed);
    }
    Ok(())
}

#[test]
fn bootstrap_input_errors_are_redacted() -> Result<(), TestFailure> {
    let login_canary = "bootstrap-login-canary";
    let password_canary = "bootstrap-password-canary";
    let Err(error) = OperatorCredentials::new(
        login_canary.to_owned(),
        password_canary.to_owned(),
        "different-password-canary".to_owned(),
    ) else {
        return Err(TestFailure::ExpectedInputFailure);
    };
    for encoded in [error.to_string(), format!("{error:?}")] {
        if encoded.contains(login_canary) || encoded.contains(password_canary) {
            return Err(TestFailure::InputErrorWasNotRedacted);
        }
    }
    Ok(())
}

#[test]
fn password_hash_uses_the_frozen_profile_and_verifies() -> Result<(), TestFailure> {
    let password = "correct horse battery staple";
    let credentials =
        OperatorCredentials::new("admin".to_owned(), password.to_owned(), password.to_owned())
            .map_err(|_| TestFailure::ValidCredentialsWereRejected)?;
    let encoded =
        hash_password(credentials.password()).map_err(|_| TestFailure::PasswordHashingFailed)?;
    let parsed = PasswordHash::new(&encoded).map_err(|_| TestFailure::GeneratedPhcWasInvalid)?;

    if parsed.algorithm.as_str() != "argon2id"
        || parsed.version != Some(19)
        || parsed.params.get_decimal("m") != Some(19_456)
        || parsed.params.get_decimal("t") != Some(2)
        || parsed.params.get_decimal("p") != Some(1)
    {
        return Err(TestFailure::Argon2ProfileWasNotFrozen);
    }
    let mut salt_bytes = [0_u8; 64];
    let salt = parsed.salt.ok_or(TestFailure::GeneratedPhcHadNoSalt)?;
    let decoded_salt = salt
        .decode_b64(&mut salt_bytes)
        .map_err(|_| TestFailure::GeneratedPhcSaltWasInvalid)?;
    if decoded_salt.len() != 16 {
        return Err(TestFailure::Argon2SaltLengthWasNotFrozen);
    }

    verify_with_frozen_profile(password.as_bytes(), &parsed)?;
    if verify_with_frozen_profile(b"different password", &parsed).is_ok() {
        return Err(TestFailure::IncorrectPasswordWasAccepted);
    }
    Ok(())
}

#[test]
fn password_verification_returns_typed_expected_outcomes() -> Result<(), TestFailure> {
    let correct = OperatorPassword::new("verification-contract-password".to_owned());
    let wrong = OperatorPassword::new("wrong-verification-contract-password".to_owned());
    let encoded = hash_password(&correct).map_err(|_| TestFailure::PasswordHashingFailed)?;
    let correct_verified = verify_password_once(&correct, &encoded)
        .map_err(|_| TestFailure::PasswordVerificationFailed)?;
    let wrong_verified = verify_password_once(&wrong, &encoded)
        .map_err(|_| TestFailure::PasswordVerificationFailed)?;
    let invalid_profile_verified = verify_password_once(&correct, "invalid-persisted-password-phc")
        .map_err(|_| TestFailure::PasswordVerificationFailed)?;
    if !correct_verified || wrong_verified || invalid_profile_verified {
        return Err(TestFailure::PasswordVerificationContractChanged);
    }
    Ok(())
}

#[test]
fn session_credentials_use_fixed_lower_hex_and_hash_raw_bytes() -> Result<(), TestFailure> {
    for byte in u8::MIN..=u8::MAX {
        let credential = SessionCredential(Zeroizing::new([byte; SESSION_CREDENTIAL_LENGTH]));
        let wire = credential.to_wire();
        let decoded =
            SessionCredential::from_wire(&wire).map_err(|_| TestFailure::CredentialDecodeFailed)?;
        if decoded.0.as_slice() != credential.0.as_slice() {
            return Err(TestFailure::CredentialRoundTripChangedBytes);
        }
    }

    let credential =
        SessionCredential::generate().map_err(|_| TestFailure::CredentialGenerationFailed)?;
    let wire = credential.to_wire();
    if wire.expose().len() != 64
        || !wire
            .expose()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TestFailure::CredentialWireEncodingWasInvalid);
    }
    let decoded =
        SessionCredential::from_wire(&wire).map_err(|_| TestFailure::CredentialDecodeFailed)?;
    if decoded.0.as_slice() != credential.0.as_slice() {
        return Err(TestFailure::CredentialRoundTripChangedBytes);
    }

    let persisted_hash = credential.sha256();
    let expected_raw_hash = Sha256::digest(credential.0.as_slice());
    let forbidden_hex_hash = Sha256::digest(wire.expose().as_bytes());
    if persisted_hash.as_bytes().as_slice() != expected_raw_hash.as_slice() {
        return Err(TestFailure::CredentialHashDidNotUseRawBytes);
    }
    if persisted_hash.as_bytes().as_slice() == forbidden_hex_hash.as_slice() {
        return Err(TestFailure::CredentialHashUsedHexText);
    }

    let invalid_values = [
        "0".repeat(63),
        "0".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
        format!(" {} ", "0".repeat(64)),
    ];
    for value in invalid_values {
        let Err(error) = decode_lower_hex(&value) else {
            return Err(TestFailure::InvalidCredentialWasAccepted);
        };
        if error != OperatorError::InvalidSessionCredential {
            return Err(TestFailure::CredentialDecodeErrorWasNotUnified);
        }
        for encoded in [error.to_string(), format!("{error:?}")] {
            if encoded.contains(&value) {
                return Err(TestFailure::CredentialErrorWasNotRedacted);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn sign_in_unifies_failures_and_supports_both_roles() -> Result<(), TestFailure> {
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    assert_dummy_password_phc_is_frozen()?;
    let admin_password = "admin-password-canary";
    let viewer_password = "viewer-password-canary";
    prepare_sign_in_accounts(&fixture.database, admin_password, viewer_password).await?;

    let before_sign_in = db_operator::tests::test_now(&fixture.database)
        .await
        .map_err(|_| TestFailure::SessionEvidenceReadFailed)?;
    let admin_session = sign_in(
        &fixture.database,
        correlation_id(),
        "exact-admin",
        admin_password.to_owned(),
    )
    .await
    .map_err(|_| TestFailure::CorrectSignInFailed)?;
    let viewer_session = sign_in(
        &fixture.database,
        correlation_id(),
        "exact-viewer",
        viewer_password.to_owned(),
    )
    .await
    .map_err(|_| TestFailure::CorrectSignInFailed)?;
    let after_sign_in = db_operator::tests::test_now(&fixture.database)
        .await
        .map_err(|_| TestFailure::SessionEvidenceReadFailed)?;
    if admin_session.identity().role() != OperatorRole::Admin
        || viewer_session.identity().role() != OperatorRole::Viewer
    {
        return Err(TestFailure::SignedInRoleWasIncorrect);
    }
    if !db_operator::tests::test_sessions_have_eight_hour_ttl(
        &fixture.database,
        &before_sign_in,
        &after_sign_in,
    )
    .await
    .map_err(|_| TestFailure::SessionEvidenceReadFailed)?
    {
        return Err(TestFailure::SignedInSessionTtlWasNotFrozen);
    }

    let authenticated = authenticate_session(
        &fixture.database,
        correlation_id(),
        admin_session.credential().to_wire(),
    )
    .await
    .map_err(|_| TestFailure::SessionAuthenticationFailed)?;
    if authenticated != admin_session.identity() {
        return Err(TestFailure::AuthenticatedIdentityChanged);
    }

    let persisted_hashes = db_operator::tests::test_session_hashes(&fixture.database)
        .await
        .map_err(|_| TestFailure::SessionEvidenceReadFailed)?;
    let expected_hash = admin_session.credential().sha256();
    if !persisted_hashes
        .iter()
        .any(|hash| hash.as_slice() == expected_hash.as_bytes().as_slice())
        || persisted_hashes
            .iter()
            .any(|hash| hash.as_slice() == admin_session.credential().0.as_slice())
    {
        return Err(TestFailure::PersistedCredentialEvidenceWasInvalid);
    }
    let wire = admin_session.credential().to_wire();
    if db_operator::tests::test_database_contains_credential_canary(
        &fixture.database,
        wire.expose(),
    )
    .await
    .map_err(|_| TestFailure::SessionEvidenceReadFailed)?
    {
        return Err(TestFailure::CredentialEscapedIntoDatabase);
    }
    assert_failed_sign_ins_are_unified(&fixture.database).await
}

async fn prepare_sign_in_accounts(
    database: &Database,
    admin_password: &str,
    viewer_password: &str,
) -> Result<(), TestFailure> {
    let admin_phc = password_phc("admin", admin_password)?;
    let viewer_phc = password_phc("viewer", viewer_password)?;
    let extra_parameter_phc = extra_parameter_phc("non-frozen-password-canary")?;
    let short_salt_phc = short_salt_phc("non-frozen-password-canary")?;
    for (login_name, role, password_hash) in [
        ("exact-admin", OperatorRole::Admin, admin_phc.as_str()),
        ("exact-viewer", OperatorRole::Viewer, viewer_phc.as_str()),
        (
            "corrupt-phc",
            OperatorRole::Admin,
            "corrupted-persisted-phc-canary",
        ),
        (
            "extra-parameter-phc",
            OperatorRole::Admin,
            extra_parameter_phc.as_str(),
        ),
        (
            "short-salt-phc",
            OperatorRole::Admin,
            short_salt_phc.as_str(),
        ),
    ] {
        db_operator::tests::test_insert_account(database, login_name, role, password_hash)
            .await
            .map_err(|_| TestFailure::AccountFixtureInsertFailed)?;
    }
    Ok(())
}

async fn assert_failed_sign_ins_are_unified(database: &Database) -> Result<(), TestFailure> {
    let before_failures = db_operator::tests::test_session_and_audit_counts(database)
        .await
        .map_err(|_| TestFailure::SessionEvidenceReadFailed)?;
    let wrong_password = failed_sign_in_once(
        database,
        "exact-admin",
        "wrong-password-canary",
        TestFailure::InvalidSignInSucceeded,
    )
    .await?;
    let extra_parameter = failed_sign_in_once(
        database,
        "extra-parameter-phc",
        "non-frozen-password-canary",
        TestFailure::NonFrozenPhcAuthenticated,
    )
    .await?;
    let short_salt = failed_sign_in_once(
        database,
        "short-salt-phc",
        "non-frozen-password-canary",
        TestFailure::NonFrozenPhcAuthenticated,
    )
    .await?;
    let corrupt_phc = failed_sign_in_once(
        database,
        "corrupt-phc",
        "wrong-password-canary",
        TestFailure::InvalidSignInSucceeded,
    )
    .await?;
    let unknown_login = failed_sign_in_once(
        database,
        "unknown-login-canary",
        "natsume-dummy-password",
        TestFailure::DummyPlaintextAuthenticatedUnknownLogin,
    )
    .await?;
    if wrong_password != OperatorError::AuthenticationFailed
        || corrupt_phc != wrong_password
        || extra_parameter != wrong_password
        || short_salt != wrong_password
        || unknown_login != wrong_password
    {
        return Err(TestFailure::SignInFailuresWereDistinguishable);
    }
    if db_operator::tests::test_session_and_audit_counts(database)
        .await
        .map_err(|_| TestFailure::SessionEvidenceReadFailed)?
        != before_failures
    {
        return Err(TestFailure::FailedSignInWroteRows);
    }
    for error in [
        wrong_password,
        corrupt_phc,
        extra_parameter,
        short_salt,
        unknown_login,
    ] {
        for encoded in [error.to_string(), format!("{error:?}")] {
            if encoded.contains("wrong-password-canary")
                || encoded.contains("unknown-login-canary")
                || encoded.contains(DUMMY_PASSWORD_PHC)
            {
                return Err(TestFailure::SignInErrorWasNotRedacted);
            }
        }
    }
    Ok(())
}

async fn failed_sign_in_once(
    database: &Database,
    login_name: &str,
    password: &str,
    unexpected_success: TestFailure,
) -> Result<OperatorError, TestFailure> {
    sign_in(database, correlation_id(), login_name, password.to_owned())
        .await
        .err()
        .ok_or(unexpected_success)
}

#[tokio::test]
async fn password_verification_concurrency_is_bounded() -> Result<(), TestFailure> {
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    let permits = u32::try_from(PASSWORD_VERIFICATION_CONCURRENCY)
        .map_err(|_| TestFailure::VerificationGateBoundWasInvalid)?;

    // All but one permit held: the gate must still admit one verification.
    let held = PASSWORD_VERIFICATION_GATE
        .acquire_many(permits - 1)
        .await
        .map_err(|_| TestFailure::VerificationGateWasClosed)?;
    gated_sign_in_fails(&fixture.database).await?;

    // Every permit held: the next verification must not proceed.
    let last = PASSWORD_VERIFICATION_GATE
        .acquire()
        .await
        .map_err(|_| TestFailure::VerificationGateWasClosed)?;
    let database = fixture.database.clone();
    let blocked = tokio::spawn(async move { gated_sign_in_fails(&database).await });
    tokio::time::sleep(GATE_OBSERVATION_WINDOW).await;
    if blocked.is_finished() {
        return Err(TestFailure::VerificationGateDidNotBoundConcurrency);
    }

    drop(last);
    drop(held);
    tokio::time::timeout(GATE_RELEASE_TIMEOUT, blocked)
        .await
        .map_err(|_| TestFailure::VerificationGateDidNotRelease)?
        .map_err(|_| TestFailure::VerificationGateDidNotRelease)??;
    if PASSWORD_VERIFICATION_GATE.available_permits() != PASSWORD_VERIFICATION_CONCURRENCY {
        return Err(TestFailure::VerificationGateLeakedPermits);
    }
    Ok(())
}

async fn gated_sign_in_fails(database: &Database) -> Result<(), TestFailure> {
    let error = sign_in(
        database,
        correlation_id(),
        "gate-unknown-login-canary",
        "gate-password-canary".to_owned(),
    )
    .await
    .err()
    .ok_or(TestFailure::InvalidSignInSucceeded)?;
    if error != OperatorError::AuthenticationFailed {
        return Err(TestFailure::SignInFailuresWereDistinguishable);
    }
    Ok(())
}

#[test]
fn admin_authorization_is_closed() -> Result<(), TestFailure> {
    if require_admin(OperatorRole::Admin).is_err()
        || require_admin(OperatorRole::Viewer) != Err(OperatorError::AuthorizationDenied)
    {
        return Err(TestFailure::AdminAuthorizationWasNotClosed);
    }
    Ok(())
}

fn verify_with_frozen_profile(
    password: &[u8],
    parsed: &PasswordHash<'_>,
) -> Result<(), TestFailure> {
    if parsed.algorithm.as_str() != "argon2id"
        || parsed.version != Some(19)
        || parsed.params.get_decimal("m") != Some(19_456)
        || parsed.params.get_decimal("t") != Some(2)
        || parsed.params.get_decimal("p") != Some(1)
    {
        return Err(TestFailure::VerificationProfileWasNotFrozen);
    }
    frozen_argon2()?
        .verify_password(password, parsed)
        .map_err(|_| TestFailure::PasswordVerificationFailed)
}

fn assert_dummy_password_phc_is_frozen() -> Result<(), TestFailure> {
    let parsed =
        PasswordHash::new(DUMMY_PASSWORD_PHC).map_err(|_| TestFailure::DummyPhcWasInvalid)?;
    if parsed.algorithm.as_str() != "argon2id"
        || parsed.version != Some(19)
        || parsed.params.get_decimal("m") != Some(19_456)
        || parsed.params.get_decimal("t") != Some(2)
        || parsed.params.get_decimal("p") != Some(1)
    {
        return Err(TestFailure::DummyPhcProfileWasNotFrozen);
    }
    let mut salt_bytes = [0_u8; 64];
    let salt = parsed.salt.ok_or(TestFailure::DummyPhcWasInvalid)?;
    if salt
        .decode_b64(&mut salt_bytes)
        .map_err(|_| TestFailure::DummyPhcWasInvalid)?
        .len()
        != 16
    {
        return Err(TestFailure::DummyPhcSaltWasNotFrozen);
    }
    verify_with_frozen_profile(b"natsume-dummy-password", &parsed)
}

fn frozen_argon2() -> Result<Argon2<'static>, TestFailure> {
    let params =
        Params::new(19_456, 2, 1, None).map_err(|_| TestFailure::FrozenParametersWereRejected)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

fn password_phc(login_name: &str, password: &str) -> Result<String, TestFailure> {
    let credentials = OperatorCredentials::new(
        login_name.to_owned(),
        password.to_owned(),
        password.to_owned(),
    )
    .map_err(|_| TestFailure::ValidCredentialsWereRejected)?;
    hash_password(credentials.password()).map_err(|_| TestFailure::PasswordHashingFailed)
}

fn extra_parameter_phc(password: &str) -> Result<String, TestFailure> {
    let mut builder = ParamsBuilder::new();
    builder
        .m_cost(19_456)
        .t_cost(2)
        .p_cost(1)
        .keyid(KeyId::new(&[0x93_u8; 4]).map_err(|_| TestFailure::NonFrozenPhcFixtureFailed)?);
    let params = builder
        .build()
        .map_err(|_| TestFailure::NonFrozenPhcFixtureFailed)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let salt = SaltString::encode_b64(&[0x91_u8; 16])
        .map_err(|_| TestFailure::NonFrozenPhcFixtureFailed)?;
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| TestFailure::NonFrozenPhcFixtureFailed)
}

fn short_salt_phc(password: &str) -> Result<String, TestFailure> {
    let salt = SaltString::encode_b64(&[0x92_u8; 8])
        .map_err(|_| TestFailure::NonFrozenPhcFixtureFailed)?;
    frozen_argon2()?
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| TestFailure::NonFrozenPhcFixtureFailed)
}

fn correlation_id() -> CorrelationId {
    CorrelationId::from_uuid(Uuid::now_v7())
}

struct TestDatabase {
    database: Database,
    path: PathBuf,
}

impl TestDatabase {
    async fn new() -> Result<Self, TestFailure> {
        let path = std::env::temp_dir().join(format!(
            "natsume-operator-application-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .map_err(|_| TestFailure::TestDatabaseCreationFailed)?;
        Ok(Self { database, path })
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", self.path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", self.path.display())));
    }
}

#[derive(Debug, Snafu)]
enum TestFailure {
    #[snafu(display("persisted operator roles did not remain closed"))]
    PersistedRolesWereNotClosed,
    #[snafu(display("a bootstrap input failure was expected"))]
    ExpectedInputFailure,
    #[snafu(display("a bootstrap input error exposed rejected context"))]
    InputErrorWasNotRedacted,
    #[snafu(display("valid bootstrap credentials were rejected"))]
    ValidCredentialsWereRejected,
    #[snafu(display("operator password hashing failed unexpectedly"))]
    PasswordHashingFailed,
    #[snafu(display("the generated password PHC string was invalid"))]
    GeneratedPhcWasInvalid,
    #[snafu(display("the generated password PHC string had no salt"))]
    GeneratedPhcHadNoSalt,
    #[snafu(display("the generated password PHC salt was invalid"))]
    GeneratedPhcSaltWasInvalid,
    #[snafu(display("the Argon2 password profile was not frozen"))]
    Argon2ProfileWasNotFrozen,
    #[snafu(display("the Argon2 password salt length was not frozen"))]
    Argon2SaltLengthWasNotFrozen,
    #[snafu(display("the frozen Argon2 parameters were rejected"))]
    FrozenParametersWereRejected,
    #[snafu(display("password verification did not use the frozen Argon2 profile"))]
    VerificationProfileWasNotFrozen,
    #[snafu(display("operator password verification failed"))]
    PasswordVerificationFailed,
    #[snafu(display("the password verification result contract changed"))]
    PasswordVerificationContractChanged,
    #[snafu(display("an incorrect operator password was accepted"))]
    IncorrectPasswordWasAccepted,
    #[snafu(display("a session credential could not be generated"))]
    CredentialGenerationFailed,
    #[snafu(display("the session credential wire encoding was invalid"))]
    CredentialWireEncodingWasInvalid,
    #[snafu(display("the session credential could not be decoded"))]
    CredentialDecodeFailed,
    #[snafu(display("the session credential round trip changed its bytes"))]
    CredentialRoundTripChangedBytes,
    #[snafu(display("the session credential hash did not use raw bytes"))]
    CredentialHashDidNotUseRawBytes,
    #[snafu(display("the session credential hash used hex text"))]
    CredentialHashUsedHexText,
    #[snafu(display("an invalid session credential was accepted"))]
    InvalidCredentialWasAccepted,
    #[snafu(display("session credential decode errors were distinguishable"))]
    CredentialDecodeErrorWasNotUnified,
    #[snafu(display("a session credential error exposed rejected context"))]
    CredentialErrorWasNotRedacted,
    #[snafu(display("the operator test database could not be created"))]
    TestDatabaseCreationFailed,
    #[snafu(display("an operator account fixture could not be inserted"))]
    AccountFixtureInsertFailed,
    #[snafu(display("correct operator credentials did not sign in"))]
    CorrectSignInFailed,
    #[snafu(display("the signed-in operator role was incorrect"))]
    SignedInRoleWasIncorrect,
    #[snafu(display("the signed-in session lifetime was not eight hours"))]
    SignedInSessionTtlWasNotFrozen,
    #[snafu(display("the new operator session did not authenticate"))]
    SessionAuthenticationFailed,
    #[snafu(display("the authenticated operator identity changed"))]
    AuthenticatedIdentityChanged,
    #[snafu(display("session persistence evidence could not be read"))]
    SessionEvidenceReadFailed,
    #[snafu(display("the persisted session credential evidence was invalid"))]
    PersistedCredentialEvidenceWasInvalid,
    #[snafu(display("a session credential escaped into the database"))]
    CredentialEscapedIntoDatabase,
    #[snafu(display("an invalid operator sign-in succeeded"))]
    InvalidSignInSucceeded,
    #[snafu(display("the password verification concurrency bound was invalid"))]
    VerificationGateBoundWasInvalid,
    #[snafu(display("the password verification gate was closed"))]
    VerificationGateWasClosed,
    #[snafu(display("the password verification gate did not bound concurrency"))]
    VerificationGateDidNotBoundConcurrency,
    #[snafu(display("the password verification gate did not release its permits"))]
    VerificationGateDidNotRelease,
    #[snafu(display("the password verification gate leaked permits"))]
    VerificationGateLeakedPermits,
    #[snafu(display("a non-frozen persisted password PHC authenticated"))]
    NonFrozenPhcAuthenticated,
    #[snafu(display("a non-frozen password PHC fixture could not be built"))]
    NonFrozenPhcFixtureFailed,
    #[snafu(display("the dummy plaintext authenticated an unknown login"))]
    DummyPlaintextAuthenticatedUnknownLogin,
    #[snafu(display("operator sign-in failures were distinguishable"))]
    SignInFailuresWereDistinguishable,
    #[snafu(display("a failed operator sign-in wrote rows"))]
    FailedSignInWroteRows,
    #[snafu(display("an operator sign-in error exposed rejected context"))]
    SignInErrorWasNotRedacted,
    #[snafu(display("the closed admin authorization policy was violated"))]
    AdminAuthorizationWasNotClosed,
    #[snafu(display("the fixed dummy password PHC string was invalid"))]
    DummyPhcWasInvalid,
    #[snafu(display("the fixed dummy password PHC profile was not frozen"))]
    DummyPhcProfileWasNotFrozen,
    #[snafu(display("the fixed dummy password PHC salt was not 16 bytes"))]
    DummyPhcSaltWasNotFrozen,
}
