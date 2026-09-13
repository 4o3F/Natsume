use std::collections::BTreeMap;

use diesel::{ExpressionMethods, JoinOnDsl, NullableExpressionMethods, QueryDsl, RunQueryDsl};

use super::super::roster::TeamDetails;
use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::{account_mappings, accounts, seats, teams},
};

pub(in crate::component::import) fn read(
    transaction: &mut Transaction<'_>,
) -> Result<BTreeMap<String, TeamDetails>, PersistenceError> {
    let rows = teams::table
        .inner_join(accounts::table)
        .left_join(account_mappings::table.on(account_mappings::account_id.eq(teams::account_id)))
        .left_join(seats::table.on(seats::seat_id.eq(account_mappings::seat_id)))
        .select((
            accounts::domjudge_username,
            seats::seat_code.nullable(),
            teams::organization_id,
            teams::name_zh,
            teams::name_en,
            teams::category,
        ))
        .load::<(String, Option<String>, i64, String, String, String)>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    Ok(rows
        .into_iter()
        .map(
            |(account, seat, organization_id, name_zh, name_en, category)| {
                (
                    account.clone(),
                    TeamDetails {
                        account,
                        seat,
                        organization_id,
                        name_zh,
                        name_en,
                        category,
                    },
                )
            },
        )
        .collect())
}

pub(in crate::component::import) fn save(
    transaction: &mut Transaction<'_>,
    account_id: &str,
    team: &TeamDetails,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(teams::table)
        .values((
            teams::account_id.eq(account_id),
            teams::organization_id.eq(team.organization_id),
            teams::name_zh.eq(&team.name_zh),
            teams::name_en.eq(&team.name_en),
            teams::category.eq(&team.category),
        ))
        .on_conflict(teams::account_id)
        .do_update()
        .set((
            teams::organization_id.eq(team.organization_id),
            teams::name_zh.eq(&team.name_zh),
            teams::name_en.eq(&team.name_en),
            teams::category.eq(&team.category),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}
