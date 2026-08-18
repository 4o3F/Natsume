use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use uuid::Uuid;

use crate::{
    application::device::{DevicePersistenceError, credentials::NewGatewayCertificate},
    db::{Transaction, schema::gateway_certificates},
};

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    certificate_id: Uuid,
    device_id: Uuid,
    enrollment_request_id: Uuid,
    certificate: &NewGatewayCertificate,
) -> Result<(), DevicePersistenceError> {
    let inserted = diesel::insert_into(gateway_certificates::table)
        .values((
            gateway_certificates::certificate_id.eq(certificate_id.to_string()),
            gateway_certificates::device_pk.eq(device_id.to_string()),
            gateway_certificates::enrollment_request_id.eq(enrollment_request_id.to_string()),
            gateway_certificates::serial.eq(certificate.serial()),
            gateway_certificates::spki_sha256.eq(certificate.spki_sha256().as_slice()),
            gateway_certificates::not_after.eq(certificate.not_after()),
            gateway_certificates::status.eq("active"),
        ))
        .execute(transaction.connection())
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    if inserted != 1 {
        return Err(DevicePersistenceError::PersistenceFailed);
    }
    Ok(())
}

pub(crate) fn retire_active(
    transaction: &mut Transaction<'_>,
    device_id: Uuid,
) -> Result<i64, DevicePersistenceError> {
    let retired = diesel::update(
        gateway_certificates::table
            .filter(gateway_certificates::device_pk.eq(device_id.to_string()))
            .filter(gateway_certificates::status.eq("active")),
    )
    .set(gateway_certificates::status.eq("retired"))
    .execute(transaction.connection())
    .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    i64::try_from(retired).map_err(|_| DevicePersistenceError::PersistenceFailed)
}

pub(crate) fn revoke_non_revoked(
    transaction: &mut Transaction<'_>,
    device_id: &str,
) -> Result<i64, DevicePersistenceError> {
    let revoked = diesel::update(
        gateway_certificates::table
            .filter(gateway_certificates::device_pk.eq(device_id))
            .filter(gateway_certificates::status.ne("revoked")),
    )
    .set(gateway_certificates::status.eq("revoked"))
    .execute(transaction.connection())
    .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    i64::try_from(revoked).map_err(|_| DevicePersistenceError::InvalidPersistedFacts)
}

pub(crate) fn active_count(
    transaction: &mut Transaction<'_>,
    device_id: Uuid,
) -> Result<i64, DevicePersistenceError> {
    gateway_certificates::table
        .filter(gateway_certificates::device_pk.eq(device_id.to_string()))
        .filter(gateway_certificates::status.eq("active"))
        .count()
        .get_result::<i64>(transaction.connection())
        .map_err(|_| DevicePersistenceError::PersistenceFailed)
}
