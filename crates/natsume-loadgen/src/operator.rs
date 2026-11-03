use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use clap::Args;
use reqwest::{
    Client, Method,
    header::{CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, SET_COOKIE},
};
use serde_json::{Value, json};
use snafu::ResultExt as _;
use tokio::{task::JoinSet, time::Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    cli::Run,
    connection::Settings,
    error::{self, Error, Result},
    metrics::Metrics,
    scenario::Action,
    setup::XLSX_TYPE,
};

#[derive(Args)]
pub(crate) struct Credentials {
    #[arg(long)]
    pub(crate) operator_login: Option<String>,
    /// UTF-8 password file; one trailing newline is removed. Never passed in argv or run.json.
    #[arg(long)]
    pub(crate) operator_password_file: Option<PathBuf>,
}

pub(crate) struct Operator {
    client: Client,
    origin: String,
    login: String,
    password: Zeroizing<String>,
    cookie: Option<HeaderValue>,
    recorder: Option<(Metrics, usize)>,
}
struct Response {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
    received: Instant,
}

impl Operator {
    pub(crate) fn new(
        origin: &str,
        trust: &Path,
        credentials: &Credentials,
        recorder: Option<(Metrics, usize)>,
    ) -> Result<Self> {
        let login = credentials.operator_login.clone().ok_or(Error::Invalid {
            code: "OPERATOR_LOGIN_REQUIRED",
        })?;
        let path = credentials
            .operator_password_file
            .as_ref()
            .ok_or(Error::Invalid {
                code: "OPERATOR_PASSWORD_FILE_REQUIRED",
            })?;
        let mut password = Zeroizing::new(std::fs::read_to_string(path).context(error::IoSnafu)?);
        if password.ends_with('\n') {
            password.pop();
            if password.ends_with('\r') {
                password.pop();
            }
        }
        if login.is_empty() || password.is_empty() {
            return Err(Error::Invalid {
                code: "OPERATOR_CREDENTIALS_EMPTY",
            });
        }
        Ok(Self {
            client: Settings::from_origin(origin, trust)?.http()?,
            origin: origin.to_owned(),
            login,
            password,
            cookie: None,
            recorder,
        })
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        kind: &'static str,
        body: Option<(Vec<u8>, &'static str)>,
        token: Option<&str>,
    ) -> Result<Response> {
        let started = Instant::now();
        let result = async {
            let mut request = self
                .client
                .request(method, format!("{}/api/v2{path}", self.origin));
            if let Some(cookie) = &self.cookie {
                request = request.header(COOKIE, cookie);
            }
            if let Some((bytes, content_type)) = body {
                request = request.header(CONTENT_TYPE, content_type).body(bytes);
            }
            if let Some(token) = token {
                request = request.header("x-natsume-preview-token", token);
            }
            let mut response = request.send().await.context(error::HttpSnafu)?;
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let mut body = Vec::new();
            while let Some(bytes) = response.chunk().await.context(error::HttpSnafu)? {
                if body.len() + bytes.len() > 64 * 1024 * 1024 {
                    return Err(Error::Peer {
                        code: "OPERATOR_RESPONSE_TOO_LARGE",
                    });
                }
                body.extend(bytes);
            }
            Ok::<_, Error>(Response {
                status,
                headers,
                body,
                received: Instant::now(),
            })
        }
        .await;
        if let Some((metrics, index)) = &self.recorder {
            metrics.record(
                *index,
                kind,
                started.elapsed(),
                result
                    .as_ref()
                    .map_or_else(|e| e.code().to_owned(), |r| r.status.to_string()),
            )?;
        }
        result
    }

    pub(crate) async fn login(&mut self) -> Result<()> {
        let body = serde_json::to_vec(
            &json!({"login_name": self.login, "password": self.password.as_str()}),
        )
        .context(error::JsonSnafu)?;
        let response = self
            .request(
                Method::POST,
                "/session",
                "operator_login",
                Some((body, "application/json")),
                None,
            )
            .await?;
        check(&response, "session")?;
        let cookie = response
            .headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| {
                v.split(';')
                    .next()
                    .filter(|v| v.starts_with("__Secure-natsume_session="))
            })
            .ok_or(Error::Peer {
                code: "SESSION_COOKIE_MISSING",
            })?;
        let mut cookie = HeaderValue::from_str(cookie).context(error::HeaderSnafu)?;
        cookie.set_sensitive(true);
        self.cookie = Some(cookie);
        Ok(())
    }

