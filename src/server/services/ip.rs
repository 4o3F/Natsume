use actix_web::{get, HttpRequest, HttpResponse, Responder};
use serde::Serialize;

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