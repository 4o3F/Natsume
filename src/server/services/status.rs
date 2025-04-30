use std::future::Ready;

use actix_web::{
    FromRequest, HttpRequest, HttpResponse, Responder, dev::Payload, error::ErrorUnauthorized, get,
};
use diesel::{
    ExpressionMethods, JoinOnDsl, NullableExpressionMethods, QueryDsl, RunQueryDsl,
    dsl::count_star, prelude::Queryable,
};
use serde::Serialize;
use tracing_unwrap::OptionExt;

#[derive(Serialize)]
struct StatusResponse {
    bind_count: i64,
    info_count: i64,
    sync_count: i64,
    notsync_count: i64,
    infos: Vec<Info>,
}

#[derive(Serialize, Queryable)]
struct Info {
    mac: Option<String>,
    id: String,
    ip: Option<String>,
    client_version: Option<String>,
    last_seen: Option<String>,
    username: Option<String>,
    password: Option<String>,
    synced: Option<bool>,
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
                    if token == config.server.panel_token {
                        return std::future::ready(Ok(Authenticated));
                    }
                }
                std::future::ready(Err(ErrorUnauthorized("Invalid token")))
            }
            None => std::future::ready(Err(ErrorUnauthorized("Missing token header"))),
        }
    }
}

#[get("/status")]
pub async fn get_status(_auth: Authenticated) -> impl Responder {
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
        notsync_count: 0,
        infos: Vec::new(),
    };

    use crate::server::schema::id_bind::dsl as id_bind_dsl;
    use crate::server::schema::player::dsl as player_dsl;
    // Get total bind count
    match id_bind_dsl::id_bind
        .select(count_star())
        .get_result::<i64>(&mut connection)
    {
        Ok(count) => response_body.bind_count = count,
        Err(err) => {
            tracing::error!("Error counting bind {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    match player_dsl::player
        .select(count_star())
        .get_result::<i64>(&mut connection)
    {
        Ok(count) => response_body.info_count = count,
        Err(err) => {
            tracing::error!("Error counting player info {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    match player_dsl::player
        .filter(player_dsl::synced.eq(true as i32))
        .count()
        .get_result::<i64>(&mut connection)
    {
        Ok(count) => response_body.sync_count = count,
        Err(err) => {
            tracing::error!("Error counting synced player info {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    match player_dsl::player
        .filter(player_dsl::synced.eq(0))
        .select(count_star())
        .get_result::<i64>(&mut connection)
    {
        Ok(count) => response_body.notsync_count = count,
        Err(err) => {
            tracing::error!("Error counting unsynced player info {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    }

    let mut infos = match id_bind_dsl::id_bind
        .left_outer_join(player_dsl::player.on(id_bind_dsl::id.eq(player_dsl::id)))
        .select((
            id_bind_dsl::mac.nullable(),
            id_bind_dsl::id,
            id_bind_dsl::ip.nullable(),
            id_bind_dsl::client_version.nullable(),
            id_bind_dsl::last_seen.nullable(),
            player_dsl::username.nullable(),
            player_dsl::password.nullable(),
            player_dsl::synced.nullable(),
        ))
        .load::<(
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i32>,
        )>(&mut connection)
    {
        Ok(result) => result
            .into_iter()
            .map(|x| Info {
                mac: x.0,
                id: x.1,
                ip: x.2,
                client_version: x.3,
                last_seen: x.4,
                username: x.5,
                password: x.6,
                synced: x.7.map(|i| i % 2 != 0),
            })
            .collect::<Vec<Info>>(),
        Err(err) => {
            tracing::error!("Error fetching id_bind LEFT JOIN player: {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    };

    // 第二次查询：player LEFT JOIN id_bind（只拿id_bind没有的数据）
    let extra_infos = match player_dsl::player
        .left_outer_join(id_bind_dsl::id_bind.on(player_dsl::id.eq(id_bind_dsl::id)))
        .filter(id_bind_dsl::id.is_null())
        .select((
            id_bind_dsl::mac.nullable(),
            player_dsl::id,
            id_bind_dsl::ip.nullable(),
            id_bind_dsl::client_version.nullable(),
            id_bind_dsl::last_seen.nullable(),
            player_dsl::username.nullable(),
            player_dsl::password.nullable(),
            player_dsl::synced.nullable(),
        ))
        .load::<(
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i32>,
        )>(&mut connection)
    {
        Ok(result) => result
            .into_iter()
            .map(|x| Info {
                mac: x.0,
                id: x.1,
                ip: x.2,
                client_version: x.3,
                last_seen: x.4,
                username: x.5,
                password: x.6,
                synced: x.7.map(|i| i % 2 != 0),
            })
            .collect::<Vec<Info>>(),
        Err(err) => {
            tracing::error!("Error fetching player LEFT JOIN id_bind: {}", err);
            return HttpResponse::InternalServerError().finish();
        }
    };

    infos.extend(extra_infos);
    response_body.infos = infos;
    HttpResponse::Ok().json(response_body)
}
