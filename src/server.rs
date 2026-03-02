use std::{fs, io::BufReader, path::Path};

use actix_cors::Cors;
use actix_web::{
    App, HttpResponse, HttpServer,
    body::{BoxBody, MessageBody, to_bytes},
    dev::ServiceResponse,
    http::header,
    middleware::{ErrorHandlerResponse, ErrorHandlers},
    web,
};
use diesel::{
    dsl::{exists, insert_into, select, update},
    prelude::*,
};
use rcgen::{CertificateParams, Issuer, KeyPair};
use rustls::pki_types::PrivateKeyDer;
use rustls_pemfile::{certs, pkcs8_private_keys};
use serde::Deserialize;
use serde_json::json;
use services::spa_handler;
use tracing_unwrap::OptionExt;

mod database;
mod schema;
mod services;

fn ensure_parent_dir(path: &str) -> std::io::Result<()> {
    if let Some(parent) = Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    Ok(())
}

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

    database::init_database().map_err(std::io::Error::other)?;

    let ca_cert_pem =
        fs::read_to_string(&server_config.server.tls_ca_cert_path).map_err(|err| {
            std::io::Error::other(format!(
                "Failed to read CA certificate {}: {err}",
                server_config.server.tls_ca_cert_path
            ))
        })?;
    let ca_key_pem = fs::read_to_string(&server_config.server.tls_ca_key_path).map_err(|err| {
        std::io::Error::other(format!(
            "Failed to read CA key {}: {err}",
            server_config.server.tls_ca_key_path
        ))
    })?;

    let ca_key = KeyPair::from_pem(&ca_key_pem).map_err(std::io::Error::other)?;
    let ca_issuer =
        Issuer::from_ca_cert_pem(&ca_cert_pem, ca_key).map_err(std::io::Error::other)?;

    let tls_cert_path = &server_config.server.tls_cert_path;
    let tls_key_path = &server_config.server.tls_key_path;
    let cached_cert = fs::read_to_string(tls_cert_path);
    let cached_key = fs::read_to_string(tls_key_path);

    let (cert_chain_pem, key_pem) = match (cached_cert, cached_key) {
        (Ok(cert_pem), Ok(key_pem)) => {
            tracing::info!(
                "Using cached issued server TLS cert {} and key {}",
                tls_cert_path,
                tls_key_path
            );
            (cert_pem, key_pem)
        }
        _ => {
            tracing::info!(
                "Cached issued server TLS cert/key not found, issuing and persisting a new one"
            );

            let mut certificate_params = CertificateParams::new(vec!["natsume.server".to_string()])
                .map_err(std::io::Error::other)?;
            certificate_params.is_ca = rcgen::IsCa::NoCa;
            certificate_params.use_authority_key_identifier_extension = true;
            certificate_params
                .extended_key_usages
                .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);

            let signing_key = KeyPair::generate().map_err(std::io::Error::other)?;
            let cert = certificate_params
                .signed_by(&signing_key, &ca_issuer)
                .map_err(std::io::Error::other)?;

            let cert_chain_pem = format!("{}\n{ca_cert_pem}", cert.pem());
            let key_pem = signing_key.serialize_pem();

            ensure_parent_dir(tls_cert_path)?;
            ensure_parent_dir(tls_key_path)?;

            fs::write(tls_cert_path, &cert_chain_pem).map_err(|err| {
                std::io::Error::other(format!(
                    "Failed to persist issued TLS certificate {}: {err}",
                    tls_cert_path
                ))
            })?;
            fs::write(tls_key_path, &key_pem).map_err(|err| {
                std::io::Error::other(format!(
                    "Failed to persist issued TLS key {}: {err}",
                    tls_key_path
                ))
            })?;

            tracing::info!(
                "Persisted issued server TLS cert to {} and key to {}",
                tls_cert_path,
                tls_key_path
            );

            (cert_chain_pem, key_pem)
        }
    };

    let cert_file = &mut BufReader::new(cert_chain_pem.as_bytes());
    let key_file = &mut BufReader::new(key_pem.as_bytes());

    let cert_chain = certs(cert_file).collect::<Result<Vec<_>, _>>()?;
    let mut keys = pkcs8_private_keys(key_file).collect::<Result<Vec<_>, _>>()?;
    let key = keys.pop().ok_or_else(|| {
        std::io::Error::other("No PKCS#8 private key generated for server certificate")
    })?;

    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, PrivateKeyDer::Pkcs8(key))
        .map_err(std::io::Error::other)?;

    if !fs::exists("./static")? {
        std::fs::create_dir("./static")?;
    }

    HttpServer::new(|| {
        let mut app = App::new()
            .wrap(ErrorHandlers::new().default_handler(add_error_header))
            .wrap(Cors::permissive())
            .service(services::get_ip)
            .service(services::bind_id)
            .service(services::report_status)
            .service(services::get_status)
            .service(services::sync_info)
            .service(services::remove_bind)
            .service(web::scope("/panel").default_service(web::to(spa_handler)));
        let static_file_enabled = crate::GLOBAL_CONFIG
            .get()
            .unwrap()
            .server
            .enable_static_file;
        if static_file_enabled {
            app = app.service(actix_files::Files::new("/static", "./static"));
        }
        app
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
