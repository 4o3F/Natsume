use super::*;
use diesel::{
    Connection as _, ExpressionMethods as _, RunQueryDsl as _, connection::SimpleConnection as _,
};
use snafu::Snafu;
use std::path::Path;

#[path = "../../src/diesel_schema.rs"]
mod schema;

const TEAM: &str = "0199a000-0000-7000-8000-000000000001";

#[derive(Debug, Snafu)]
enum TestError {
    Io {
        source: std::io::Error,
    },
    Connection {
        source: diesel::ConnectionError,
    },
    Query {
        source: diesel::result::Error,
    },
    Application {
        source: super::Error,
    },
    Workspace {
        source: natsume_photo_client::workspace::Error,
    },
    Capture {
        source: natsume_photo_client::capture_session::Error,
    },
    Image {
        source: image::ImageError,
    },
    Platform {
        source: slint::PlatformError,
    },
    EventLoop {
        source: slint::EventLoopError,
    },
    Rendering {
        source: slint::SetRenderingNotifierError,
    },
    Fixture,
}
type TestResult<T = ()> = std::result::Result<T, TestError>;

#[test]
fn application_shutdown_leaves_a_final_snapshot_after_the_sender_closes() -> TestResult {
    let (commands, receive) = mpsc::channel(1);
    let (updates, snapshots) = watch::channel(Snapshot::default());
    assert!(commands.blocking_send(Command::Shutdown).is_ok());
    super::run(receive, &updates).context(ApplicationSnafu)?;
    drop(updates);
    assert!(snapshots.has_changed().is_err());
    assert!(snapshots.borrow().stopped);
    Ok(())
}

async fn fixture() -> TestResult<(tempfile::TempDir, Application, PathBuf)> {
    let temporary = tempfile::tempdir().context(IoSnafu)?;
    let root = temporary.path().join("workspace");
    std::fs::create_dir(&root).context(IoSnafu)?;
    let mut db = diesel::SqliteConnection::establish(
        root.join("workspace.sqlite3")
            .to_str()
            .context(FixtureSnafu)?,
    )
    .context(ConnectionSnafu)?;
    // Actual migration statements are distinct from ordinary fixture DML.
    for migration in [
        include_str!("../../migrations/00000000000001_workspace/up.sql"),
        include_str!("../../migrations/00000000000002_display_resources/up.sql"),
        include_str!("../../migrations/00000000000003_confirmed_results/up.sql"),
    ] {
        db.batch_execute(migration).context(QuerySnafu)?;
    }
    diesel::insert_into(schema::photo_roster::table)
        .values((
            schema::photo_roster::account_id.eq(TEAM),
            schema::photo_roster::account.eq("team001"),
            schema::photo_roster::organization_id.eq(1_i64),
            schema::photo_roster::school_name_zh.eq("测试大学"),
            schema::photo_roster::school_name_en.eq("Test University"),
            schema::photo_roster::team_name_zh.eq("测试队伍"),
            schema::photo_roster::team_name_en.eq("Test Team"),
        ))
        .execute(&mut db)
        .context(QuerySnafu)?;
    diesel::update(schema::photo_workspace::table)
        .set(schema::photo_workspace::ready.eq(1))
        .execute(&mut db)
        .context(QuerySnafu)?;
    drop(db);
    let mut app = Application::default();
    // Deliberate test assembly only: exercise capture without creating production
    // credentials. The real Open command always performs authorization diagnosis.
    app.use_workspace(Workspace::open(&root).await.context(WorkspaceSnafu)?)
        .await
        .context(ApplicationSnafu)?;
    let hot = temporary.path().join("camera");
    std::fs::create_dir(&hot).context(IoSnafu)?;
    app.execute(Command::Session(hot.clone()))
        .await
        .context(ApplicationSnafu)?;
    app.execute(Command::Select(TEAM.to_owned()))
        .await
        .context(ApplicationSnafu)?;
    app.execute(Command::Start(TEAM.to_owned()))
        .await
        .context(ApplicationSnafu)?;
    Ok((temporary, app, hot))
}

