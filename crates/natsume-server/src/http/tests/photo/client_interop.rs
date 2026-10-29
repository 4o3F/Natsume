//! Opt-in WSL Server / native Windows Client interoperability test.
use super::{Fixture, PendingEnrollment, TestResult};
use crate::test_error::TestError;
use axum::serve::Listener as _;
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    time::Duration,
};

async fn windows_path(path: &Path) -> Result<String, TestError> {
    let output = tokio::process::Command::new("wslpath")
        .arg("-w")
        .arg(path)
        .output()
        .await?;
    assert!(output.status.success());
    Ok(std::str::from_utf8(&output.stdout)?.trim().to_owned())
}

#[tokio::test]
#[ignore = "requires WSL and NATSUME_PHOTO_INTEROP_DRIVER pointing to the native Windows initialization test executable"]
async fn photo_client_initializes_against_real_server_tls_and_then_runs_offline() -> TestResult {
    let driver = std::env::var_os("NATSUME_PHOTO_INTEROP_DRIVER")
        .ok_or_else(|| TestError::missing("native Windows initialization test executable"))?;
    let f = Fixture::new().await?;
    let source = f
        .state
        .photo()
        .create_source("Real Windows client".into())
        .await?
        .source_id;
    let code = f
        .state
        .photo()
        .issue_code(source.to_string(), PendingEnrollment::Preserve)
        .await?;
    let identity = crate::tls::tests::TestIdentity::new(Ipv4Addr::LOCALHOST)?;
    let listener = crate::tls::TlsListener::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity.certificate_path(),
        identity.private_key_path(),
    )
    .await?;
    let origin = format!("https://{}", listener.local_addr()?);
    let application = f.app.clone();
    let _running = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(
            listener,
            application.into_make_service_with_connect_info::<crate::tls::PeerAddress>(),
        )
        .await
    }));
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let directory = tempfile::tempdir_in(target)?;
    let ca = directory.path().join("ca.crt");
    std::fs::copy(identity.directory_path().join("local-origin-ca.crt"), &ca)?;
    let input = directory.path().join("input.json");
    let config = serde_json::json!({
        "origin": origin, "code": code.code,
        "ca": windows_path(&ca).await?,
        "workspace": windows_path(&directory.path().join("workspace")).await?,
    });
    std::fs::write(&input, serde_json::to_vec(&config)?)?;
    let inherited = std::env::var("WSLENV").unwrap_or_default();
    let mut child = tokio::process::Command::new(driver)
        .args(["--ignored", "--exact", "windows_client_against_live_server"])
        .env("NATSUME_PHOTO_INTEROP_INPUT", windows_path(&input).await?)
        .env("WSLENV", format!("{inherited}:NATSUME_PHOTO_INTEROP_INPUT"))
        .kill_on_drop(true)
        .spawn()?;
    let status = tokio::time::timeout(Duration::from_mins(1), child.wait()).await??;
    assert!(
        status.success(),
        "native Windows initialization test failed"
    );
    assert!(
        f.state
            .photo()
            .list_sources()
            .await?
            .into_iter()
            .any(|s| s.source_id == source && s.initialization_completed)
    );
    Ok(())
}
