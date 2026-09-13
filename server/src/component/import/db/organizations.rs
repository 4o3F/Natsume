use std::collections::BTreeMap;

use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, QueryableByName, RunQueryDsl, sql_types::BigInt,
};

use super::super::roster::OrganizationDetails;
use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::organizations,
};

pub(in crate::component::import) fn read(
    transaction: &mut Transaction<'_>,
) -> Result<BTreeMap<String, OrganizationDetails>, PersistenceError> {
    let rows = organizations::table
        .select((
            organizations::organization_id,
            organizations::name_key,
            organizations::name_zh,
            organizations::name_en,
            organizations::country,
        ))
        .order(organizations::organization_id)
        .load::<(i64, String, String, String, String)>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    let mut result = BTreeMap::new();
    for (organization_id, key, name_zh, name_en, country) in rows {
        let details = OrganizationDetails {
            organization_id,
            name_zh,
            name_en,
            country,
        };
        if organization_id < 1
            || key.is_empty()
            || details.key() != key
            || result.insert(key, details).is_some()
        {
            return Err(PersistenceError::InvalidPersistedData);
        }
    }
    Ok(result)
}

pub(in crate::component::import) fn sequence(
    transaction: &mut Transaction<'_>,
) -> Result<i64, PersistenceError> {
    #[derive(QueryableByName)]
    struct Sequence {
        #[diesel(sql_type = BigInt)]
        seq: i64,
    }
    let value = diesel::sql_query("SELECT seq FROM sqlite_sequence WHERE name = 'organizations'")
        .get_result::<Sequence>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .map_or(0, |row| row.seq);
    if value < 0 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(value)
}

pub(in crate::component::import) fn save(
    transaction: &mut Transaction<'_>,
    school: &OrganizationDetails,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(organizations::table)
        .values((
            organizations::organization_id.eq(school.organization_id),
            organizations::name_key.eq(school.key()),
            organizations::name_zh.eq(&school.name_zh),
            organizations::name_en.eq(&school.name_en),
            organizations::country.eq(&school.country),
        ))
        .on_conflict(organizations::organization_id)
        .do_update()
        .set((
            organizations::name_zh.eq(&school.name_zh),
            organizations::name_en.eq(&school.name_en),
            organizations::country.eq(&school.country),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::import) fn delete(
    transaction: &mut Transaction<'_>,
    id: i64,
) -> Result<usize, PersistenceError> {
    diesel::delete(organizations::table.filter(organizations::organization_id.eq(id)))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}