async fn ready(app: &mut Application, hot: &Path) -> TestResult<PresentationRequest> {
    app.execute(Command::Attach(Uuid::now_v7()))
        .await
        .context(ApplicationSnafu)?;
    image::RgbImage::from_pixel(80, 60, image::Rgb([226, 35, 26]))
        .save(hot.join("photo.png"))
        .context(ImageSnafu)?;
    app.session()
        .context(ApplicationSnafu)?
        .wait_until(Duration::from_secs(15), |state| {
            state.pending_tasks == 0 && state.candidates().count() == 1
        })
        .await
        .context(CaptureSnafu)?
        .context(FixtureSnafu)?;
    app.refresh().await.context(ApplicationSnafu)?;
    Ok(app
        .view
        .frame
        .as_ref()
        .context(FixtureSnafu)?
        .request
        .clone())
}

#[tokio::test]
async fn ui_owner_requires_exact_frame_then_saves_and_cleans_without_deleting_camera_source()
-> TestResult {
    let (_temporary, mut app, hot) = fixture().await?;
    let request = ready(&mut app, &hot).await?;
    assert!(
        app.execute(Command::Confirm(request.clone()))
            .await
            .is_err()
    );
    app.execute(Command::Rendered(crate::gui::controller::receipt(&request)))
        .await
        .context(ApplicationSnafu)?;
    app.execute(Command::Confirm(request.clone()))
        .await
        .context(ApplicationSnafu)?;
    assert_eq!(app.view.finals.len(), 1);
    app.execute(Command::Group {
        id: request.group_id(),
        action: GroupAction::Finish,
    })
    .await
    .context(ApplicationSnafu)?;
    app.refresh().await.context(ApplicationSnafu)?;
    assert!(app.view.thumbnails.is_empty());
    assert!(app.view.frame.is_none());
    assert!(hot.join("photo.png").is_file());
    assert_eq!(
        app.workspace
            .as_ref()
            .context(FixtureSnafu)?
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?
            .len(),
        1
    );
    app.execute(Command::Cleanup)
        .await
        .context(ApplicationSnafu)?;
    app.execute(Command::PreviewFinal(TEAM.to_owned()))
        .await
        .context(ApplicationSnafu)?;
    assert!(app.view.final_preview.is_some());
    assert!(
        app.view.frame.is_none(),
        "viewing a saved result cannot create a presentation receipt"
    );
    assert!(
        app.execute(Command::PreviewFinal("missing".to_owned()))
            .await
            .is_err()
    );
    assert!(app.view.final_preview.is_none());
    app.session
        .take()
        .context(FixtureSnafu)?
        .shutdown()
        .await
        .context(CaptureSnafu)?;
    Ok(())
}

#[tokio::test]
async fn stale_group_command_cannot_abandon_the_retake_group() -> TestResult {
    let (_temporary, mut app, _hot) = fixture().await?;
    let old = app
        .session()
        .context(ApplicationSnafu)?
        .snapshot()
        .context(CaptureSnafu)?
        .group_id
        .context(FixtureSnafu)?;
    app.execute(Command::Group {
        id: old,
        action: GroupAction::Retake,
    })
    .await
    .context(ApplicationSnafu)?;
    let current = app
        .session()
        .context(ApplicationSnafu)?
        .snapshot()
        .context(CaptureSnafu)?
        .group_id;
    assert_ne!(current, Some(old));
    assert!(
        app.execute(Command::Group {
            id: old,
            action: GroupAction::Abandon
        })
        .await
        .is_err()
    );
    assert_eq!(
        app.session()
            .context(ApplicationSnafu)?
            .snapshot()
            .context(CaptureSnafu)?
            .group_id,
        current
    );
    app.session
        .take()
        .context(FixtureSnafu)?
        .shutdown()
        .await
        .context(CaptureSnafu)?;
    Ok(())
}

