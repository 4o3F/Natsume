use actix_web::{HttpRequest, HttpResponse, Responder, post, web::Json};
use diesel::dsl::{count_star, update};
use diesel::prelude::*;
use serde::Deserialize;
use tracing_unwrap::OptionExt;

use crate::server::schema::id_bind::dsl as id_bind_dsl;
use crate::server::schema::player::dsl as player_dsl;

#[derive(Deserialize)]
struct ReportStatusRequest {
    mac: String,
    synced: bool,
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

    match id_bind_dsl::id_bind
        .filter(id_bind_dsl::mac.eq(&report.mac))
        .select(count_star())
        .execute(&mut connection)
    {
        Ok(result) => {
            if result == 0 {
                tracing::warn!(
                    "Unbinded MAC {} reporting from IP {}!",
                    report.mac,
                    client_ip
                );
                return HttpResponse::Forbidden().body("Unknown MAC");
            }
        }
        Err(err) => {
            tracing::error!("Error checking MAC {} existance, err {}", report.mac, err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    // Update client IP addr
    match update(id_bind_dsl::id_bind.filter(id_bind_dsl::mac.eq(&report.mac)))
        .set(id_bind_dsl::ip.eq(&client_ip))
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

    HttpResponse::Ok().finish()
}
