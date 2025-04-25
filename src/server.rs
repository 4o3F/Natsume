use std::{fs, io::BufReader};

use actix_web::{
    App, HttpResponse, HttpServer,
    body::{BoxBody, MessageBody, to_bytes},
    dev::ServiceResponse,
    http::header,
    middleware::{ErrorHandlerResponse, ErrorHandlers},
};
use diesel::{
    dsl::{exists, insert_into, select, update},
    prelude::*,
};
use rustls::pki_types::PrivateKeyDer;
use rustls_pemfile::{certs, pkcs8_private_keys};
use serde::Deserialize;
use serde_json::json;
use tracing_unwrap::OptionExt;

mod database;
mod schema;
mod services;

pub fn add_error_header<B>(
    res: ServiceResponse<B>,
) -> actix_web::Result<ErrorHandlerResponse<BoxBody>>
where
    B: MessageBody + 'static,
    <B as MessageBody>::Error: actix_web::ResponseError,
{
    let (req, res) = res.into_parts();
    let status = res.status();
    let error_msg = status
        .canonical_reason()
        .unwrap_or("Unknown error")
        .to_string();

    let fut = async move {
        let body_bytes = to_bytes(res.into_body()).await?;

        let original_body_text = String::from_utf8_lossy(&body_bytes).to_string();

        let combined_body = json!({
            "msg": error_msg,
            "error": original_body_text
        });
        let new_response = HttpResponse::build(status)
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .body(combined_body.to_string());

        Ok(ServiceResponse::new(
            req,
            new_response.map_into_right_body(),
        ))
    };
    Ok(ErrorHandlerResponse::Future(Box::pin(fut)))
}

#[actix_web::main]
pub async fn serve() -> std::io::Result<()> {
    let server_config = super::GLOBAL_CONFIG.get().unwrap_or_log();

    database::init_database().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(["natsume.server".to_string()]).unwrap();
    let cert_file = cert.pem();
    let key_file = key_pair.serialize_pem();

    let cert_file = &mut BufReader::new(cert_file.as_bytes());
    let key_file = &mut BufReader::new(key_file.as_bytes());

    let cert_chain = certs(cert_file).collect::<Result<Vec<_>, _>>().unwrap();
    let mut keys = pkcs8_private_keys(key_file)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, PrivateKeyDer::Pkcs8(keys.remove(0)))
        .unwrap();

    if !fs::exists("./static")? {
        std::fs::create_dir("./static")?;
    }

    HttpServer::new(|| {
        App::new()
            .wrap(ErrorHandlers::new().default_handler(add_error_header))
            .service(services::get_ip)
            .service(services::bind_id)
            .service(services::get_status)
            .service(services::sync_info)
            .service(actix_files::Files::new("/static", "./static"))
    })
    .bind_rustls_0_23(("0.0.0.0", server_config.server.port), tls_config)?
    .run()
    .await
}

#[derive(Deserialize, Debug)]
struct PlayerInfo {
    id: String,
    username: String,
    password: String,
}

pub fn load_data(data_path: String) -> anyhow::Result<()> {
    let mut rdr = csv::Reader::from_path(data_path)?;
    let mut infos = Vec::<PlayerInfo>::new();
    for row in rdr.deserialize() {
        let row: PlayerInfo = row?;
        infos.push(row);
    }
    database::init_database()?;

    let connection_pool = database::DB_CONNECTION_POOL.get().unwrap_or_log();

    use schema::player::dsl::*;
    for info in infos {
        // Check if id key is present
        let mut connection = connection_pool.get()?;
        let exist =
            select(exists(player.filter(id.eq(&info.id)))).get_result::<bool>(&mut connection)?;

        if exist {
            // Data exist, this should happen between the warmup contest and official contest
            update(player.filter(id.eq(&info.id)))
                .set((
                    username.eq(&info.username),
                    password.eq(&info.password),
                    synced.eq(false as i32),
                ))
                .execute(&mut connection)?;
            tracing::info!("ID {} data exist, updated", &info.id);
        } else {
            // Data does not exist, inserting new one.
            // This should happen when first setup
            tracing::info!("ID {} data don't exist, inserting", &info.id);
            insert_into(player)
                .values((
                    id.eq(&info.id),
                    username.eq(&info.username),
                    password.eq(&info.password),
                    synced.eq(false as i32),
                ))
                .execute(&mut connection)?;
        }
    }
    Ok(())
}
