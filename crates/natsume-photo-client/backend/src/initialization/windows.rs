use ::windows::Security::Credentials::{PasswordCredential, PasswordVault};
use snafu::ResultExt as _;
use zeroize::Zeroizing;

use super::secrets::WindowsSnafu;
use super::{CredentialResult, SecretSlot, Secrets};

/// Stores signing seeds and temporary enrollment credentials in the current user's vault.
#[derive(Clone, Copy)]
pub struct VaultSecrets;

impl Secrets for VaultSecrets {
    fn load(&self, origin: &str, slot: &SecretSlot) -> CredentialResult<Option<Zeroizing<String>>> {
        let slot = slot.storage_key();
        let vault = PasswordVault::new().context(WindowsSnafu)?;
        let credential = match vault.Retrieve(
            &format!("Natsume.Photo/{origin}").into(),
            &slot.as_str().into(),
        ) {
            Ok(credential) => credential,
            Err(error) if error.code() == ::windows::core::HRESULT::from_win32(1168) => {
                return Ok(None);
            }
            Err(source) => return Err(source).context(WindowsSnafu),
        };
        credential.RetrievePassword().context(WindowsSnafu)?;
        Ok(Some(Zeroizing::new(
            credential.Password().context(WindowsSnafu)?.to_string(),
        )))
    }
    fn save(&self, origin: &str, slot: &SecretSlot, value: &str) -> CredentialResult<()> {
        let slot = slot.storage_key();
        let vault = PasswordVault::new().context(WindowsSnafu)?;
        let credential = PasswordCredential::CreatePasswordCredential(
            &format!("Natsume.Photo/{origin}").into(),
            &slot.as_str().into(),
            &value.into(),
        )
        .context(WindowsSnafu)?;
        vault.Add(&credential).context(WindowsSnafu)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/windows_credentials.rs"]
mod tests;
