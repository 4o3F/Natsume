use axum::{
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use cookie::{Cookie, SameSite, time::Duration};
use zeroize::Zeroizing;

use crate::component::operator::SessionCredentialHex;

const SESSION_COOKIE_NAME: &str = "__Secure-natsume_session";
const SESSION_COOKIE_PATH: &str = "/api/v2";
const SESSION_COOKIE_MAX_AGE_SECONDS: i64 = 57_600;

pub(super) fn issue_session_credential(wire_credential: &str) -> Result<HeaderValue, ()> {
    let value =
        Zeroizing::new(session_cookie(wire_credential, SESSION_COOKIE_MAX_AGE_SECONDS).to_string());
    HeaderValue::from_str(&value).map_err(|_| ())
}

pub(super) fn session_credential(headers: &HeaderMap) -> Result<SessionCredentialHex, ()> {
    let mut target = None;
    for header_value in headers.get_all(header::COOKIE) {
        let value = header_value.to_str().map_err(|_| ())?;
        for parsed in Cookie::split_parse(value).filter_map(Result::ok) {
            if parsed.name() == SESSION_COOKIE_NAME {
                if target.is_some() {
                    return Err(());
                }
                target = Some(SessionCredentialHex::new(parsed.value().to_owned()));
            }
        }
    }
    target.ok_or(())
}

pub(super) fn with_clearing_session_cookie(mut response: Response) -> Result<Response, ()> {
    let clearing = session_cookie("", 0).to_string();
    let value = HeaderValue::from_str(&clearing).map_err(|_| ())?;
    response.headers_mut().insert(header::SET_COOKIE, value);
    Ok(response)
}

fn session_cookie(value: &str, max_age_seconds: i64) -> Cookie<'_> {
    Cookie::build((SESSION_COOKIE_NAME, value))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(true)
        .path(SESSION_COOKIE_PATH)
        .max_age(Duration::seconds(max_age_seconds))
        .build()
}
