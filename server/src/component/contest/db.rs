use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl, dsl::sql, sql_types::BigInt};

use crate::{
    component::{
        contest::{AccountFacts, BindingFacts, SeatFacts},
        device::DeviceId,
    },
    db::{PersistenceError, Transaction},
    diesel_schema::{accounts, device_bindings, seats},
};

pub(in crate::component::contest) fn list_seats(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<SeatFacts>, PersistenceError> {
    seats::table
        .select((seats::seat_id, seats::seat_code))
        .order(seats::seat_id)
        .load::<(String, String)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(|(seat_id, seat_code)| SeatFacts { seat_id, seat_code })
                .collect()
        })
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::contest) fn list_accounts(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<AccountFacts>, PersistenceError> {
    use crate::diesel_schema::{account_mappings, organizations, teams};
    use diesel::JoinOnDsl;
    let profiles = teams::table
        .inner_join(organizations::table)
        .inner_join(account_mappings::table.on(account_mappings::account_id.eq(teams::account_id)))
        .inner_join(seats::table.on(seats::seat_id.eq(account_mappings::seat_id)))
        .select((
            teams::account_id,
            seats::seat_id,
            seats::seat_code,
            teams::organization_id,
            teams::name_zh,
            teams::name_en,
            organizations::name_zh,
            organizations::name_en,
        ))
        .load::<(String, String, String, i64, String, String, String, String)>(
            transaction.connection(),
        )
        .map_err(|_| PersistenceError::OperationFailed)?;
    let mut profiles = profiles
        .into_iter()
        .map(
            |(
                account,
                seat_id,
                seat_code,
                organization_id,
                team_name_zh,
                team_name_en,
                school_name_zh,
                school_name_en,
            )| {
                (
                    account,
                    super::TeamDetails {
                        seat_id,
                        seat_code,
                        organization_id,
                        team_name_zh,
                        team_name_en,
                        school_name_zh,
                        school_name_en,
                    },
                )
            },
        )
        .collect::<std::collections::HashMap<_, _>>();
    accounts::table
        .select((
            accounts::account_id,
            accounts::domjudge_username,
            sql::<BigInt>("credential_revision"),
        ))
        .order(accounts::account_id)
        .load::<(String, String, i64)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(
                    |(account_id, domjudge_username, credential_revision)| AccountFacts {
                        team: profiles.remove(&account_id),
                        account_id,
                        domjudge_username,
                        credential_revision,
                    },
                )
                .collect()
        })
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::contest) fn list_bindings(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<BindingFacts>, PersistenceError> {
    let rows = device_bindings::table
        .select((
            device_bindings::binding_id,
            device_bindings::seat_id,
            device_bindings::device_id,
        ))
        .order(device_bindings::seat_id)
        .load::<(String, String, String)>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    rows.into_iter()
        .map(|(binding_id, seat_id, device_id)| {
            let device_id =
                DeviceId::parse(&device_id).ok_or(PersistenceError::InvalidPersistedData)?;
            Ok(BindingFacts {
                binding: binding_id,
                seat: seat_id,
                device: device_id,
            })
        })
        .collect()
}

pub(super) fn list_organizations(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<super::OrganizationDetails>, PersistenceError> {
    use crate::diesel_schema::{organizations, teams};
    let rows = organizations::table
        .filter(organizations::organization_id.eq_any(teams::table.select(teams::organization_id)))
        .select((
            organizations::organization_id,
            organizations::name_zh,
            organizations::name_en,
            organizations::country,
        ))
        .order(organizations::organization_id)
        .load::<(i64, String, String, String)>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    Ok(rows
        .into_iter()
        .map(
            |(organization_id, name_zh, name_en, country)| super::OrganizationDetails {
                organization_id,
                name_zh,
                name_en,
                country,
            },
        )
        .collect())
}

pub(super) fn export_roster(
    transaction: &mut Transaction<'_>,
) -> Result<super::roster::ExportRoster, PersistenceError> {
    use crate::diesel_schema::{account_mappings, server_vault_records, teams};
    use diesel::JoinOnDsl;
    let organizations = list_organizations(transaction)?;
    let count = accounts::table
        .count()
        .get_result::<i64>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    let rows = teams::table
        .inner_join(accounts::table)
        .inner_join(account_mappings::table.on(account_mappings::account_id.eq(teams::account_id)))
        .inner_join(seats::table.on(seats::seat_id.eq(account_mappings::seat_id)))
        .inner_join(
            server_vault_records::table.on(server_vault_records::account_id.eq(teams::account_id)),
        )
        .select((
            accounts::domjudge_username,
            seats::seat_code,
            teams::organization_id,
            teams::name_zh,
            teams::name_en,
            teams::category,
            server_vault_records::nonce,
            server_vault_records::ciphertext,
        ))
        .order(accounts::domjudge_username)
        .load::<(
            String,
            String,
            i64,
            String,
            String,
            String,
            Vec<u8>,
            Vec<u8>,
        )>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    if count != i64::try_from(rows.len()).map_err(|_| PersistenceError::InvalidPersistedData)? {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(super::roster::ExportRoster {
        organizations,
        teams: rows
            .into_iter()
            .map(
                |(
                    account,
                    seat,
                    organization_id,
                    name_zh,
                    name_en,
                    category,
                    nonce,
                    ciphertext,
                )| super::roster::ExportTeam {
                    account,
                    seat,
                    organization_id,
                    name_zh,
                    name_en,
                    category,
                    nonce,
                    ciphertext,
                },
            )
            .collect(),
    })
}
