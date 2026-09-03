use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::gateway_credentials,
};

pub(in crate::component::gateway) struct PersistedGatewayRow {
    credential_id: String,
    gateway_csr_der: Option<Vec<u8>>,
    gateway_leaf_der: Option<Vec<u8>>,
    issuer_chain_der: Option<Vec<u8>>,
}

impl PersistedGatewayRow {
    pub(in crate::component::gateway) fn new(
        credential_id: String,
        gateway_csr_der: Option<Vec<u8>>,
        gateway_leaf_der: Option<Vec<u8>>,
        issuer_chain_der: Option<Vec<u8>>,
    ) -> Self {
        Self {
            credential_id,
            gateway_csr_der,
            gateway_leaf_der,
            issuer_chain_der,
        }
    }

    pub(in crate::component::gateway) fn credential_id(&self) -> &str {
        &self.credential_id
    }

    pub(in crate::component::gateway) fn gateway_csr_der(&self) -> Option<&[u8]> {
        self.gateway_csr_der.as_deref()
    }

    pub(in crate::component::gateway) fn gateway_leaf_der(&self) -> Option<&[u8]> {
        self.gateway_leaf_der.as_deref()
    }

    pub(in crate::component::gateway) fn issuer_chain_der(&self) -> Option<&[u8]> {
        self.issuer_chain_der.as_deref()
    }
}

pub(in crate::component::gateway) fn find_by_device_id(
    transaction: &mut Transaction<'_>,
    device_id: &str,
) -> Result<Option<PersistedGatewayRow>, PersistenceError> {
    gateway_credentials::table
        .select((
            gateway_credentials::credential_id,
            gateway_credentials::gateway_csr_der,
            gateway_credentials::gateway_leaf_der,
            gateway_credentials::issuer_chain_der,
        ))
        .filter(gateway_credentials::device_id.eq(device_id))
        .first::<(String, Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>)>(
            transaction.connection(),
        )
        .optional()
        .map(|row| {
            row.map(|(credential_id, csr, leaf, chain)| {
                PersistedGatewayRow::new(credential_id, csr, leaf, chain)
            })
        })
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::gateway) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<(String, PersistedGatewayRow)>, PersistenceError> {
    gateway_credentials::table
        .select((
            gateway_credentials::device_id,
            gateway_credentials::credential_id,
            gateway_credentials::gateway_csr_der,
            gateway_credentials::gateway_leaf_der,
            gateway_credentials::issuer_chain_der,
        ))
        .load::<(
            String,
            String,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
        )>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(|(device_id, credential_id, csr, leaf, chain)| {
                    (
                        device_id,
                        PersistedGatewayRow::new(credential_id, csr, leaf, chain),
                    )
                })
                .collect()
        })
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::gateway) fn insert_initial_generation(
    transaction: &mut Transaction<'_>,
    device_id: &str,
    credential_id: &str,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(gateway_credentials::table)
        .values((
            gateway_credentials::device_id.eq(device_id),
            gateway_credentials::credential_id.eq(credential_id),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::gateway) fn accept_exact_csr(
    transaction: &mut Transaction<'_>,
    device_id: &str,
    credential_id: &str,
    gateway_csr_der: &[u8],
) -> Result<usize, PersistenceError> {
    diesel::update(
        gateway_credentials::table
            .filter(gateway_credentials::device_id.eq(device_id))
            .filter(gateway_credentials::credential_id.eq(credential_id))
            .filter(gateway_credentials::gateway_csr_der.is_null())
            .filter(gateway_credentials::gateway_leaf_der.is_null())
            .filter(gateway_credentials::issuer_chain_der.is_null()),
    )
    .set(gateway_credentials::gateway_csr_der.eq(Some(gateway_csr_der)))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::gateway) fn replace_generation(
    transaction: &mut Transaction<'_>,
    device_id: &str,
    current_credential_id: &str,
    replacement_credential_id: &str,
) -> Result<usize, PersistenceError> {
    diesel::update(
        gateway_credentials::table
            .filter(gateway_credentials::device_id.eq(device_id))
            .filter(gateway_credentials::credential_id.eq(current_credential_id)),
    )
    .set((
        gateway_credentials::credential_id.eq(replacement_credential_id),
        gateway_credentials::gateway_csr_der.eq(None::<&[u8]>),
        gateway_credentials::gateway_leaf_der.eq(None::<&[u8]>),
        gateway_credentials::issuer_chain_der.eq(None::<&[u8]>),
    ))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::gateway) fn store_exact_grant(
    transaction: &mut Transaction<'_>,
    device_id: &str,
    credential_id: &str,
    gateway_csr_der: &[u8],
    gateway_leaf_der: &[u8],
    issuer_chain_der: &[u8],
) -> Result<usize, PersistenceError> {
    diesel::update(
        gateway_credentials::table
            .filter(gateway_credentials::device_id.eq(device_id))
            .filter(gateway_credentials::credential_id.eq(credential_id))
            .filter(gateway_credentials::gateway_csr_der.eq(Some(gateway_csr_der)))
            .filter(gateway_credentials::gateway_leaf_der.is_null())
            .filter(gateway_credentials::issuer_chain_der.is_null()),
    )
    .set((
        gateway_credentials::gateway_leaf_der.eq(Some(gateway_leaf_der)),
        gateway_credentials::issuer_chain_der.eq(Some(issuer_chain_der)),
    ))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}
