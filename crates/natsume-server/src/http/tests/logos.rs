use super::*;

#[tokio::test]
#[ignore = "local 1000-request logo micro-benchmark"]
async fn logo_conditional_micro_benchmark() -> Result<(), crate::test_error::TestError> {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let fixture = TestDatabase::new().await?;
    let logos = tempfile::tempdir()?;
    let pixels = image::RgbImage::from_pixel(512, 512, image::Rgb([64, 128, 192]));
    pixels.save(logos.path().join("示例大学.png"))?;
    let features =
        feature::tests::ServerFixture::new(fixture.database.clone(), fixture.documents_path())
            .logos(logos.path().to_path_buf())
            .build_with_features()?;
    let xlsx = include_bytes!("../../component/import/examples/roster.xlsx");
    let preview = features
        .import
        .component()
        .try_begin()?
        .create_candidate(xlsx)
        .await?;
    let _change = features
        .import
        .component()
        .try_begin()?
        .commit(preview.candidate_id(), preview.preview_token_bytes(), xlsx)
        .await?;
    let application = features.loaded.http_router(unused_web_root());
    let path = "/api/v2/organizations/INST-001/logo";
    let warm = drive(&application, request(Method::GET, path, "")?).await?;
    assert_eq!(warm.status, StatusCode::OK);
    let etag = warm.headers[header::ETAG].clone();
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        let mut conditional = request(Method::GET, path, "")?;
        conditional
            .headers_mut()
            .insert(header::IF_NONE_MATCH, etag.clone());
        assert_eq!(
            drive(&application, conditional).await?.status,
            StatusCode::NOT_MODIFIED
        );
    }
    tracing::info!(
        requests = 1000,
        total_ms = started.elapsed().as_secs_f64() * 1000.0,
        per_request_us = started.elapsed().as_secs_f64() * 1000.0,
        "logo conditional handler benchmark"
    );
    Ok(())
}

#[tokio::test]
async fn conditional_logo_uses_current_roster_names_and_team_membership()
-> Result<(), crate::test_error::TestError> {
    use crate::diesel_schema::{organizations, teams};
    use diesel::{ExpressionMethods as _, QueryDsl as _};

    let fixture = TestDatabase::new().await?;
    let logos = tempfile::tempdir()?;
    let source = logos.path().join("示例大学.svg");
    fs::write(
        &source,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="3"/>"#,
    )?;
    let features =
        feature::tests::ServerFixture::new(fixture.database.clone(), fixture.documents_path())
            .logos(logos.path().to_path_buf())
            .build_with_features()?;
    let xlsx = include_bytes!("../../component/import/examples/roster.xlsx");
    let preview = features
        .import
        .component()
        .try_begin()?
        .create_candidate(xlsx)
        .await?;
    let _change = features
        .import
        .component()
        .try_begin()?
        .commit(preview.candidate_id(), preview.preview_token_bytes(), xlsx)
        .await?;
    let application = features.loaded.http_router(unused_web_root());
    let path = "/api/v2/organizations/INST-001/logo";
    let first = drive(&application, request(Method::GET, path, "")?).await?;
    assert_eq!(first.status, StatusCode::OK);
    let etag = first.headers[header::ETAG].clone();
    fixture
        .database
        .write(|transaction| {
            diesel::update(organizations::table.find(1_i64))
                .set((
                    organizations::name_zh.eq("Renamed"),
                    organizations::name_en.eq("Renamed"),
                ))
                .execute(transaction.connection())
                .map_err(crate::db::PersistenceError::from)
        })
        .await?;
    let mut conditional = request(Method::GET, path, "")?;
    conditional
        .headers_mut()
        .insert(header::IF_NONE_MATCH, etag.clone());
    assert_eq!(
        drive(&application, conditional).await?.status,
        StatusCode::NOT_FOUND
    );
    fs::rename(source, logos.path().join("Renamed.svg"))?;
    let mut conditional = request(Method::GET, path, "")?;
    conditional
        .headers_mut()
        .insert(header::IF_NONE_MATCH, etag.clone());
    assert_eq!(
        drive(&application, conditional).await?.status,
        StatusCode::NOT_MODIFIED
    );
    fixture
        .database
        .write(|transaction| {
            diesel::delete(teams::table.filter(teams::organization_id.eq(1_i64)))
                .execute(transaction.connection())
                .map_err(crate::db::PersistenceError::from)
        })
        .await?;
    let mut conditional = request(Method::GET, path, "")?;
    conditional
        .headers_mut()
        .insert(header::IF_NONE_MATCH, etag);
    assert_eq!(
        drive(&application, conditional).await?.status,
        StatusCode::NOT_FOUND
    );
    for id in ["INST-1", "INST-0001", "1", "INST-001x"] {
        assert_eq!(
            drive(
                &application,
                request(Method::GET, &format!("/api/v2/organizations/{id}/logo"), "")?
            )
            .await?
            .status,
            StatusCode::NOT_FOUND
        );
    }
    Ok(())
}
