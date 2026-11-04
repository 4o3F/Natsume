//! Opt-in real Windows-to-WSL HTTPS download with one named photo per roster team.
use super::*;
use crate::component::contest::PhotoTeam;
use axum::serve::Listener as _;
use natsume_photo_protocol::{
    FormatVersion, PhotoInitialization,
    results::{ManifestKind, ResultContext, ResultsManifest, TeamResult},
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::io::AsyncWriteExt as _;

#[derive(serde::Deserialize)]
struct Inputs {
    image: PathBuf,
    logos: PathBuf,
    output: PathBuf,
    teams: Vec<PhotoTeam>,
}

async fn seed(f: &Fixture, teams: Vec<PhotoTeam>) -> TestResult {
    f.database
        .database
        .write(move |tx| -> Result<(), PersistenceError> {
            diesel::delete(accounts::table).execute(tx.connection())?;
            diesel::delete(seats::table).execute(tx.connection())?;
            diesel::delete(organizations::table).execute(tx.connection())?;
            let mut schools = std::collections::BTreeSet::new();
            for team in teams {
                if schools.insert(team.organization_id) {
                    diesel::insert_into(organizations::table)
                        .values((
                            organizations::organization_id.eq(team.organization_id),
                            organizations::name_key
                                .eq(format!("benchmark-{}", team.organization_id)),
                            organizations::name_zh.eq(&team.school_name_zh),
                            organizations::name_en.eq(&team.school_name_en),
                            organizations::country.eq("CHN"),
                        ))
                        .execute(tx.connection())?;
                }
                diesel::insert_into(accounts::table)
                    .values((
                        accounts::account_id.eq(&team.account_id),
                        accounts::domjudge_username.eq(&team.account),
                        accounts::credential_revision.eq(1_i64),
                    ))
                    .execute(tx.connection())?;
                diesel::insert_into(teams::table)
                    .values((
                        teams::account_id.eq(&team.account_id),
                        teams::organization_id.eq(team.organization_id),
                        teams::name_zh.eq(&team.team_name_zh),
                        teams::name_en.eq(&team.team_name_en),
                        teams::category.eq("official"),
                    ))
                    .execute(tx.connection())?;
                diesel::insert_into(seats::table)
                    .values((
                        seats::seat_id.eq(&team.account_id),
                        seats::seat_code.eq(&team.account),
                    ))
                    .execute(tx.connection())?;
                diesel::insert_into(account_mappings::table)
                    .values((
                        account_mappings::account_id.eq(&team.account_id),
                        account_mappings::seat_id.eq(&team.account_id),
                    ))
                    .execute(tx.connection())?;
            }
            Ok(())
        })
        .await?;
    Ok(())
}

async fn windows_path(path: &Path) -> Result<String, TestError> {
    let output = tokio::process::Command::new("wslpath")
        .arg("-w")
        .arg(path)
        .output()
        .await?;
    assert!(output.status.success());
    Ok(std::str::from_utf8(&output.stdout)?.trim().to_owned())
}

async fn download(
    origin: &str,
    cookie: &str,
    ca: &Path,
    output: &Path,
) -> Result<Value, TestError> {
    let mut child = tokio::process::Command::new("/mnt/c/Windows/System32/curl.exe")
        .args([
            "--silent",
            "--show-error",
            "--ssl-revoke-best-effort",
            "--request",
            "POST",
            "--max-time",
            "900",
            "--config",
            "-",
        ])
        .arg("--cacert")
        .arg(windows_path(ca).await?)
        .arg("--output")
        .arg(windows_path(output).await?)
        .args(["--write-out", "%{json}"])
        .arg(format!("{origin}/api/v2/photo/exports/pyrite"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    // Send the session only over stdin, not the process command line or logs.
    let mut input = child.stdin.take().ok_or(TestError::missing("curl stdin"))?;
    input
        .write_all(format!("header = \"Cookie: {cookie}\"\n").as_bytes())
        .await?;
    drop(input);
    let result = child.wait_with_output().await?;
    if !result.status.success() {
        std::fs::write(
            output.with_extension("transport.json"),
            serde_json::to_vec_pretty(&json!({
                "exit_code":result.status.code(), "stderr":String::from_utf8_lossy(&result.stderr),
                "stdout":String::from_utf8_lossy(&result.stdout)
            }))?,
        )?;
    }
    assert!(
        result.status.success(),
        "Windows HTTPS download failed: {:?}",
        result.status.code()
    );
    let metrics: Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(metrics["http_code"], 200);
    Ok(metrics)
}

async fn initialize(f: &Fixture) -> Result<(PhotoInitialization, Sha256Digest), TestError> {
    f.confirm().await?;
    let response = f
        .app
        .clone()
        .oneshot(
            Request::post("/api/v2/photo/client/initialization")
                .header(header::AUTHORIZATION, format!("Bearer {}", f.token))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024).await?;
    let snapshot: PhotoInitialization = serde_json::from_slice(&bytes)?;
    let snapshot_hash = Sha256Digest::of(&bytes);
    assert_eq!(
        f.send(
            Method::POST,
            "client/initialization/complete",
            json!({
                "initialization_id":snapshot.initialization_id, "manifest_sha256":snapshot_hash,
                "request_id":Uuid::now_v7()
            })
        )
        .await?
        .status,
        StatusCode::NO_CONTENT
    );
    Ok((snapshot, snapshot_hash))
}

fn verify_archive(inputs: &Inputs, jpeg: &[u8]) -> TestResult {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&inputs.output)?)?;
    let mut names = std::collections::BTreeSet::new();
    for name in zip.file_names() {
        let name = name?;
        if name.starts_with("teams/") && name.ends_with(".jpg") {
            names.insert(name.into_owned());
        }
    }
    assert_eq!(names.len(), inputs.teams.len());
    for team in &inputs.teams {
        let mut entry = zip.by_name(&format!("teams/{}.jpg", team.account))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        assert_eq!(bytes, team_jpeg(jpeg, &team.account)?);
    }
    Ok(())
}

fn team_jpeg(jpeg: &[u8], account: &str) -> Result<Vec<u8>, TestError> {
    // A JPEG comment preserves the exact pixels while giving each team its own object.
    let length = u16::try_from(account.len() + 2)?.to_be_bytes();
    let mut bytes = Vec::with_capacity(jpeg.len() + account.len() + 4);
    bytes.extend_from_slice(&[0xff, 0xd8, 0xff, 0xfe]);
    bytes.extend_from_slice(&length);
    bytes.extend_from_slice(account.as_bytes());
    bytes.extend_from_slice(&jpeg[2..]);
    Ok(bytes)
}

async fn separate_photos(f: &Fixture, inputs: &Inputs, jpeg: &[u8]) -> TestResult {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
    let directory = f
        .database
        .documents_path()
        .with_extension("photo-results")
        .join("objects");
    let mut updates = Vec::new();
    for team in &inputs.teams {
        let bytes = team_jpeg(jpeg, &team.account)?;
        let hash = Sha256Digest::of(&bytes);
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(directory.join(format!("{hash}.jpg")))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        updates.push((
            team.account_id.clone(),
            hash.to_string(),
            i64::try_from(bytes.len())?,
        ));
    }
    f.database
        .database
        .write(move |tx| -> Result<(), PersistenceError> {
            use crate::diesel_schema::photo_submissions as submissions;
            for (id, hash, bytes) in updates {
                diesel::update(submissions::table.filter(submissions::assigned_account_id.eq(id)))
                    .set((
                        submissions::image_sha256.eq(hash),
                        submissions::image_bytes.eq(bytes),
                    ))
                    .execute(tx.connection())?;
            }
            Ok(())
        })
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires WSL, native Windows curl and NATSUME_PHOTO_BENCHMARK_INPUT non-secret fixture JSON"]
async fn pyrite_download_each_team_over_real_windows_https() -> TestResult {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_test_writer()
        .try_init()
        .map_err(|_| TestError::missing("benchmark tracing subscriber"))?;
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let path = std::env::var_os("NATSUME_PHOTO_BENCHMARK_INPUT")
        .ok_or(TestError::missing("benchmark input JSON"))?;
    let inputs: Inputs = serde_json::from_slice(&std::fs::read(path)?)?;
    assert!(!inputs.teams.is_empty());
    let (mut f, cookie) = admin().await?;
    seed(&f, inputs.teams.clone()).await?;
    for entry in std::fs::read_dir(&inputs.logos)? {
        let entry = entry?;
        std::fs::copy(entry.path(), f.logos.path().join(entry.file_name()))?;
    }
    let (snapshot, snapshot_hash) = initialize(&f).await?;
    // Normalize the supplied screenshot once, like a confirmed client JPEG.
    // Separate physical objects are created below, so storage deduplication cannot skew delivery.
    let image = image::open(&inputs.image)?.into_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode_image(&image)?;
    let hash = Sha256Digest::of(&jpeg);
    let image_bytes = u64::try_from(jpeg.len())?;
    let mut manifest = ResultsManifest {
        format_version: FormatVersion::V1,
        kind: ManifestKind::TeamResults,
        manifest_id: ManifestId::generate(),
        signing_public_key: f.key.clone(),
        context: ResultContext {
            source_id: snapshot.source_id,
            dataset_id: snapshot.dataset_id,
            initialization_id: snapshot.initialization_id,
            initialization_sha256: snapshot_hash,
        },
        results: inputs
            .teams
            .iter()
            .map(|team| TeamResult {
                account_id: team.account_id.clone(),
                account: team.account.clone(),
                result_id: ResultId::generate(),
                submission_id: SubmissionId::generate(),
                image_format: PhotoFormat::Jpeg,
                image_sha256: hash.clone(),
                image_bytes,
                width: image.width(),
                height: image.height(),
            })
            .collect(),
    };
    manifest
        .results
        .sort_by(|a, b| a.account_id.cmp(&b.account_id));
    let signed = manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &jpeg).await?;
    let response = f
        .app
        .clone()
        .oneshot(
            Request::post(format!("/api/v2/photo/imports/{id}/commit"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024).await?;
    separate_photos(&f, &inputs, &jpeg).await?;
    let identity = crate::tls::tests::TestIdentity::new(Ipv4Addr::LOCALHOST)?;
    let listener = crate::tls::TlsListener::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity.certificate_path(),
        identity.private_key_path(),
    )
    .await?;
    let origin = format!("https://{}", listener.local_addr()?);
    let ca = inputs.output.with_extension("ca.crt");
    std::fs::copy(identity.directory_path().join("local-origin-ca.crt"), &ca)?;
    let app = f.app.clone();
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<crate::tls::PeerAddress>(),
        )
        .await
    }));
    let metrics = download(&origin, &cookie, &ca, &inputs.output).await?;
    verify_archive(&inputs, &jpeg)?;
    let report = json!({"teams": inputs.teams.len(), "independent_photo_objects":inputs.teams.len(),
        "jpeg_base_bytes": jpeg.len(), "build_mode":"debug test profile",
        "image_width": image.width(), "image_height": image.height(), "curl": metrics});
    std::fs::write(
        inputs.output.with_extension("metrics.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    std::fs::remove_file(ca)?;
    running.shutdown().await?;
    Ok(())
}
