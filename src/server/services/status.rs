use actix_web::{HttpResponse, Responder, get};
use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl, dsl::count_star};
use serde::Serialize;
use tracing_unwrap::OptionExt;

#[get("/status")]
pub async fn get_status() -> impl Responder {
    #[derive(Serialize)]
    struct StatusResponse {
        bind_count: i64,
        info_count: i64,
        sync_count: i64,
        not_synced: Vec<String>,
    }
    let connection_pool = crate::server::database::DB_CONNECTION_POOL
        .get()
        .unwrap_or_log();

    let mut connection;
    match connection_pool.get() {
        Ok(conn) => {
            connection = conn;
        }
        Err(err) => {
            tracing::error!("Error getting database connection {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    let mut response_body = StatusResponse {
        bind_count: 0,
        info_count: 0,
        sync_count: 0,
        not_synced: Vec::new(),
    };

    use crate::server::schema::id_bind::dsl::*;
    use crate::server::schema::player::dsl::*;
    // Get total bind count
    match id_bind
        .select(count_star())
        .get_result::<i64>(&mut connection)
    {
        Ok(count) => response_body.bind_count = count,
        Err(err) => {
            tracing::error!("Error counting bind {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    match player
        .select(count_star())
        .get_result::<i64>(&mut connection)
    {
        Ok(count) => response_body.info_count = count,
        Err(err) => {
            tracing::error!("Error counting player info {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    match player
        .filter(synced.eq(true as i32))
        .count()
        .get_result::<i64>(&mut connection)
    {
        Ok(count) => response_body.sync_count = count,
        Err(err) => {
            tracing::error!("Error counting synced player info {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    match player
        .filter(synced.eq(0))
        .select(crate::server::schema::player::columns::id)
        .load::<String>(&mut connection)
    {
        Ok(count) => response_body.not_synced = count,
        Err(err) => {
            tracing::error!("Error counting unsynced player info {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    HttpResponse::Ok().json(response_body)
}
