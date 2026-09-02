mod db;
mod issuer;

use std::{path::Path, sync::Arc};

use sha2::{Digest as _, Sha256};
use snafu::Snafu;
use uuid::{Uuid, Variant, Version};
use x509_parser::{certificate::X509Certificate, prelude::FromDer as _};

use crate::{
    component::device::DeviceId,
    config::GatewaySiteConfig,
    db::{Database, PersistenceError, Transaction, TransactionError},
};

use self::issuer::{GatewayIssuer, GatewayIssuerError};

pub(crate) struct GatewayComponent {
    database: Database,
    issuer: Arc<GatewayIssuer>,
}

impl GatewayComponent {
    fn new(database: Database, issuer: Arc<GatewayIssuer>) -> Self {
        Self { database, issuer }
    }

    pub(crate) fn load(
        database: Database,
        ca_certificate_path: &Path,
        ca_private_key_path: &Path,
        packaged_trust_root_path: &Path,
        site: &GatewaySiteConfig,
    ) -> Result<Self, GatewayLoadError> {
        GatewayIssuer::load(
            ca_certificate_path,
            ca_private_key_path,
            packaged_trust_root_path,
            site,
        )
        .map(|issuer| Self::new(database, Arc::new(issuer)))
        .map_err(map_load_error)
    }

    #[cfg(test)]
    pub(crate) fn for_test(database: Database) -> Result<Self, GatewayLoadError> {
        GatewayIssuer::for_test()
            .map(|(issuer, _)| Self::new(database, Arc::new(issuer)))
            .map_err(map_load_error)
    }