    async fn get(&mut self, path: &str, kind: &'static str) -> Result<Response> {
        let mut response = self.request(Method::GET, path, kind, None, None).await?;
        if response.status == 401 {
            self.login().await?;
            response = self.request(Method::GET, path, kind, None, None).await?;
        }
        check(&response, kind)?;
        Ok(response)
    }

    pub(crate) async fn probe_devices(&mut self) -> Result<()> {
        self.get("/devices", "operator_devices").await.map(|_| ())
    }

    pub(crate) async fn export(&mut self) -> Result<()> {
        self.get("/exports/domjudge", "operator_export")
            .await
            .map(|_| ())
    }

    pub(crate) async fn import(&mut self, workbook: Vec<u8>) -> Result<()> {
        // HEAD's extractor caps bodies at 2 MiB; v2.10.3 allows 8 MiB. Use the common budget.
        if workbook.len() > 2 * 1024 * 1024 {
            return Err(Error::Invalid {
                code: "ROSTER_EXCEEDS_COMMON_HTTP_LIMIT",
            });
        }
        let preview = self
            .request(
                Method::POST,
                "/imports",
                "operator_import_preview",
                Some((workbook.clone(), XLSX_TYPE)),
                None,
            )
            .await?;
        check(&preview, "imports")?;
        let preview: Value = serde_json::from_slice(&preview.body).context(error::JsonSnafu)?;
        let id = preview["candidate_id"].as_str().ok_or(Error::Peer {
            code: "IMPORT_CANDIDATE",
        })?;
        let token = preview["preview_token"].as_str().ok_or(Error::Peer {
            code: "IMPORT_TOKEN",
        })?;
        let commit = self
            .request(
                Method::POST,
                &format!("/imports/{id}/actions/commit"),
                "operator_import_commit",
                Some((workbook, XLSX_TYPE)),
                Some(token),
            )
            .await?;
        check(&commit, "import_commit")
    }

