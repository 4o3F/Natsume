use ::windows::Security::Credentials::{PasswordCredential, PasswordVault};
use snafu::ResultExt as _;
use zeroize::Zeroizing;

use super::{CredentialSnafu, Result, Secrets};

/// Stores signing seeds and temporary enrollment credentials in the current user's vault.
pub struct VaultSecrets;

impl Secrets for VaultSecrets {
    fn load(&self, origin: &str, slot: &str) -> Result<Option<Zeroizing<String>>> {
        let vault = PasswordVault::new().context(CredentialSnafu)?;
        let credential =
            match vault.Retrieve(&format!("Natsume.Photo/{origin}").into(), &slot.into()) {
                Ok(credential) => credential,
                Err(error) if error.code() == ::windows::core::HRESULT::from_win32(1168) => {
                    return Ok(None);
                }
                Err(source) => return Err(source).context(CredentialSnafu),
            };
        credential.RetrievePassword().context(CredentialSnafu)?;
        Ok(Some(Zeroizing::new(
            credential.Password().context(CredentialSnafu)?.to_string(),
        )))
    }
    fn save(&self, origin: &str, slot: &str, value: &str) -> Result<()> {
        let vault = PasswordVault::new().context(CredentialSnafu)?;
        let credential = PasswordCredential::CreatePasswordCredential(
            &format!("Natsume.Photo/{origin}").into(),
            &slot.into(),
            &value.into(),
        )
        .context(CredentialSnafu)?;
        vault.Add(&credential).context(CredentialSnafu)
    }
}