    pub(crate) async fn ingest(
        &self,
        device_id: DeviceId,
        input: Option<GatewayCredentialInput>,
        actual: GatewayActualState,
    ) -> Result<(), GatewayError> {
        if let Some(csr_der) = input.as_ref().and_then(GatewayCredentialInput::csr_der) {
            GatewayIssuer::validate_csr(csr_der).map_err(|_| GatewayError::InvalidCsr)?;
        }

        let initial_id = GatewayCredentialId::new();
        let replacement_id = GatewayCredentialId::new();
        self.database
            .write(move |transaction| {
                ingest_in_transaction(
                    transaction,
                    &device_id,
                    initial_id,
                    replacement_id,
                    input.as_ref(),
                    &actual,
                )
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn materialize(
        &self,
        device_id: DeviceId,
    ) -> Result<MaterializedGateway, GatewayError> {
        self.ensure_generation(device_id).await?;
        let mut fact = self.read_current_fact(device_id).await?;

        if fact
            .grant
            .as_ref()
            .map(grant_is_expired)
            .transpose()?
            .unwrap_or(false)
        {
            self.replace_if_current(device_id, fact.credential_id)
                .await?;
            fact = self.read_current_fact(device_id).await?;
        }

        let Some(csr_der) = fact.csr_der.as_deref() else {
            return Ok(resolve(fact));
        };
        if fact.grant.is_some() {
            return Ok(resolve(fact));
        }

        let issued = self
            .issuer
            .issue(fact.credential_id.value(), csr_der)
            .map_err(map_persisted_issuer_error)?;
        let leaf_der = issued.into_leaf_der();

        let expected_id = fact.credential_id;
        let expected_csr = csr_der.to_vec();
        self.database
            .write(move |transaction| {
                let updated = db::store_exact_grant(
                    transaction,
                    &device_id.as_text(),
                    &expected_id.as_text(),
                    &expected_csr,
                    &leaf_der,
                    &[],
                )?;
                if updated > 1 {
                    return Err(GatewayError::InvalidPersistedFacts);
                }
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)?;

        self.read_current_fact(device_id).await.map(resolve)
    }

    async fn ensure_generation(&self, device_id: DeviceId) -> Result<(), GatewayError> {
        let initial_id = GatewayCredentialId::new();
        self.database
            .write(move |transaction| {
                if db::find_by_device_id(transaction, &device_id.as_text())?.is_none() {
                    require_one(db::insert_initial_generation(
                        transaction,
                        &device_id.as_text(),
                        &initial_id.as_text(),
                    )?)?;
                }
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)
    }

    async fn read_current_fact(&self, device_id: DeviceId) -> Result<GatewayFact, GatewayError> {
        self.database
            .read(move |transaction| {
                let row = db::find_by_device_id(transaction, &device_id.as_text())?
                    .ok_or(GatewayError::InvalidPersistedFacts)?;
                GatewayFact::from_persisted(&row)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    async fn replace_if_current(
        &self,
        device_id: DeviceId,
        expected_id: GatewayCredentialId,
    ) -> Result<(), GatewayError> {
        let replacement_id = GatewayCredentialId::new();
        self.database
            .write(move |transaction| {
                replace_generation(
                    transaction,
                    &device_id.as_text(),
                    expected_id,
                    replacement_id,
                )?;
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)
    }
}

fn ingest_in_transaction(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    initial_id: GatewayCredentialId,
    replacement_id: GatewayCredentialId,
    input: Option<&GatewayCredentialInput>,
    actual: &GatewayActualState,
) -> Result<(), GatewayError> {
    let device_id_text = device_id.as_text();
    let row = if let Some(row) = db::find_by_device_id(transaction, &device_id_text)? {
        row
    } else {
        require_one(db::insert_initial_generation(
            transaction,
            &device_id_text,
            &initial_id.as_text(),
        )?)?;
        db::PersistedGatewayRow::new(initial_id.as_text(), None, None, None)
    };
    let mut fact = GatewayFact::from_persisted(&row)?;

    if actual.requires_replacement(&fact) {
        require_change(replace_generation(
            transaction,
            &device_id_text,
            fact.credential_id,
            replacement_id,
        )?)?;
        fact = GatewayFact::waiting(replacement_id);
    }

    let Some(input) = input else {
        return Ok(());
    };
    if input.credential_id != fact.credential_id {
        return Ok(());
    }
    let Some(csr_der) = input.csr_der() else {
        require_change(replace_generation(
            transaction,
            &device_id_text,
            fact.credential_id,
            replacement_id,
        )?)?;
        return Ok(());
    };

    match fact.csr_der.as_deref() {
        None => require_one(db::accept_exact_csr(
            transaction,
            &device_id_text,
            &fact.credential_id.as_text(),
            csr_der,
        )?),
        Some(current) if current == csr_der => Ok(()),
        Some(_) => Err(GatewayError::ConflictingCsr),
    }
}

fn replace_generation(
    transaction: &mut Transaction<'_>,
    device_id: &str,
    current_id: GatewayCredentialId,
    replacement_id: GatewayCredentialId,
) -> Result<bool, GatewayError> {
    let updated = db::replace_generation(
        transaction,
        device_id,
        &current_id.as_text(),
        &replacement_id.as_text(),
    )?;
    match updated {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(GatewayError::InvalidPersistedFacts),
    }
}

fn require_change(changed: bool) -> Result<(), GatewayError> {
    if changed {
        Ok(())
    } else {
        Err(GatewayError::InvalidPersistedFacts)
    }
}

fn require_one(updated: usize) -> Result<(), GatewayError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(GatewayError::InvalidPersistedFacts)
    }
}

fn resolve(fact: GatewayFact) -> MaterializedGateway {
    let intent = GatewayIntent {
        credential_id: fact.credential_id,
    };
    let target = GatewayTarget {
        credential_id: fact.credential_id,
        certificate: fact.grant,
    };
    MaterializedGateway { intent, target }
}

fn grant_is_expired(grant: &GatewayCertificateGrant) -> Result<bool, GatewayError> {
    let (remainder, certificate) = X509Certificate::from_der(&grant.leaf_der)
        .map_err(|_| GatewayError::InvalidPersistedFacts)?;
    if !remainder.is_empty() {
        return Err(GatewayError::InvalidPersistedFacts);
    }
    Ok(certificate.validity().not_after.timestamp()
        <= time::OffsetDateTime::now_utc().unix_timestamp())
}

fn map_persisted_issuer_error(error: GatewayIssuerError) -> GatewayError {
    if error.is_invalid_csr() {
        GatewayError::InvalidPersistedFacts
    } else {
        GatewayError::IssuanceFailed
    }
}

fn map_load_error(error: GatewayIssuerError) -> GatewayLoadError {
    if error.is_trust_root_mismatch() {
        GatewayLoadError::TrustRootMismatch
    } else {
        GatewayLoadError::OriginCa
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GatewayCredentialId(Uuid);

impl GatewayCredentialId {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let parsed = Uuid::parse_str(value).ok()?;
        if parsed.hyphenated().to_string() != value
            || parsed.get_version() != Some(Version::SortRand)
            || parsed.get_variant() != Variant::RFC4122
        {
            return None;
        }
        Some(Self(parsed))
    }

    fn new() -> Self {
        Self(Uuid::now_v7())
    }

    const fn value(self) -> Uuid {
        self.0
    }

    pub(crate) fn as_text(self) -> String {
        self.0.hyphenated().to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayCredentialInput {
    credential_id: GatewayCredentialId,
    csr_der: Option<Vec<u8>>,
}

impl GatewayCredentialInput {
    pub(crate) const fn new(credential_id: GatewayCredentialId, csr_der: Option<Vec<u8>>) -> Self {
        Self {
            credential_id,
            csr_der,
        }
    }

    fn csr_der(&self) -> Option<&[u8]> {
        self.csr_der.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GatewayActualState {
    Absent,
    Tracking {
        credential_id: GatewayCredentialId,
    },
    Loaded {
        credential_id: GatewayCredentialId,
        leaf_sha256: [u8; 32],
    },
    RecoveryRequired {
        credential_id: GatewayCredentialId,
    },
}

impl GatewayActualState {
    fn requires_replacement(self, fact: &GatewayFact) -> bool {
        match self {
            Self::Absent | Self::Tracking { .. } => false,
            Self::RecoveryRequired { credential_id } => credential_id == fact.credential_id,
            Self::Loaded {
                credential_id,
                leaf_sha256,
            } => {
                credential_id == fact.credential_id
                    && fact.grant.as_ref().is_none_or(|grant| {
                        let expected: [u8; 32] = Sha256::digest(&grant.leaf_der).into();
                        expected != leaf_sha256
                    })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GatewayFact {
    credential_id: GatewayCredentialId,
    csr_der: Option<Vec<u8>>,
    grant: Option<GatewayCertificateGrant>,
}

impl GatewayFact {
    fn waiting(credential_id: GatewayCredentialId) -> Self {
        Self {
            credential_id,
            csr_der: None,
            grant: None,
        }
    }

    fn from_persisted(row: &db::PersistedGatewayRow) -> Result<Self, GatewayError> {
        let credential_id = GatewayCredentialId::parse(row.credential_id())
            .ok_or(GatewayError::InvalidPersistedFacts)?;
        let csr_der = row.gateway_csr_der().map(<[u8]>::to_vec);
        let grant = match (
            csr_der.as_ref(),
            row.gateway_leaf_der(),
            row.issuer_chain_der(),
        ) {
            (None | Some(_), None, None) => None,
            (Some(_), Some(leaf_der), Some([])) => Some(GatewayCertificateGrant {
                leaf_der: leaf_der.to_vec(),
                issuer_chain_der: Vec::new(),
            }),
            _ => return Err(GatewayError::InvalidPersistedFacts),
        };
        Ok(Self {
            credential_id,
            csr_der,
            grant,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayIntent {
    credential_id: GatewayCredentialId,
}

impl GatewayIntent {
    pub(crate) const fn credential_id(&self) -> GatewayCredentialId {
        self.credential_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayCertificateGrant {
    leaf_der: Vec<u8>,
    issuer_chain_der: Vec<Vec<u8>>,
}

impl GatewayCertificateGrant {
    pub(crate) fn leaf_der(&self) -> &[u8] {
        &self.leaf_der
    }

    pub(crate) fn issuer_chain_der(&self) -> &[Vec<u8>] {
        &self.issuer_chain_der
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayTarget {
    credential_id: GatewayCredentialId,
    certificate: Option<GatewayCertificateGrant>,
}

impl GatewayTarget {
    pub(crate) const fn credential_id(&self) -> GatewayCredentialId {
        self.credential_id
    }

    pub(crate) const fn certificate(&self) -> Option<&GatewayCertificateGrant> {
        self.certificate.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaterializedGateway {
    intent: GatewayIntent,
    target: GatewayTarget,
}

impl MaterializedGateway {
    pub(crate) const fn intent(&self) -> &GatewayIntent {
        &self.intent
    }

    pub(crate) const fn target(&self) -> &GatewayTarget {
        &self.target
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum GatewayError {
    #[snafu(display("the Gateway CSR is invalid"))]
    InvalidCsr,
    #[snafu(display("the current Gateway CSR conflicts with the accepted CSR"))]
    ConflictingCsr,
    #[snafu(display("persisted Gateway facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Gateway persistence failed"))]
    PersistenceFailed,
    #[snafu(display("Gateway certificate issuance failed"))]
    IssuanceFailed,
}

impl From<PersistenceError> for GatewayError {
    fn from(error: PersistenceError) -> Self {
        match error {
            PersistenceError::InvalidPersistedData => Self::InvalidPersistedFacts,
            PersistenceError::OperationFailed => Self::PersistenceFailed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum GatewayLoadError {
    #[snafu(display("Origin CA loading or validation failed"))]
    OriginCa,
    #[snafu(display("Origin CA issuing certificate and packaged trust root differ"))]
    TrustRootMismatch,
}

impl GatewayLoadError {
    pub(crate) const fn is_trust_root_mismatch(self) -> bool {
        matches!(self, Self::TrustRootMismatch)
    }
}

#[cfg(test)]
mod tests;