    pub(crate) async fn open_window(&self) -> Result<()> {
        check(
            &self
                .request(
                    Method::PUT,
                    "/provisioning-window",
                    "operator_provisioning",
                    Some((br#"{"state":"open"}"#.to_vec(), "application/json")),
                    None,
                )
                .await?,
            "provisioning-window",
        )
    }
    pub(crate) async fn organizations(&mut self) -> Result<Vec<Value>> {
        serde_json::from_slice(
            &self
                .get("/organizations", "operator_organizations")
                .await?
                .body,
        )
        .context(error::JsonSnafu)
    }
    pub(crate) async fn check_logo(&mut self, id: &str) -> Result<()> {
        let response = self
            .get(&format!("/organizations/{id}/logo"), "operator_logo")
            .await?;
        if response.status != 200
            || response
                .headers
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                != Some("image/webp")
        {
            return Err(Error::Peer {
                code: "SETUP_WEBP_UNAVAILABLE",
            });
        }
        Ok(())
    }

    async fn submit(&mut self, action: Action, metrics: &Metrics) -> Result<()> {
        // No filter syntax: v2.10.3 accepts comma-separated states; HEAD also has non_revoked.
        let devices: Vec<Value> =
            serde_json::from_slice(&self.get("/devices", "operator_devices").await?.body)
                .context(error::JsonSnafu)?;
        let id = Uuid::now_v7().to_string();
        let action_body = match action {
            Action::Contest => json!({"kind":"set_foreground", "foreground_target":"contest"}),
            Action::Waiting => json!({"kind":"set_foreground", "foreground_target":"waiting"}),
            Action::Terminate => json!({"kind":"terminate_session"}),
            Action::ResetHome => json!({"kind":"reset_home"}),
            Action::Export | Action::Import => {
                return Err(Error::Invalid {
                    code: "TARGET_ACTION",
                });
            }
        };
        let body = serde_json::to_vec(
            &json!({"operation_id":id, "scope":{"kind":"all_enabled"}, "action":action_body}),
        )
        .context(error::JsonSnafu)?;
        metrics.convergence.begin(id.clone(), action, &devices)?;
        let response = self
            .request(
                Method::POST,
                "/target-submissions",
                "operator_target_submission",
                Some((body, "application/json")),
                None,
            )
            .await?;
        check(&response, "target-submissions")?;
        let receipt: Value = serde_json::from_slice(&response.body).context(error::JsonSnafu)?;
        metrics
            .convergence
            .finish(&id, response.received, &receipt, metrics)?;
        let results = receipt["results"].as_array().ok_or(Error::Peer {
            code: "SUBMISSION_RECEIPT",
        })?;
        let submitted = results
            .iter()
            .filter(|row| row["status"] == "submitted")
            .count();
        tracing::info!(
            operation_id = id,
            action = action.name(),
            submitted,
            rejected = results.len() - submitted,
            "operator target response"
        );
        metrics.record(
            0,
            "operator_submission_receipt",
            Duration::ZERO,
            format!(
                "{id}:{}:{submitted}:{}",
                action.name(),
                results.len() - submitted
            ),
        )?;
        if submitted != results.len() {
            return Err(Error::Peer {
                code: "TARGET_SUBMISSION_REJECTED",
            });
        }
        Ok(())
    }
}

fn check(response: &Response, route: &'static str) -> Result<()> {
    if (200..300).contains(&response.status) {
        Ok(())
    } else {
        Err(Error::Status {
            route,
            status: response.status,
        })
    }
}

const PANEL_ROUTES: [(&str, &str); 8] = [
    ("/devices", "operator_devices"),
    ("/bindings", "operator_bindings"),
    ("/seats", "operator_seats"),
    ("/accounts", "operator_accounts"),
    ("/organizations", "operator_organizations"),
    ("/provisioning-window", "operator_provisioning"),
    ("/enrollment-reviews", "operator_enrollment"),
    ("/session", "operator_session"),
];

pub(crate) fn validate_rates(
    rates: Option<&std::collections::BTreeMap<String, f64>>,
) -> Result<()> {
    if rates.is_some_and(|rates| {
        rates.iter().any(|(path, rate)| {
            !PANEL_ROUTES.iter().any(|(route, _)| *route == path)
                || !rate.is_finite()
                || *rate < 0.0
                || (*rate > 0.0 && !(0.0001..=60_000.0).contains(rate))
        })
    }) {
        return Err(Error::Invalid {
            code: "PANEL_RATES",
        });
    }
    Ok(())
}

fn panel_schedule(
    run: &Run,
    index: usize,
    started: Instant,
) -> Vec<(&'static str, &'static str, Instant, Duration)> {
    PANEL_ROUTES
        .into_iter()
        .filter_map(|(path, kind)| {
            let period = if let Some(rates) = &run.panel_rates {
                let rate = rates.get(path).copied().unwrap_or_default();
                if rate == 0.0 {
                    return None;
                }
                Duration::from_secs_f64(
                    60.0 * f64::from(u32::try_from(run.operators).unwrap_or(u32::MAX)) / rate,
                )
            } else if path == "/session" {
                run.session_interval
            } else {
                run.poll_interval
            };
            let phase = if run.panel_rates.is_some() {
                period.mul_f64(
                    f64::from(u32::try_from(index).unwrap_or(u32::MAX))
                        / f64::from(u32::try_from(run.operators.max(1)).unwrap_or(u32::MAX)),
                )
            } else {
                Duration::ZERO
            };
            Some((path, kind, started + phase, period))
        })
        .collect()
}

pub(crate) fn spawn(
    tasks: &mut JoinSet<Result<()>>,
    run: &Arc<Run>,
    metrics: &Metrics,
    stop: &CancellationToken,
    started: Instant,
) -> Result<()> {
    for index in 0..run.operators {
        let mut operator = Operator::new(
            &run.server,
            &run.trust_root,
            &run.operator,
            Some((metrics.clone(), index)),
        )?;
        let run = Arc::clone(run);
        let stop = stop.clone();
        tasks.spawn(async move {
            tokio::select! { () = stop.cancelled() => return Ok(()), result = operator.login() => result? }
            let mut schedule = panel_schedule(&run, index, started);
            loop {
                let Some((next, _)) = schedule.iter().enumerate().min_by_key(|(_, row)| row.2) else {
                    stop.cancelled().await;
                    return Ok(());
                };
                let (path, kind, deadline, period) = &mut schedule[next];
                tokio::select! {
                    () = stop.cancelled() => return Ok(()),
                    () = tokio::time::sleep_until(*deadline) => {}
                }
                tokio::select! {
                    () = stop.cancelled() => return Ok(()),
                    result = operator.get(path, kind) => {
                        if let Err(error) = result {
                            tracing::warn!(tab = index, code = error.code(), "operator poll failed");
                        }
                    }
                }
                *deadline += *period;
                // Skip missed periods after an outage; keep the calibrated phase.
                while *deadline < Instant::now() { *deadline += *period; }
            }
        });
    }
    if !run.events.is_empty() {
        let mut operator = Operator::new(
            &run.server,
            &run.trust_root,
            &run.operator,
            Some((metrics.clone(), 0)),
        )?;
        let run = Arc::clone(run);
        let metrics = metrics.clone();
        let stop = stop.clone();
        tasks.spawn(async move {
            tokio::select! { () = stop.cancelled() => return Ok(()), result = operator.login() => result? }
            for event in &run.events {
                tokio::select! { () = stop.cancelled() => return Ok(()), () = tokio::time::sleep_until(started + event.at) => {} }
                let action = async {
                    if event.action.is_target() { operator.submit(event.action, &metrics).await }
                    else if event.action == Action::Export { operator.get("/exports/domjudge", "operator_export").await.map(|_| ()) }
                    else {
                        let path = event.roster.as_ref().or(run.operator_roster.as_ref()).ok_or(Error::Invalid { code: "IMPORT_ROSTER_REQUIRED" })?;
                        operator.import(tokio::fs::read(path).await.context(error::IoSnafu)?).await
                    }
                };
                tokio::select! { () = stop.cancelled() => return Ok(()), result = action => result? }
            }
            // Keep the schedule task alive until the fleet's duration expires.
            stop.cancelled().await;
            Ok(())
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calibrated_tabs_preserve_rates_and_phase() -> Result<()> {
        let mut run = crate::cli::tests::run();
        run.operators = 2;
        run.panel_rates = Some(
            [
                ("/devices".to_owned(), 10.616_666_666_666_667),
                ("/session".to_owned(), 2.088_888_888_888_889),
                ("/accounts".to_owned(), 0.0),
            ]
            .into(),
        );
        validate_rates(run.panel_rates.as_ref())?;
        let start = Instant::now();
        let first = panel_schedule(&run, 0, start);
        let second = panel_schedule(&run, 1, start);
        assert_eq!(first.len(), 2);
        assert!((first[0].3.as_secs_f64() - 11.302_982_731_554_16).abs() < 1e-6);
        assert_eq!(first[0].2, start);
        assert!(
            (second[0].2.duration_since(start).as_secs_f64() - first[0].3.as_secs_f64() / 2.0)
                .abs()
                < 1e-6
        );
        run.panel_rates = Some([("/secret".to_owned(), 1.0)].into());
        assert!(validate_rates(run.panel_rates.as_ref()).is_err());
        run.panel_rates = Some([("/devices".to_owned(), f64::NAN)].into());
        assert!(validate_rates(run.panel_rates.as_ref()).is_err());
        Ok(())
    }
}
