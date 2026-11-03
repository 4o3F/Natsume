use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use reqwest::{
    Client, StatusCode,
    header::{ETAG, IF_NONE_MATCH},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use snafu::ResultExt as _;
use tokio::{sync::watch, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::Run,
    error::{self, Error, Result},
    identity::atomic_write,
    metrics::Metrics,
};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cache {
    scope: String,
    etags: BTreeMap<String, String>,
}

impl Cache {
    fn load(path: &std::path::Path, scope: String) -> Result<Self> {
        let cache = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<Self>(&bytes).context(error::JsonSnafu)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(source) => return Err(Error::Io { source }),
        };
        Ok(if cache.scope == scope {
            cache
        } else {
            Self {
                scope,
                ..Self::default()
            }
        })
    }
    fn save(&self, path: &std::path::Path) -> Result<()> {
        atomic_write(
            path,
            path.parent().ok_or(Error::Invalid {
                code: "LOGO_CACHE_PATH",
            })?,
            &serde_json::to_vec(self).context(error::JsonSnafu)?,
        )
    }
}

pub(crate) async fn run(
    client: Client,
    run: Arc<Run>,
    device: usize,
    metrics: Metrics,
    mut organization: watch::Receiver<Option<String>>,
    stop: CancellationToken,
) -> Result<()> {
    let path: PathBuf = run.state_dir.join(format!("logos-{device:05}.json"));
    let ca = std::fs::read(&run.trust_root).context(error::IoSnafu)?;
    let scope = format!("{}:{}", run.server, hex::encode(Sha256::digest(ca)));
    let mut cache = Cache::load(&path, scope)?;
    loop {
        // None pauses downloads on disconnect or unbinding. Organization entries remain durable.
        let school = organization.borrow_and_update().clone();
        if let Some(school) = school {
            let mut etag = cache.etags.get(&school).cloned();
            let previous = etag.clone();
            let started = Instant::now();
            let result = tokio::select! {
                biased;
                () = stop.cancelled() => return Ok(()),
                changed = organization.changed() => { if changed.is_err() { return Ok(()); } continue; }
                result = fetch(&client, &run.server, &school, &mut etag) => result,
            };
            let code = match result {
                Ok(status) => status.to_string(),
                Err(error) => error.code().to_owned(),
            };
            if previous != etag {
                match etag {
                    Some(etag) => {
                        cache.etags.insert(school, etag);
                    }
                    None => {
                        cache.etags.remove(&school);
                    }
                }
                cache.save(&path)?;
            }
            metrics.record(device, "logo", started.elapsed(), code)?;
        }
        tokio::select! {
            () = stop.cancelled() => return Ok(()),
            changed = organization.changed() => { if changed.is_err() { return Ok(()); } }
            () = tokio::time::sleep(run.logo_interval) => {}
        }
    }
}

async fn fetch(
    client: &Client,
    origin: &str,
    school: &str,
    etag: &mut Option<String>,
) -> Result<u16> {
    let mut request = client.get(format!("{origin}/api/v2/organizations/{school}/logo"));
    if let Some(etag) = etag.as_ref() {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let mut response = request.send().await.context(error::HttpSnafu)?;
    let status = response.status();
    match status {
        StatusCode::NOT_MODIFIED if etag.is_some() => return Ok(status.as_u16()),
        StatusCode::NOT_FOUND => {
            *etag = None;
            return Ok(status.as_u16());
        }
        StatusCode::OK => {}
        _ => return Ok(status.as_u16()),
    }
    let candidate = response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 256)
        .map(str::to_owned);
    let mut length = 0;
    while let Some(chunk) = response.chunk().await.context(error::HttpSnafu)? {
        length += chunk.len();
        if length > 20 * 1024 * 1024 {
            return Err(Error::Peer {
                code: "LOGO_TOO_LARGE",
            });
        }
    }
    *etag = candidate;
    Ok(status.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::atomic::Ordering, time::Duration};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    async fn respond(listener: tokio::net::TcpListener) -> Result<()> {
        for (school, status, conditional) in [
            ("INST-001", "200 OK", false),
            ("INST-002", "200 OK", false),
            ("INST-001", "304 Not Modified", true),
            ("INST-001", "304 Not Modified", true),
        ] {
            let (mut socket, _) = listener.accept().await.context(error::IoSnafu)?;
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.context(error::IoSnafu)?);
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.contains(&format!("/organizations/{school}/logo")));
            assert_eq!(request.contains("if-none-match: \"logo-v1\""), conditional);
            socket.write_all(format!("HTTP/1.1 {status}\r\nETag: \"logo-v1\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.context(error::IoSnafu)?;
        }
        Ok(())
    }

    async fn polls(metrics: &Metrics, count: u64) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            while metrics.events.load(Ordering::Relaxed) < count {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .context(error::TimeoutSnafu)
    }

    #[tokio::test]
    async fn disconnect_pauses_polling_and_organization_etags_survive_reconnect_and_reload()
    -> Result<()> {
        let directory = tempfile::tempdir().context(error::IoSnafu)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context(error::IoSnafu)?;
        let origin = format!("http://{}", listener.local_addr().context(error::IoSnafu)?);
        let server = tokio::spawn(respond(listener));
        let client = Client::builder()
            .no_proxy()
            .build()
            .context(error::HttpSnafu)?;
        let mut config = crate::cli::tests::run();
        config.server = origin;
        config.state_dir = directory.path().to_owned();
        config.trust_root = directory.path().join("root");
        std::fs::write(&config.trust_root, b"synthetic trust scope").context(error::IoSnafu)?;
        // Short interval makes an accidental poll while offline observable.
        config.logo_interval = Duration::from_millis(10);
        let config = Arc::new(config);
        let (metrics, writer) = Metrics::start(&directory.path().join("events"))?;
        let (selection, receiver) = watch::channel(Some("INST-001".to_owned()));
        let stop = CancellationToken::new();
        let task = tokio::spawn(run(
            client.clone(),
            Arc::clone(&config),
            0,
            metrics.clone(),
            receiver,
            stop.clone(),
        ));
        polls(&metrics, 1).await?;
        selection.send_replace(None);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            metrics.events.load(Ordering::Relaxed),
            1,
            "offline downloads must pause"
        );
        selection.send_replace(Some("INST-002".to_owned()));
        polls(&metrics, 2).await?;
        selection.send_replace(Some("INST-001".to_owned()));
        polls(&metrics, 3).await?;
        stop.cancel();
        task.await.context(error::JoinSnafu)??;
        let restarted = CancellationToken::new();
        let receiver = selection.subscribe();
        let task = tokio::spawn(run(
            client,
            config,
            0,
            metrics.clone(),
            receiver,
            restarted.clone(),
        ));
        polls(&metrics, 4).await?;
        restarted.cancel();
        task.await.context(error::JoinSnafu)??;
        server.await.context(error::JoinSnafu)??;
        let path = directory.path().join("logos-00000.json");
        assert!(
            Cache::load(&path, "another-server".to_owned())?
                .etags
                .is_empty()
        );
        drop(metrics);
        writer.await.context(error::JoinSnafu)??;
        Ok(())
    }
}
