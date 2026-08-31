use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::super::{
    OperatorError,
    password::{self, OperatorPassword},
};

pub(in crate::component::operator) const SESSION_CREDENTIAL_LENGTH: usize = 32;
const SESSION_CREDENTIAL_HEX_LENGTH: usize = SESSION_CREDENTIAL_LENGTH * 2;
// The persisted hash width is the SHA-256 digest width and is validated by this
// component independently of the credential width.
const SESSION_CREDENTIAL_HASH_LENGTH: usize = 32;

pub(in crate::component::operator) struct SessionCredential(
    pub(in crate::component::operator) Zeroizing<[u8; SESSION_CREDENTIAL_LENGTH]>,
);

impl SessionCredential {
    pub(in crate::component::operator) fn generate() -> Result<Self, OperatorError> {
        let mut bytes = Zeroizing::new([0_u8; SESSION_CREDENTIAL_LENGTH]);
        getrandom::fill(&mut *bytes).map_err(|_| OperatorError::EntropyUnavailable)?;
        Ok(Self(bytes))
    }

    pub(in crate::component::operator) fn from_wire(
        wire: &SessionCredentialHex,
    ) -> Result<Self, OperatorError> {
        decode_lower_hex(wire.expose())
    }

    pub(in crate::component::operator) fn to_wire(&self) -> SessionCredentialHex {
        SessionCredentialHex(encode_lower_hex(&self.0))
    }

    pub(in crate::component::operator) fn sha256(&self) -> SessionCredentialHash {
        SessionCredentialHash(Zeroizing::new(Sha256::digest(self.0.as_slice()).into()))
    }
}

pub(crate) struct SessionCredentialHex(SecretString);

impl SessionCredentialHex {
    pub(crate) fn new(value: String) -> Self {
        Self(value.into())
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

pub(in crate::component::operator) struct SessionCredentialHash(
    Zeroizing<[u8; SESSION_CREDENTIAL_HASH_LENGTH]>,
);

impl SessionCredentialHash {
    pub(in crate::component::operator) fn as_bytes(&self) -> &[u8; SESSION_CREDENTIAL_HASH_LENGTH] {
        &self.0
    }
}

pub(crate) struct OperatorCredentials {
    login_name: String,
    password: OperatorPassword,
}

impl OperatorCredentials {
    /// Validates non-interactive operator credential input.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`OperatorError`] when the login name is empty or the
    /// two passwords differ.
    pub(crate) fn new(
        login_name: String,
        password: String,
        password_confirmation: String,
    ) -> Result<Self, OperatorError> {
        let password = OperatorPassword::new(password);
        let password_confirmation = OperatorPassword::new(password_confirmation);
        if login_name.is_empty() {
            return Err(OperatorError::EmptyLoginName);
        }
        if password.expose() != password_confirmation.expose() {
            return Err(OperatorError::PasswordMismatch);
        }
        Ok(Self {
            login_name,
            password,
        })
    }

    pub(crate) fn login_name(&self) -> &str {
        &self.login_name
    }

    pub(crate) fn hash_password(&self) -> Result<String, OperatorError> {
        password::hash_password(&self.password)
    }

    #[cfg(test)]
    pub(in crate::component::operator) fn password(&self) -> &OperatorPassword {
        &self.password
    }
}

fn encode_lower_hex(bytes: &[u8; SESSION_CREDENTIAL_LENGTH]) -> SecretString {
    hex::encode(bytes).into()
}

pub(in crate::component::operator) fn decode_lower_hex(
    value: &str,
) -> Result<SessionCredential, OperatorError> {
    if value.len() != SESSION_CREDENTIAL_HEX_LENGTH
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OperatorError::InvalidSessionCredential);
    }
    let mut bytes = Zeroizing::new([0_u8; SESSION_CREDENTIAL_LENGTH]);
    hex::decode_to_slice(value, &mut *bytes)
        .map_err(|_| OperatorError::InvalidSessionCredential)?;
    Ok(SessionCredential(bytes))
}
