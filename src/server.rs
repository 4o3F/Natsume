use actix_web::{App, HttpServer};
use anyhow::Ok;
use diesel::{
    dsl::{exists, insert_into, select, update},
    prelude::*,
};
use serde::Deserialize;
use tracing_unwrap::OptionExt;

mod database;
mod schema;
mod services;

#[actix_web::main]
pub async fn serve() -> std::io::Result<()> {
    let server_config = super::GLOBAL_CONFIG.get().unwrap_or_log();

    database::init_database().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    HttpServer::new(|| App::new().service(services::get_ip))
        .bind(("0.0.0.0", server_config.server.port))?
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
                .set((username.eq(&info.username), password.eq(&info.password)))
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
                ))
                .execute(&mut connection)?;
        }
    }
    Ok(())
}