#[tokio::test]
async fn late_render_receipt_does_not_acknowledge_or_interrupt_the_new_photo() -> TestResult {
    let (_temporary, mut app, hot) = fixture().await?;
    let old = ready(&mut app, &hot).await?;
    image::RgbImage::from_pixel(80, 60, image::Rgb([15, 80, 240]))
        .save(hot.join("second.png"))
        .context(ImageSnafu)?;
    app.session()
        .context(ApplicationSnafu)?
        .wait_until(Duration::from_secs(15), |state| {
            state.pending_tasks == 0 && state.candidates().count() == 2
        })
        .await
        .context(CaptureSnafu)?
        .context(FixtureSnafu)?;
    app.refresh().await.context(ApplicationSnafu)?;
    let current = app
        .view
        .frame
        .as_ref()
        .context(FixtureSnafu)?
        .request
        .clone();
    assert_ne!(old.photo_id(), current.photo_id());
    app.execute(Command::Rendered(crate::gui::controller::receipt(&old)))
        .await
        .context(ApplicationSnafu)?;
    assert!(
        matches!(app.session().context(ApplicationSnafu)?.snapshot().context(CaptureSnafu)?.presentation,
        PresentationStatus::Pending(ref request) if request == &current)
    );
    app.execute(Command::Rendered(crate::gui::controller::receipt(&current)))
        .await
        .context(ApplicationSnafu)?;
    assert!(
        matches!(app.session().context(ApplicationSnafu)?.snapshot().context(CaptureSnafu)?.presentation,
        PresentationStatus::Presented(ref request) if request == &current)
    );
    app.session
        .take()
        .context(FixtureSnafu)?
        .shutdown()
        .await
        .context(CaptureSnafu)?;
    Ok(())
}

#[tokio::test]
async fn changed_image_never_produces_pixels_or_a_receipt() -> TestResult {
    let (_temporary, mut app, hot) = fixture().await?;
    let request = ready(&mut app, &hot).await?;
    std::fs::write(request.image().path(), b"tampered").context(IoSnafu)?;
    assert!(images::candidate(request.image()).is_err());
    app.session
        .take()
        .context(FixtureSnafu)?
        .shutdown()
        .await
        .context(CaptureSnafu)?;
    Ok(())
}

#[test]
#[ignore = "requires an interactive Windows desktop and the Skia renderer"]
fn native_windows_render_each_page_and_acknowledge_only_a_rendered_photo() -> TestResult {
    use slint::{
        ComponentHandle as _,
        winit_030::{winit, winit::platform::windows::EventLoopBuilderExtWindows as _},
    };
    use std::{cell::RefCell, rc::Rc};
    let runtime = tokio::runtime::Runtime::new().context(IoSnafu)?;
    let (_temporary, mut app, hot) = runtime.block_on(fixture())?;
    let request = runtime.block_on(ready(&mut app, &hot))?;
    let mut builder = winit::event_loop::EventLoop::with_user_event();
    builder.with_any_thread(true);
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .with_winit_event_loop_builder(builder)
        .select()
        .context(PlatformSnafu)?;
    let ui = crate::gui::PhotoWindow::new().context(PlatformSnafu)?;
    ui.window().set_size(slint::LogicalSize::new(1440., 960.));
    let display = crate::gui::ContestantWindow::new().context(PlatformSnafu)?;
    let rendered = Rc::new(RefCell::new(None));
    let rendered_callback = rendered.clone();
    let target = request.clone();
    display
        .window()
        .set_rendering_notifier(move |state, _| {
            if matches!(state, slint::RenderingState::AfterRendering) {
                *rendered_callback.borrow_mut() = Some(crate::gui::controller::receipt(&target));
            }
        })
        .context(RenderingSnafu)?;
    display.set_has_team(true);
    display.set_has_photo(true);
    display.set_account("team001".into());
    display.set_school("测试大学".into());
    display.set_school_en("Test University".into());
    display.set_team("测试队伍".into());
    display.set_photo(
        app.view
            .frame
            .as_ref()
            .context(FixtureSnafu)?
            .pixels
            .image(),
    );
    assert!(
        rendered.borrow().is_none(),
        "loading pixels alone cannot acknowledge presentation"
    );
    ui.show().context(PlatformSnafu)?;
    display.show().context(PlatformSnafu)?;
    capture_pages(&ui, &app.view)?;
    runtime
        .block_on(app.execute(Command::Rendered(
            rendered.borrow_mut().take().context(FixtureSnafu)?,
        )))
        .context(ApplicationSnafu)?;
    runtime
        .block_on(app.execute(Command::Confirm(request)))
        .context(ApplicationSnafu)?;
    assert_eq!(app.view.finals.len(), 1);
    runtime
        .block_on(app.session.take().context(FixtureSnafu)?.shutdown())
        .context(CaptureSnafu)?;
    Ok(())
}

