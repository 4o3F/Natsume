use actix_web::{HttpRequest, HttpResponse, Responder, get, post, web::Json};
use diesel::{
    ExpressionMethods,
    dsl::{count_star, exists, insert_into, update},
    select,
};
use serde::{Deserialize, Serialize};
use tracing_unwrap::OptionExt;

use diesel::prelude::*;

use crate::server::schema;

#[get("/ip")]
pub async fn get_ip(req: HttpRequest) -> impl Responder {
    if let Some(value) = req.peer_addr() {
        let ip = value.ip().to_string();
        tracing::info!("Address {:?} requested for IP", ip);
        #[derive(Serialize)]
        struct Response {
            ip: String,
        }
        HttpResponse::Ok().json(Response { ip })
    } else {
        tracing::error!("No IP can be extracted, this SHOULD NOT HAPPEN");
        unreachable!()
    }
}

#[get("/status")]
pub async fn status() -> impl Responder {
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

    use super::schema::id_bind::dsl::*;
    use super::schema::player::dsl::*;
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
        .select(schema::player::columns::id)
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

#[derive(Deserialize)]
struct BindRequestBody {
    mac: String,
    id: String,
}
#[post("/bind")]
pub async fn bind(body: Json<BindRequestBody>) -> impl Responder {
    tracing::info!("MAC {} ID {}", body.mac, body.id);
    // Write to database
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
            return HttpResponse::InternalServerError();
        }
    }

    use super::schema::id_bind::dsl::*;
    let exist;
    match select(exists(id_bind.filter(mac.eq(&body.mac)))).get_result::<bool>(&mut connection) {
        Ok(result) => exist = result,
        Err(err) => {
            tracing::error!("Error fetching from database {}", err);
            return HttpResponse::InternalServerError();
        }
    }

    if exist {
        // Update bind id
        match update(id_bind.filter(mac.eq(&body.mac)))
            .set(id.eq(&body.id))
            .execute(&mut connection)
        {
            Ok(_) => {
                tracing::info!("Updated MAC {} ID {}", body.mac, body.id);
                HttpResponse::Ok()
            }
            Err(err) => {
                tracing::error!(
                    "Error updating MAC {} with ID {}, err {}",
                    body.mac,
                    body.id,
                    err
                );
                return HttpResponse::InternalServerError();
            }
        }
    } else {
        // Insert new binding
        match insert_into(id_bind)
            .values((mac.eq(&body.mac), id.eq(&body.id)))
            .execute(&mut connection)
        {
            Ok(_) => {
                tracing::info!("Updated MAC {} ID {}", body.mac, body.id);
                HttpResponse::Ok()
            }
            Err(err) => {
                tracing::error!(
                    "Error inserting MAC {} with ID {}, err {}",
                    body.mac,
                    body.id,
                    err
                );
                return HttpResponse::InternalServerError();
            }
        }
    }
}

#[derive(Deserialize)]
struct SyncRequestBody {
    mac: String,
    /// IV should be in BASE64 format
    iv: String,
}
#[post("/sync")]
pub async fn sync(body: Json<SyncRequestBody>) -> impl Responder {
    #[derive(Serialize)]
    struct SyncResponseBody {
        username: String,
        password: String,
    }

    tracing::info!("Received sync request from MAC {}, IV {}", body.mac, body.iv);
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

    use super::schema::id_bind::dsl as id_bind_dsl;
    use super::schema::player::dsl as player_dsl;
    let id;
    match id_bind_dsl::id_bind
        .filter(id_bind_dsl::mac.eq(&body.mac))
        .select(id_bind_dsl::id)
        .first::<String>(&mut connection)
        .optional()
    {
        Ok(result) => match result {
            Some(result) => id = result,
            None => {
                return HttpResponse::Forbidden().body("No ID bind to this MAC!");
            }
        },
        Err(err) => {
            tracing::error!("Failed to get ID by MAC from database, err: {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    let response: SyncResponseBody;
    match player_dsl::player
        .filter(player_dsl::id.eq(&id))
        .select((
            player_dsl::username,
            player_dsl::password,
            player_dsl::synced,
        ))
        .first::<(String, String, i32)>(&mut connection)
    {
        // TODO: consider should we introduce check for sync status? perhaps we should only return once for syncing?
        Ok((username, password, _)) => response = SyncResponseBody { username, password },
        Err(err) => {
            tracing::error!("Failed to get info by ID from database, err: {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }
    HttpResponse::Ok().json(response)
}
