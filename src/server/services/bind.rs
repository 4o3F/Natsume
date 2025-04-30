use actix_web::{HttpRequest, HttpResponse, Responder, post, web::Json};
use chrono::Utc;
use diesel::{
    dsl::{exists, insert_into, select, update},
    prelude::*,
};
use serde::Deserialize;
use tracing_unwrap::OptionExt;

#[derive(Deserialize)]
struct BindRequestBody {
    mac: String,
    id: String,
    #[serde(default)]
    client_version: Option<String>,
}
#[post("/bind")]
pub async fn bind_id(req: HttpRequest, body: Json<BindRequestBody>) -> impl Responder {
    let client_ip;
    if let Some(value) = req.peer_addr() {
        client_ip = value.ip().to_string();
    } else {
        tracing::error!("No IP can be extracted, this SHOULD NOT HAPPEN");
        return HttpResponse::InternalServerError().finish();
    }

    let bind_enabled = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized!")
        .server
        .enable_bind;

    if !bind_enabled {
        tracing::warn!(
            "MAC {} try to bind to ID {} with bind service disabled!",
            body.mac,
            body.id
        );
        return HttpResponse::Forbidden()
            .body("Bind is not enabled! This request has been logged".to_string());
    }

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
            return HttpResponse::InternalServerError().finish();
        }
    }

    use crate::server::schema::id_bind::dsl as id_bind_dsl;
    let exist;
    match select(exists(
        id_bind_dsl::id_bind.filter(id_bind_dsl::mac.eq(&body.mac)),
    ))
    .get_result::<bool>(&mut connection)
    {
        Ok(result) => exist = result,
        Err(err) => {
            tracing::error!("Error fetching from database {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    let timestamp = Utc::now().timestamp().to_string();
    if exist {
        // Update bind id
        match update(id_bind_dsl::id_bind.filter(id_bind_dsl::mac.eq(&body.mac)))
            .set((
                id_bind_dsl::id.eq(&body.id),
                id_bind_dsl::ip.eq(&client_ip),
                id_bind_dsl::client_version.eq(&body.client_version.as_deref().unwrap_or_default()),
                id_bind_dsl::last_seen.eq(&timestamp),
            ))
            .execute(&mut connection)
        {
            Ok(_) => {
                tracing::info!("Updated MAC {} ID {}", body.mac, body.id);
                HttpResponse::Ok().finish()
            }
            Err(err) => {
                tracing::error!(
                    "Error updating MAC {} with ID {}, err {}",
                    body.mac,
                    body.id,
                    err
                );
                HttpResponse::InternalServerError().finish()
            }
        }
    } else {
        // Insert new binding
        match insert_into(id_bind_dsl::id_bind)
            .values((
                id_bind_dsl::mac.eq(&body.mac),
                id_bind_dsl::id.eq(&body.id),
                id_bind_dsl::ip.eq(&client_ip),
                id_bind_dsl::client_version.eq(&body.client_version.as_deref().unwrap_or_default()),
                id_bind_dsl::last_seen.eq(&timestamp),
            ))
            .execute(&mut connection)
        {
            Ok(_) => {
                tracing::info!("Updated MAC {} ID {}", body.mac, body.id);
                HttpResponse::Ok().finish()
            }
            Err(err) => {
                tracing::error!(
                    "Error inserting MAC {} with ID {}, err {}",
                    body.mac,
                    body.id,
                    err
                );
                HttpResponse::InternalServerError().finish()
            }
        }
    }
}
