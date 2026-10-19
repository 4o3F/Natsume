use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::{component::device::DeviceId, db::PersistenceError};

use super::{
    roster::{OrganizationDetails, TeamDetails},
    write_field, write_optional_field,
};

/// Import-owned snapshot used by preview, stale detection, and atomic commit planning.
pub(super) struct ImportBaseline {
    pub(super) seats: BTreeMap<String, BaselineSeat>,
    pub(super) accounts: BTreeMap<String, BaselineAccount>,
    pub(super) organizations: BTreeMap<String, OrganizationDetails>,
    pub(super) teams: BTreeMap<String, TeamDetails>,
    pub(super) organization_sequence: i64,
}

impl ImportBaseline {
    pub(in crate::component::import) fn new(
        seats: Vec<BaselineSeat>,
        accounts: Vec<BaselineAccount>,
        organizations: BTreeMap<String, OrganizationDetails>,
        teams: BTreeMap<String, TeamDetails>,
        organization_sequence: i64,
    ) -> Result<Self, PersistenceError> {
        if organizations
            .values()
            .any(|school| school.organization_id > organization_sequence)
        {
            return Err(PersistenceError::InvalidPersistedData);
        }
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
            organizations,
            teams,
            organization_sequence,
        })
    }

    pub(super) fn fingerprint(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        write_field(&mut hasher, b"natsume/import-baseline/v2");
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
            write_field(&mut hasher, &account.nonce);
            write_field(&mut hasher, &account.ciphertext);
        }
        write_field(&mut hasher, &self.organization_sequence.to_be_bytes());
        for organization in self.organizations.values() {
            write_field(&mut hasher, b"organization");
            write_field(&mut hasher, &organization.organization_id.to_be_bytes());
            for value in [
                &organization.name_zh,
                &organization.name_en,
                &organization.country,
            ] {
                write_field(&mut hasher, value.as_bytes());
            }
        }
        for team in self.teams.values() {
            write_field(&mut hasher, b"team");
            write_field(&mut hasher, team.account.as_bytes());
            write_optional_field(&mut hasher, team.seat.as_deref().map(str::as_bytes));
            write_field(&mut hasher, &team.organization_id.to_be_bytes());
            for value in [&team.name_zh, &team.name_en, &team.category] {
                write_field(&mut hasher, value.as_bytes());
            }
        }
        hasher.finalize().into()
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
    pub(super) nonce: Vec<u8>,
    pub(super) ciphertext: Vec<u8>,
}

impl BaselineAccount {
    pub(in crate::component::import) const fn new(
        account_id: String,
        domjudge_username: String,
        credential_revision: i64,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Self {
        Self {
            account_id,
            domjudge_username,
            credential_revision,
            nonce,
            ciphertext,
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
