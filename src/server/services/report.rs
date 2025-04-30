use actix_web::{HttpRequest, HttpResponse, Responder, post, web::Json};
use chrono::Utc;
use diesel::dsl::{count_star, insert_into, update};
use diesel::prelude::*;
use serde::Deserialize;
use tracing_unwrap::OptionExt;

use crate::server::schema::id_bind::dsl as id_bind_dsl;
use crate::server::schema::player::dsl as player_dsl;

#[derive(Deserialize)]
struct ReportStatusRequest {
    mac: String,
    synced: bool,
    #[serde(default)]
    client_version: Option<String>,
}

#[post("/report")]
pub async fn report_status(req: HttpRequest, report: Json<ReportStatusRequest>) -> impl Responder {
    let client_ip;
    if let Some(value) = req.peer_addr() {
        client_ip = value.ip().to_string();
    } else {
        tracing::error!("No IP can be extracted, this SHOULD NOT HAPPEN");
        return HttpResponse::InternalServerError().finish();
    }

    for (i, c) in report.mac.chars().enumerate() {
        tracing::debug!("MAC char {}: {:?} (U+{:04X})", i, c, c as u32);
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

    let mut insert_unknown = false;

    match id_bind_dsl::id_bind
        .filter(id_bind_dsl::mac.eq(&report.mac))
        .select(count_star())
        .first::<i64>(&mut connection)
    {
        Ok(result) => {
            tracing::debug!("MAC {} count result: {}", report.mac, result);
            if result == 0 {
                tracing::warn!(
                    "Unbinded MAC {} reporting from IP {} with version '{}'! Logging as unknown",
                    report.mac,
                    client_ip,
                    report.client_version.as_deref().unwrap_or_default()
                );
                insert_unknown = true;
            }
        }
        Err(err) => {
            tracing::error!("Error checking MAC {} existance, err {}", report.mac, err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    let timestamp = Utc::now().timestamp().to_string();
    if insert_unknown {
        match insert_into(id_bind_dsl::id_bind)
            .values((
                id_bind_dsl::mac.eq(&report.mac),
                id_bind_dsl::id.eq("UNKNOWN"),
                id_bind_dsl::ip.eq(&client_ip),
                id_bind_dsl::client_version
                    .eq(&report.client_version.as_deref().unwrap_or_default()),
                id_bind_dsl::last_seen.eq(&timestamp),
            ))
            .execute(&mut connection)
        {
            Ok(_) => {
                tracing::info!("Logging unbinded MAC with ID as unknown");
                return HttpResponse::Ok().finish();
            }
            Err(err) => {
                tracing::error!("Failed to log unbinded MAC with ID as unknown, err {}", err);
                return HttpResponse::InternalServerError()
                    .body("Failed to log unbinded MAC with ID as unknown");
            }
        }
    }

    // Update client IP addr
    match update(id_bind_dsl::id_bind.filter(id_bind_dsl::mac.eq(&report.mac)))
        .set((
            id_bind_dsl::ip.eq(&client_ip),
            id_bind_dsl::client_version.eq(&report.client_version.as_deref().unwrap_or_default()),
            id_bind_dsl::last_seen.eq(&timestamp),
        ))
        .execute(&mut connection)
    {
        Ok(_) => {}
        Err(err) => {
            tracing::error!(
                "Error updating IP {} with MAC {}, err {}",
                client_ip,
                report.mac,
                err
            );
            return HttpResponse::InternalServerError().finish();
        }
    }

    if report.synced {
        match update(player_dsl::player)
            .filter(
                player_dsl::id.eq_any(
                    id_bind_dsl::id_bind
                        .select(id_bind_dsl::id)
                        .filter(id_bind_dsl::mac.eq(&report.mac)),
                ),
            )
            .set(player_dsl::synced.eq(true as i32))
            .execute(&mut connection)
        {
            Ok(_) => {}
            Err(err) => {
                tracing::error!("Error updating synced with MAC {}, err {}", report.mac, err);
                return HttpResponse::InternalServerError().finish();
            }
        }
    }

    tracing::info!("MAC {} heartbeat received!", report.mac);

    HttpResponse::Ok().finish()
}
