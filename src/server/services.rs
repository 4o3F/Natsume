use actix_web::{HttpRequest, HttpResponse, Responder, get, post, web::Json};
use serde::{Deserialize, Serialize};
use tracing_unwrap::OptionExt;

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

#[derive(Deserialize)]
struct BindRequestBody {
    mac: String,
    id: String,
}
#[post("/bind")]
pub async fn bind(body: Json<BindRequestBody>) -> impl Responder {
    tracing::info!("MAC {} ID {}", body.mac, body.id);
    
    HttpResponse::Ok()
}
