use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::{component::device::DeviceId, db::PersistenceError};

use super::{write_field, write_optional_field};

/// Import-owned snapshot used by preview, stale detection, and atomic commit planning.
pub(super) struct ImportBaseline {
    seats: BTreeMap<String, BaselineSeat>,
    accounts: BTreeMap<String, BaselineAccount>,
}

impl ImportBaseline {
    pub(in crate::component::import) fn new(
        seats: Vec<BaselineSeat>,
        accounts: Vec<BaselineAccount>,
    ) -> Result<Self, PersistenceError> {
        let mut seats_by_code = BTreeMap::new();
        for seat in seats {
            if seats_by_code
                .insert(seat.seat_code().to_owned(), seat)
                .is_some()
            {
                return Err(PersistenceError::InvalidPersistedData);
            }
        }

        let mut accounts_by_username = BTreeMap::new();
        for account in accounts {
            if account.credential_revision() < 1
                || accounts_by_username
                    .insert(account.domjudge_username().to_owned(), account)
                    .is_some()
            {
                return Err(PersistenceError::InvalidPersistedData);
            }
        }

        Ok(Self {
            seats: seats_by_code,
            accounts: accounts_by_username,
        })
    }

    pub(super) fn fingerprint(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        write_field(&mut hasher, b"natsume/import-baseline/v1");
        for seat in self.seats.values() {
            write_field(&mut hasher, b"seat");
            write_field(&mut hasher, seat.seat_id().as_bytes());
            write_field(&mut hasher, seat.seat_code().as_bytes());
            write_optional_field(
                &mut hasher,
                seat.current_domjudge_username().map(str::as_bytes),
            );
            let device_id = seat.device_id().map(DeviceId::as_text);
            write_optional_field(&mut hasher, device_id.as_deref().map(str::as_bytes));
        }
        for account in self.accounts.values() {
            write_field(&mut hasher, b"account");
            write_field(&mut hasher, account.account_id().as_bytes());
            write_field(&mut hasher, account.domjudge_username().as_bytes());
            write_field(&mut hasher, &account.credential_revision().to_be_bytes());
        }
        hasher.finalize().into()
    }

    pub(super) const fn seats(&self) -> &BTreeMap<String, BaselineSeat> {
        &self.seats
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        BTreeMap<String, BaselineSeat>,
        BTreeMap<String, BaselineAccount>,
    ) {
        (self.seats, self.accounts)
    }
}

pub(super) struct BaselineSeat {
    seat_id: String,
    seat_code: String,
    current_domjudge_username: Option<String>,
    device_id: Option<DeviceId>,
}

impl BaselineSeat {
    pub(in crate::component::import) const fn new(
        seat_id: String,
        seat_code: String,
        current_domjudge_username: Option<String>,
        device_id: Option<DeviceId>,
    ) -> Self {
        Self {
            seat_id,
            seat_code,
            current_domjudge_username,
            device_id,
        }
    }

    pub(super) fn seat_id(&self) -> &str {
        &self.seat_id
    }

    pub(super) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    pub(super) fn current_domjudge_username(&self) -> Option<&str> {
        self.current_domjudge_username.as_deref()
    }

    pub(super) const fn device_id(&self) -> Option<&DeviceId> {
        self.device_id.as_ref()
    }
}

pub(super) struct BaselineAccount {
    account_id: String,
    domjudge_username: String,
    credential_revision: i64,
}

impl BaselineAccount {
    pub(in crate::component::import) const fn new(
        account_id: String,
        domjudge_username: String,
        credential_revision: i64,
    ) -> Self {
        Self {
            account_id,
            domjudge_username,
            credential_revision,
        }
    }

    pub(super) fn account_id(&self) -> &str {
        &self.account_id
    }

    pub(super) fn domjudge_username(&self) -> &str {
        &self.domjudge_username
    }

    pub(super) const fn credential_revision(&self) -> i64 {
        self.credential_revision
    }
}
