use actix_web::{HttpResponse, Responder, post, web::Json};
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
}
#[post("/bind")]
pub async fn bind_id(body: Json<BindRequestBody>) -> impl Responder {
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

    use crate::server::schema::id_bind::dsl::*;
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
