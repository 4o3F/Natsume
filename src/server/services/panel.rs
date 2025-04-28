use actix_web::{HttpRequest, HttpResponse, Responder};
use mime_guess::from_path;
use rust_embed::Embed;

#[derive(Embed)]
#[allow_missing = true]
#[folder = "panel/dist/"]
pub struct Asset;

pub async fn spa_handler(req: HttpRequest) -> impl Responder {
    let full_path = req.path();

    let path = if let Some(stripped) = full_path.strip_prefix("/panel/") {
        stripped
    } else if full_path == "/panel" {
        ""
    } else {
        return HttpResponse::NotFound().finish();
    };

    let file = if path.is_empty() {
        "index.html"
    } else if Asset::get(path).is_some() {
        path
    } else {
        "index.html"
    };

    if let Some(content) = Asset::get(file) {
        let body = actix_web::body::BoxBody::new(content.data.into_owned());
        let mime = from_path(file).first_or_octet_stream();
        HttpResponse::Ok().content_type(mime.to_string()).body(body)
    } else {
        HttpResponse::NotFound().finish()
    }
}
