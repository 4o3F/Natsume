use super::*;
use snafu::ensure;

#[tokio::test]
#[ignore = "creates and removes an isolated entry in the current Windows user's credential vault"]
async fn blocking_thread_round_trip_uses_and_removes_an_isolated_vault_entry() -> Result<()> {
    tokio::task::spawn_blocking(|| {
        let origin = format!("https://photo-vault-test-{}.invalid", uuid::Uuid::now_v7());
        let slot = "isolated/signing";
        let value = Zeroizing::new(uuid::Uuid::now_v7().to_string());
        let vault = PasswordVault::new().context(CredentialSnafu)?;
        let result = VaultSecrets
            .save(&origin, slot, &value)
            .and_then(|()| VaultSecrets.load(&origin, slot));
        // Clean up even when reading fails; never touch another origin or slot.
        match vault.Retrieve(&format!("Natsume.Photo/{origin}").into(), &slot.into()) {
            Ok(credential) => vault.Remove(&credential).context(CredentialSnafu)?,
            Err(error) if error.code() == ::windows::core::HRESULT::from_win32(1168) => {}
            Err(source) => return Err(source).context(CredentialSnafu),
        }
        ensure!(
            result?.as_deref() == Some(&value),
            super::super::InvalidSnafu
        );
        ensure!(
            VaultSecrets.load(&origin, slot)?.is_none(),
            super::super::InvalidSnafu
        );
        Ok(())
    })
    .await
    .context(super::super::WorkerSnafu)?
}
