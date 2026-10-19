use crate::db::{Database, PersistenceError, Transaction, TransactionError};

use super::{
    ActivationError, ControlAuthority, ControlPublicKey, DeviceError, DeviceId, DeviceState,
    EvidenceQuality, MachineHardwareId, db, types::DeviceRecord,
};

pub(super) async fn find_current_authority(
    database: &Database,
    machine_hardware_id: MachineHardwareId,
) -> Result<Option<ControlAuthority>, DeviceError> {
    database
        .read(move |transaction| {
            find_current_authority_in_transaction(transaction, &machine_hardware_id)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(DeviceError::from)
}

pub(super) async fn activate(
    database: &Database,
    machine_hardware_id: MachineHardwareId,
    candidate_public_key: ControlPublicKey,
    evidence_quality: EvidenceQuality,
) -> Result<ControlAuthority, ActivationError> {
    database
        .write(move |transaction| {
            activate_in_transaction(
                transaction,
                &machine_hardware_id,
                &candidate_public_key,
                evidence_quality,
            )
        })
        .await
        .map_err(TransactionError::into_error)
}

fn find_current_authority_in_transaction(
    transaction: &mut Transaction<'_>,
    machine_hardware_id: &MachineHardwareId,
) -> Result<Option<ControlAuthority>, PersistenceError> {
    let Some(device) = db::find_non_revoked_by_machine(transaction, machine_hardware_id)? else {
        return Ok(None);
    };
    let current_key = current_key_for_device(transaction, &device)?;
    ControlAuthority::new(device.device_id(), current_key, device.state())
        .ok_or(PersistenceError::InvalidPersistedData)
        .map(Some)
}

fn activate_in_transaction(
    transaction: &mut Transaction<'_>,
    machine_hardware_id: &MachineHardwareId,
    candidate_public_key: &ControlPublicKey,
    evidence_quality: EvidenceQuality,
) -> Result<ControlAuthority, ActivationError> {
    let current_device = db::find_non_revoked_by_machine(transaction, machine_hardware_id)?;
    let current_key = current_device
        .as_ref()
        .map(|device| current_key_for_device(transaction, device))
        .transpose()?;

    if let (Some(device), Some(key)) = (&current_device, &current_key)
        && *key == *candidate_public_key
    {
        return authority_for(device.device_id(), *key, device.state());
    }

    // A public key is never recycled. In particular, a retired key cannot be
    // approved again for either its old Device or another Machine Hardware ID.
    if db::public_key_exists(transaction, candidate_public_key)? {
        return Err(ActivationError::CandidateKeyRejected);
    }

    let now = db::current_unix_ms(transaction)?;
    if let (Some(device), Some(current_key)) = (current_device, current_key) {
        require_one(db::update_evidence(
            transaction,
            &device.device_id(),
            evidence_quality,
        )?)?;
        require_one(db::retire_current(
            transaction,
            &current_key,
            &device.device_id(),
            now,
        )?)?;
        require_one(db::insert_current(
            transaction,
            candidate_public_key,
            &device.device_id(),
            now,
        )?)?;
        return authority_for(device.device_id(), *candidate_public_key, device.state());
    }

    let device_id = DeviceId::new();
    require_one(db::insert(
        transaction,
        &device_id,
        machine_hardware_id,
        evidence_quality,
        now,
    )?)?;
    require_one(db::insert_current(
        transaction,
        candidate_public_key,
        &device_id,
        now,
    )?)?;
    authority_for(device_id, *candidate_public_key, DeviceState::Enabled)
}

fn authority_for(
    device_id: DeviceId,
    public_key: ControlPublicKey,
    state: DeviceState,
) -> Result<ControlAuthority, ActivationError> {
    ControlAuthority::new(device_id, public_key, state)
        .ok_or(ActivationError::InvalidAuthorityFacts)
}

fn current_key_for_device(
    transaction: &mut Transaction<'_>,
    device: &DeviceRecord,
) -> Result<ControlPublicKey, PersistenceError> {
    db::find_current_for_device(transaction, &device.device_id())?
        .ok_or(PersistenceError::InvalidPersistedData)
}

fn require_one(updated: usize) -> Result<(), PersistenceError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(PersistenceError::InvalidPersistedData)
    }
}
