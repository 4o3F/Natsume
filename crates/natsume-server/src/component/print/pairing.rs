use std::{net::IpAddr, time::Instant};

use sha2::{Digest as _, Sha256};
use snafu::ensure;
use uuid::Uuid;

use super::{
    AuthenticationSnafu, InvalidSnafu, PrintComponent, PrintError, RateLimitedSnafu, canonical_id,
    db, lowercase_hash,
};
use crate::db::TransactionError;

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PrintPrinter {
    pub printer_id: String,
    pub name: String,
    pub client_id: Option<String>,
    pub client_confirmed: bool,
    #[specta(type = Option<specta_typescript::Number>)]
    pub pairing_expires_at_unix_ms: Option<i64>,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PrintPairingCode {
    pub printer_id: String,
    pub code: String,
    #[specta(type = specta_typescript::Number)]
    pub expires_at_unix_ms: i64,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PrintClientIssued {
    pub printer_id: String,
    pub client_id: String,
    pub token: String,
}

/// Bounded by the global admission limit; rejected traffic cannot grow the maps.
#[derive(Default)]
pub(super) struct PairingLimits(super::super::pairing::PairingBudget);
impl PairingLimits {
    pub(super) fn check(&mut self, source: IpAddr, now: Instant) -> Result<(), PrintError> {
        ensure!(self.0.admit(source, now), RateLimitedSnafu);
        Ok(())
    }
}

impl PrintComponent {
    pub(crate) fn admit_pairing(&self, source: IpAddr) -> Result<(), PrintError> {
        self.pairing_limits
            .lock()
            // Never retain the guard state; preserve the safe native poison diagnostic.
            .map_err(|source| {
                drop(source.into_inner());
                PrintError::StatePoisoned {
                    source: std::sync::PoisonError::new(()),
                }
            })?
            .check(source, Instant::now())
    }

    pub(crate) async fn create_printer(&self, name: String) -> Result<PrintPrinter, PrintError> {
        ensure!(
            (1..=128).contains(&name.chars().count()) && !name.chars().any(char::is_control),
            InvalidSnafu
        );
        let id = Uuid::now_v7().to_string();
        self.database
            .write(move |tx| {
                db::printers::insert(tx, &id, &name)?;
                db::printers::find(tx, &id)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn list_printers(&self) -> Result<Vec<PrintPrinter>, PrintError> {
        self.database
            .read(db::printers::list)
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn issue_pairing_code(
        &self,
        id: String,
    ) -> Result<PrintPairingCode, PrintError> {
        canonical_id(&id)?;
        let mut random = [0; 8];
        getrandom::fill(&mut random).map_err(|source| PrintError::Entropy { source })?;
        // Each uniformly random byte selects one of 32 symbols without modulo bias.
        let raw = super::super::pairing::code(random);
        let hash: [u8; 32] = Sha256::digest(raw.as_bytes()).into();
        self.database
            .write(move |tx| {
                let now = db::printers::now(tx)?;
                let expires_at_unix_ms = db::printers::issue_code(tx, &id, &hash, now)?;
                Ok(PrintPairingCode {
                    printer_id: id,
                    code: format!("{}-{}", &raw[..4], &raw[4..]),
                    expires_at_unix_ms,
                })
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn clear_printer_binding(&self, id: String) -> Result<(), PrintError> {
        canonical_id(&id)?;
        self.database
            .write(move |tx| db::printers::clear_binding(tx, &id))
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn pair_client(&self, code: &str) -> Result<PrintClientIssued, PrintError> {
        let normalized = normalize_code(code)?;
        let code_hash: [u8; 32] = Sha256::digest(normalized.as_bytes()).into();
        let client_id = Uuid::now_v7().to_string();
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).map_err(|source| PrintError::Entropy { source })?;
        let token = format!("npc1.{client_id}.{}", hex::encode(secret));
        let token_hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.database
            .write(move |tx| {
                let now = db::printers::now(tx)?;
                let printer_id = db::printers::pair(tx, &code_hash, &client_id, &token_hash, now)?;
                Ok(PrintClientIssued {
                    printer_id,
                    client_id,
                    token,
                })
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn confirm_client(&self, token: &str) -> Result<PrintPrinter, PrintError> {
        let mut parts = token.split('.');
        let (Some("npc1"), Some(client), Some(secret), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return AuthenticationSnafu.fail();
        };
        ensure!(
            canonical_id(client).is_ok() && lowercase_hash(secret),
            AuthenticationSnafu
        );
        let client = client.to_owned();
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.database
            .write(move |tx| db::printers::confirm(tx, &client, &hash))
            .await
            .map_err(TransactionError::into_error)
    }
}

fn normalize_code(value: &str) -> Result<String, PrintError> {
    super::super::pairing::normalize(value).ok_or_else(|| AuthenticationSnafu.build())
}
