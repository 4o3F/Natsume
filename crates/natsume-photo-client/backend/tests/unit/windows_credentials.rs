use super::*;
use crate::initialization::{CredentialError, SecretKind, SecretSlot};
use snafu::{Snafu, ensure};
#[derive(Debug, Snafu)]
enum TestError {
    Vault { source: CredentialError },
    Windows { source: ::windows::core::Error },
    Worker { source: tokio::task::JoinError },
    Mismatch,
}

#[tokio::test]
#[ignore = "creates and removes an isolated entry in the current Windows user's credential vault"]
async fn blocking_thread_round_trip_uses_and_removes_an_isolated_vault_entry()
-> std::result::Result<(), TestError> {
    tokio::task::spawn_blocking(|| {
        let origin = crate::initialization::ServerOrigin::parse(&format!(
            "https://photo-vault-test-{}.invalid",
            uuid::Uuid::now_v7()
        ))
        .unwrap_or_else(|error| panic!("valid isolated vault origin: {error:?}"));
        let slot = SecretSlot::new(
            crate::initialization::ClientId::generate(),
            SecretKind::Signing,
        );
        let value = Zeroizing::new(uuid::Uuid::now_v7().to_string());
        let vault = PasswordVault::new().context(WindowsSnafu)?;
        let result = VaultSecrets
            .save(&origin, &slot, &value)
            .and_then(|()| VaultSecrets.load(&origin, &slot));
        // Clean up even when reading fails; never touch another origin or slot.
        match vault.Retrieve(
            &format!("Natsume.Photo/{origin}").into(),
            &slot.storage_key().as_str().into(),
        ) {
            Ok(credential) => vault.Remove(&credential).context(WindowsSnafu)?,
            Err(error) if error.code() == ::windows::core::HRESULT::from_win32(1168) => {}
            Err(source) => return Err(source).context(WindowsSnafu),
        }
        ensure!(
            result.context(VaultSnafu)?.as_deref() == Some(&value),
            MismatchSnafu
        );
        ensure!(
            VaultSecrets
                .load(&origin, &slot)
                .context(VaultSnafu)?
                .is_none(),
            MismatchSnafu
        );
        Ok(())
    })
    .await
    .context(WorkerSnafu)?
}
