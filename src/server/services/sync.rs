use std::future::Ready;

use actix_web::{
    FromRequest, HttpRequest, HttpResponse, Responder, dev::Payload, error::ErrorUnauthorized,
    post, web::Json,
};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};
use serde::{Deserialize, Serialize};
use tracing_unwrap::OptionExt;

#[derive(Deserialize)]
struct SyncRequestBody {
    mac: String,
}

#[derive(Serialize)]
struct SyncResponseBody {
    username: String,
    password: String,
}

pub struct Authenticated;

impl FromRequest for Authenticated {
    type Error = actix_web::Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        let config = crate::GLOBAL_CONFIG.get().expect("Config not initialized");

        match req.headers().get("token") {
            Some(header_value) => {
                if let Ok(token) = header_value.to_str() {
                    if token == config.server.token {
                        return std::future::ready(Ok(Authenticated));
                    }
                }
                std::future::ready(Err(ErrorUnauthorized("Invalid token")))
            }
            None => std::future::ready(Err(ErrorUnauthorized("Missing token header"))),
        }
    }
}

#[post("/sync")]
pub async fn sync_info(_auth: Authenticated, body: Json<SyncRequestBody>) -> impl Responder {
    let sync_enabled = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized!")
        .server
        .enable_sync;

    if !sync_enabled {
        tracing::warn!(
            "MAC {} try to sync info with sync service disabled!",
            body.mac
        );
        return HttpResponse::Forbidden()
            .body("Bind is not enabled! This request has been logged".to_string());
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

    use crate::server::schema::id_bind::dsl as id_bind_dsl;
    use crate::server::schema::player::dsl as player_dsl;
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
        Ok((username, password, _)) => response = SyncResponseBody { username, password },
        Err(err) => {
            tracing::error!("Failed to get info by ID from database, err: {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }
    tracing::info!("Synced MAC {} with user {}", body.mac, response.username);
    HttpResponse::Ok().json(response)
}
