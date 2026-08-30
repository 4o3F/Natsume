use std::{future::Future, io, sync::Arc, time::Duration};

use axum::{Extension, Router, extract::ConnectInfo, serve::Listener as _};
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    server::graceful::GracefulShutdown,
    service::TowerToHyperService,
};
use tokio::sync::{Semaphore, watch};
use tower::Layer as _;

use crate::tls::TlsListener;

pub(crate) const HTTP_MAX_HEADER_COUNT: usize = 64;
pub(crate) const HTTP_HEADER_READ_TIMEOUT_SECONDS: u64 = 10;
pub(crate) const HTTP_MAX_BUF_SIZE_BYTES: usize = 65_536;
pub(crate) const MAX_CONCURRENT_CONNECTIONS: usize = 2_048;

pub(crate) async fn serve_until<F>(
    mut listener: TlsListener,
    router: Router,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let capacity = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    let graceful = GracefulShutdown::new();
    let (connection_shutdown, _) = watch::channel(false);
    tokio::pin!(shutdown);

    loop {
        if capacity.available_permits() == 0 {
            tracing::warn!("connection capacity saturated; accepting is paused");
        }
        let permit = tokio::select! {
            biased;
            () = &mut shutdown => break,
            permit = Arc::clone(&capacity).acquire_owned() => {
                match permit {
                    Ok(permit) => permit,
                    Err(_) => return Err(io::Error::other("connection capacity semaphore closed")),
                }
            }
        };

        let (stream, remote_address) = tokio::select! {
            biased;
            () = &mut shutdown => {
                drop(permit);
                break;
            }
            accepted = listener.accept() => accepted,
        };

        // `ClientAddress::from_request_parts` reads `ConnectInfo<SocketAddr>` when the
        // typed extension is absent, so handlers observe the same value the old
        // `into_make_service_with_connect_info` path produced. `AddExtension` wraps the
        // router without rebuilding its route table per connection.
        let service = Extension(ConnectInfo(remote_address)).layer(router.clone());
        let hyper_service = TowerToHyperService::new(service);
        let mut builder = http1::Builder::new();
        builder
            .timer(TokioTimer::new())
            .max_headers(HTTP_MAX_HEADER_COUNT)
            .header_read_timeout(Duration::from_secs(HTTP_HEADER_READ_TIMEOUT_SECONDS))
            .max_buf_size(HTTP_MAX_BUF_SIZE_BYTES);
        let connection = builder
            .serve_connection(TokioIo::new(stream), hyper_service)
            .with_upgrades();
        let mut connection_shutdown = connection_shutdown.subscribe();

        // hyper-util 0.1.20 cannot wrap Hyper's HTTP/1 UpgradeableConnection
        // directly. Holding a watcher registers this task with GracefulShutdown;
        // the explicit signal below starts Hyper's own graceful drain.
        let graceful_watcher = graceful.watcher();
        tokio::spawn(async move {
            let _permit = permit;
            let _graceful_watcher = graceful_watcher;
            let mut connection = std::pin::pin!(connection);

            tokio::select! {
                result = connection.as_mut() => log_connection_error(&result),
                _ = connection_shutdown.changed() => {
                    connection.as_mut().graceful_shutdown();
                    log_connection_error(&connection.await);
                }
            }
        });
    }

    // Releasing the listener before draining matches axum::serve: new connections are
    // refused immediately instead of sitting unaccepted in the kernel backlog.
    drop(listener);
    let _send_result = connection_shutdown.send(true);
    graceful.shutdown().await;
    Ok(())
}

fn log_connection_error(result: &Result<(), hyper::Error>) {
    if result.is_err() {
        tracing::debug!("HTTP connection ended with a redacted protocol or transport error");
    }
}
