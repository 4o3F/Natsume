use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{
        Error as PasswordHashError, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
    },
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Semaphore;

use super::OperatorError;

const ARGON2_MEMORY_COST_KIB: u32 = 19_456;
const ARGON2_TIME_COST: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;
const PASSWORD_SALT_LENGTH: usize = 16;
pub(in crate::application::operator) const DUMMY_PASSWORD_PHC: &str = "$argon2id$v=19$m=19456,t=2,p=1$\
    bmF0c3VtZS1kdW1teS1zbA$KQCQGYQS75NixY7KaNGTRwtboqGCDKN5SXQjAFrx+7w";
// One in-flight verification holds `ARGON2_MEMORY_COST_KIB` of memory, and the
// unauthenticated sign-in path always performs one, so without a bound the
// blocking pool lets anonymous callers allocate hundreds at once. Both inputs
// are frozen: the Argon2 memory cost above and the roughly three operator
// browsers a site runs. Device Enrollment and Device WSS never verify a
// password, so fleet size cannot raise this bound.
pub(in crate::application::operator) const PASSWORD_VERIFICATION_CONCURRENCY: usize = 4;

pub(in crate::application::operator) static PASSWORD_VERIFICATION_GATE: Semaphore =
    Semaphore::const_new(PASSWORD_VERIFICATION_CONCURRENCY);

pub(crate) struct OperatorPassword(SecretString);

impl OperatorPassword {
    pub(in crate::application::operator) fn new(value: String) -> Self {
        Self(value.into())
    }

    pub(in crate::application::operator) fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

/// Hashes an operator password with the frozen Argon2id profile.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] if entropy, parameter construction,
/// salt encoding, or password hashing fails.
pub(crate) fn hash_password(password: &OperatorPassword) -> Result<String, OperatorError> {
    let params = Params::new(
        ARGON2_MEMORY_COST_KIB,
        ARGON2_TIME_COST,
        ARGON2_PARALLELISM,
        None,
    )
    .map_err(|_| OperatorError::InvalidHashingParameters)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut salt_bytes = [0_u8; PASSWORD_SALT_LENGTH];
    getrandom::fill(&mut salt_bytes).map_err(|_| OperatorError::EntropyUnavailable)?;
    let salt =
        SaltString::encode_b64(&salt_bytes).map_err(|_| OperatorError::SaltEncodingFailed)?;
    argon2
        .hash_password(password.expose().as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| OperatorError::PasswordHashingFailed)
}

pub(in crate::application::operator) fn verify_password_once(
    password: &OperatorPassword,
    candidate_phc: &str,
) -> Result<bool, OperatorError> {
    let (parsed, persisted_profile_valid) =
        if let Some(parsed) = parse_frozen_password_hash(candidate_phc) {
            (parsed, true)
        } else {
            let dummy = parse_frozen_password_hash(DUMMY_PASSWORD_PHC)
                .ok_or(OperatorError::PasswordVerificationFailed)?;
            (dummy, false)
        };
    let verifier = frozen_argon2()?;

    let verification_succeeded =
        match verifier.verify_password(password.expose().as_bytes(), &parsed) {
            Ok(()) => true,
            Err(PasswordHashError::Password) => false,
            Err(_) => return Err(OperatorError::PasswordVerificationFailed),
        };

    Ok(persisted_profile_valid && verification_succeeded)
}

fn parse_frozen_password_hash(value: &str) -> Option<PasswordHash<'_>> {
    let parsed = PasswordHash::new(value).ok()?;
    if parsed.algorithm.as_str() != "argon2id"
        || parsed.version != Some(19)
        || parsed.params.iter().count() != 3
        || parsed.params.get_decimal("m") != Some(19_456)
        || parsed.params.get_decimal("t") != Some(2)
        || parsed.params.get_decimal("p") != Some(1)
    {
        return None;
    }
    let salt = parsed.salt?;
    let mut salt_bytes = [0_u8; PASSWORD_SALT_LENGTH];
    if salt.decode_b64(&mut salt_bytes).ok()?.len() != PASSWORD_SALT_LENGTH {
        return None;
    }
    Some(parsed)
}

fn frozen_argon2() -> Result<Argon2<'static>, OperatorError> {
    let params = Params::new(
        ARGON2_MEMORY_COST_KIB,
        ARGON2_TIME_COST,
        ARGON2_PARALLELISM,
        None,
    )
    .map_err(|_| OperatorError::InvalidHashingParameters)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}