fn capture_pages(ui: &crate::gui::PhotoWindow, snapshot: &Snapshot) -> TestResult {
    use slint::ComponentHandle as _;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    prepare_workbench_view(ui, snapshot)?;
    let output =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/photo-ui-screenshots");
    std::fs::create_dir_all(&output).context(IoSnafu)?;
    let stage = Rc::new(Cell::new(0));
    let failure = Rc::new(RefCell::new(None));
    let failure_callback = failure.clone();
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            let result = (|| {
                let ui = weak.upgrade().context(FixtureSnafu)?;
                let step = stage.get();
                let screenshot = ui.window().take_snapshot().context(PlatformSnafu)?;
                image::save_buffer(
                    output.join(format!("page-{step}.png")),
                    screenshot.as_bytes(),
                    screenshot.width(),
                    screenshot.height(),
                    image::ColorType::Rgba8,
                )
                .context(ImageSnafu)?;
                if step == 9 {
                    slint::quit_event_loop().context(EventLoopSnafu)?;
                } else {
                    ui.set_ready(true);
                    ui.set_page(step + 1);
                    if step >= 7 {
                        ui.set_page(3);
                        ui.set_group_active(true);
                    }
                    if step == 8 {
                        ui.set_dialog_action(2);
                    }
                    ui.set_team_count(1);
                    ui.set_teams(slint::ModelRc::new(slint::VecModel::from(vec![
                        crate::gui::TeamRow {
                            id: TEAM.into(),
                            account: "team001".into(),
                            school: "测试大学".into(),
                            school_en: "Test University".into(),
                            name: "测试队伍".into(),
                            name_en: "Test Team".into(),
                            r#final: false,
                        },
                    ])));
                    stage.set(step + 1);
                }
                Ok::<_, TestError>(())
            })();
            if let Err(error) = result {
                *failure_callback.borrow_mut() = Some(error);
                let _ = slint::quit_event_loop();
            }
        },
    );
    slint::run_event_loop().context(PlatformSnafu)?;
    timer.stop();
    if let Some(error) = failure.borrow_mut().take() {
        return Err(error);
    }
    Ok(())
}

fn prepare_workbench_view(ui: &crate::gui::PhotoWindow, snapshot: &Snapshot) -> TestResult {
    ui.set_selected_id(TEAM.into());
    ui.set_account("team001".into());
    ui.set_school("测试大学".into());
    ui.set_team("测试队伍".into());
    ui.set_team_en("Test Team".into());
    ui.set_session_active(true);
    ui.set_display_online(true);
    ui.set_preview(
        snapshot
            .frame
            .as_ref()
            .context(FixtureSnafu)?
            .pixels
            .image(),
    );
    ui.set_photo_label("#01".into());
    ui.set_candidates(slint::ModelRc::new(slint::VecModel::from(
        snapshot
            .thumbnails
            .iter()
            .map(|thumbnail| crate::gui::PhotoRow {
                id: thumbnail.id.to_string().into(),
                title: "#01".into(),
                note: thumbnail.note.clone().into(),
                thumbnail: thumbnail.pixels.image(),
                selected: true,
            })
            .collect::<Vec<_>>(),
    )));
    Ok(())
}
