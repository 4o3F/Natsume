//! Opt-in live acceptance: real HTTPS Server, Windows vault, Slint callbacks and renderer.
use super::*;
use natsume_photo_client::workspace::WorkspaceReader;
use natsume_photo_protocol::{
    DatasetId, InitializationId, Sha256Digest, SigningPublicKey, SourceId,
    results::{ResultContext, SignedResults},
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use std::{
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use winit::platform::windows::EventLoopBuilderExtWindows as _;

#[derive(Debug, Snafu)]
enum Failure {
    #[snafu(display("live test requires {name}"))]
    Environment {
        name: &'static str,
    },
    Io {
        source: std::io::Error,
    },
    Platform {
        source: slint::PlatformError,
    },
    Backend {
        source: application::Error,
    },
    Workspace {
        source: natsume_photo_client::workspace::Error,
    },
    Image {
        source: image::ImageError,
    },
    Json {
        source: serde_json::Error,
    },
    Zip {
        source: zip::result::ZipError,
    },
    Signature {
        source: natsume_photo_protocol::results::ResultError,
    },
    #[snafu(display("live UI failed: {reason}"))]
    Ui {
        reason: String,
    },
    #[snafu(display("live UI timed out at {stage:?}"))]
    Timeout {
        stage: Stage,
    },
    #[snafu(display("live acceptance invariant failed"))]
    Invariant,
    ThreadPanicked,
}
type Result<T = ()> = std::result::Result<T, Failure>;

struct Config {
    root: PathBuf,
    hot: PathBuf,
    zip: PathBuf,
    ca: PathBuf,
    url: String,
    source: String,
}

fn required(name: &'static str) -> Result<String> {
    std::env::var(name).map_err(|_| EnvironmentSnafu { name }.build())
}

impl Config {
    fn load() -> Result<Self> {
        Ok(Self {
            root: required("PHOTO_LIVE_WORKSPACE")?.into(),
            hot: required("PHOTO_LIVE_HOT_FOLDER")?.into(),
            zip: required("PHOTO_LIVE_ZIP")?.into(),
            ca: required("PHOTO_LIVE_CA")?.into(),
            url: required("PHOTO_LIVE_URL")?,
            source: required("PHOTO_LIVE_SOURCE")?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Initialize,
    Session,
    Team,
    Group,
    Photos,
    Choose,
    Presented,
    Confirmed,
    Finished,
    Exported,
    Done,
}

struct Journey {
    config: Arc<Config>,
    stage: Stage,
    started: Instant,
    selected: Option<uuid::Uuid>,
    originals: Vec<(PathBuf, String)>,
}

impl Journey {
    fn next(&mut self, next: Stage) {
        tracing::info!(stage = ?next, "live Photo UI stage");
        self.stage = next;
        self.started = Instant::now();
    }

    fn advance(&mut self, app: &Rc<Controller>, ui: &PhotoWindow) -> Result<bool> {
        ensure!(
            self.started.elapsed() < Duration::from_secs(150),
            TimeoutSnafu { stage: self.stage }
        );
        ensure!(
            ui.get_problem().is_empty(),
            UiSnafu {
                reason: ui.get_problem().to_string()
            }
        );
        if ui.get_busy() {
            return Ok(false);
        }
        let snapshot = app.snapshot.borrow().clone();
        match self.stage {
            Stage::Initialize if snapshot.root.is_some() => {
                ensure!(!snapshot.teams.is_empty(), InvariantSnafu);
                app.refresh_monitors();
                ui.set_hot_folder(self.config.hot.display().to_string().into());
                // Explicit automated consent for the single-display dialog; no production bypass.
                ui.set_single_confirm(true);
                ui.set_operator_monitor(0);
                ui.set_display_monitor(1);
                ui.invoke_start_session();
                self.next(Stage::Session);
            }
            Stage::Session if ui.get_display_online() => {
                let team = snapshot.teams.first().context(InvariantSnafu)?;
                ui.invoke_select_team(team.account_id.clone().into());
                self.next(Stage::Team);
            }
            Stage::Team if snapshot.selected.is_some() => {
                ui.invoke_start_group();
                self.next(Stage::Group);
            }
            Stage::Group if ui.get_group_active() => {
                self.write_photos()?;
                self.next(Stage::Photos);
            }
            Stage::Photos if snapshot.thumbnails.len() == 2 && ui.get_pending() == 0 => {
                let photo = snapshot.thumbnails.last().context(InvariantSnafu)?.id;
                self.selected = Some(photo);
                ui.invoke_choose_photo(photo.to_string().into());
                self.next(Stage::Choose);
            }
            Stage::Choose
                if snapshot
                    .capture
                    .as_ref()
                    .is_some_and(|capture| capture.selection.photo_id() == self.selected) =>
            {
                self.next(Stage::Presented);
            }
            Stage::Presented if ui.get_presented() => {
                let frame = snapshot.frame.as_ref().context(InvariantSnafu)?;
                ensure!(
                    Some(frame.request.photo_id()) == self.selected,
                    InvariantSnafu
                );
                save_screenshot(ui, &self.config, "presented")?;
                ui.set_verbal(true);
                ui.invoke_confirm_photo();
                self.next(Stage::Confirmed);
            }
            Stage::Confirmed if ui.get_confirmed() => {
                save_screenshot(ui, &self.config, "confirmed")?;
                ui.invoke_finish_group();
                self.next(Stage::Finished);
            }
            Stage::Finished if !ui.get_group_active() => {
                ensure!(snapshot.thumbnails.is_empty(), InvariantSnafu);
                ui.set_page(6);
                ui.set_export_path(self.config.zip.display().to_string().into());
                ui.invoke_export_results();
                self.next(Stage::Exported);
            }
            Stage::Exported if self.config.zip.is_file() => {
                ensure!(
                    snapshot.export_status.starts_with("导出完成"),
                    InvariantSnafu
                );
                for (path, expected) in &self.originals {
                    ensure!(&hash_file(path)? == expected, InvariantSnafu);
                }
                save_screenshot(ui, &self.config, "exported")?;
                self.next(Stage::Done);
            }
            Stage::Done => return Ok(true),
            _ => {}
        }
        Ok(false)
    }

    fn write_photos(&mut self) -> Result {
        for (name, base) in [("camera-one.png", 35_u8), ("camera-two.jpg", 145_u8)] {
            let path = self.config.hot.join(name);
            image::RgbImage::from_fn(640, 480, |x, y| {
                image::Rgb([
                    base,
                    u8::try_from(x % 256).unwrap_or(0),
                    u8::try_from(y % 256).unwrap_or(0),
                ])
            })
            .save(&path)
            .context(ImageSnafu)?;
            self.originals.push((path.clone(), hash_file(&path)?));
        }
        Ok(())
    }
}

fn hash_file(path: &Path) -> Result<String> {
    Ok(hex::encode(Sha256::digest(
        fs::read(path).context(IoSnafu)?,
    )))
}

fn save_screenshot(ui: &PhotoWindow, config: &Config, name: &str) -> Result {
    let image = ui.window().take_snapshot().context(PlatformSnafu)?;
    let directory = config.zip.parent().context(InvariantSnafu)?;
    image::save_buffer(
        directory.join(format!("{name}.png")),
        image.as_bytes(),
        image.width(),
        image.height(),
        image::ColorType::Rgba8,
    )
    .context(ImageSnafu)
}

fn controller(ui: &PhotoWindow, commands: mpsc::Sender<Command>) -> Rc<Controller> {
    // Fixture-owned state uses the same production callbacks, mapping and display adapter.
    Rc::new(Controller {
        ui: ui.as_weak(),
        commands,
        snapshot: RefCell::default(),
        display: RefCell::default(),
        monitors: RefCell::default(),
        display_monitor: RefCell::default(),
        installed: RefCell::default(),
        quitting: Cell::new(false),
        shutdown_sent: Cell::new(false),
        thumbnails: RefCell::default(),
        final_preview: RefCell::default(),
    })
}

#[test]
#[ignore = "requires a live authorized Server, one-time code, Windows vault and desktop"]
fn live_server_ui_initialization_through_signed_zip() -> Result {
    let _ = tracing_subscriber::fmt().with_target(false).try_init();
    let config = Arc::new(Config::load()?);
    ensure!(
        !config.root.exists() && !config.zip.exists(),
        InvariantSnafu
    );
    fs::create_dir_all(&config.hot).context(IoSnafu)?;
    fs::create_dir_all(config.zip.parent().context(InvariantSnafu)?).context(IoSnafu)?;
    let mut builder = winit::event_loop::EventLoop::with_user_event();
    builder.with_any_thread(true);
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .with_winit_event_loop_builder(builder)
        .select()
        .context(PlatformSnafu)?;
    let ui = PhotoWindow::new().context(PlatformSnafu)?;
    let (commands, receive) = mpsc::channel(32);
    let (updates, mut snapshots) = watch::channel(Snapshot::default());
    let worker = std::thread::spawn(move || application::run(receive, &updates));
    let app = controller(&ui, commands);
    bind(&ui, &app);
    ui.show().context(PlatformSnafu)?;
    ui.set_server_url(config.url.clone().into());
    ui.set_workspace_path(config.root.display().to_string().into());
    ui.set_custom_ca(true);
    ui.set_ca_path(config.ca.display().to_string().into());
    // A previously paired workspace resumes with its original vault credentials.
    let code = zeroize::Zeroizing::new(std::env::var("PHOTO_LIVE_CODE").unwrap_or_default());
    ui.set_pairing_code(code.as_str().into());
    ui.invoke_initialize();
    let failed = Rc::new(RefCell::new(None));
    let failure = failed.clone();
    let app_tick = app.clone();
    let mut journey = Journey {
        config: config.clone(),
        stage: Stage::Initialize,
        started: Instant::now(),
        selected: None,
        originals: Vec::new(),
    };
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let result = (|| {
                let ui = weak.upgrade().context(InvariantSnafu)?;
                if snapshots.has_changed().unwrap_or(true) {
                    app_tick.apply(snapshots.borrow_and_update().clone());
                }
                app_tick.check_display();
                journey.advance(&app_tick, &ui)
            })();
            match result {
                Ok(false) => {}
                Ok(true) => {
                    let _ = slint::quit_event_loop();
                }
                Err(error) => {
                    *failure.borrow_mut() = Some(error);
                    let _ = slint::quit_event_loop();
                }
            }
        },
    );
    let event_result = slint::run_event_loop().context(PlatformSnafu);
    timer.stop();
    app.quitting.set(true);
    app.clear_render();
    app.display.borrow_mut().take();
    ensure!(
        app.commands.blocking_send(Command::Shutdown).is_ok(),
        InvariantSnafu
    );
    worker
        .join()
        .map_err(|_| ThreadPanickedSnafu.build())?
        .context(BackendSnafu)?;
    event_result?;
    if let Some(error) = failed.borrow_mut().take() {
        return Err(error);
    }
    verify_package(&config)
}

#[derive(Deserialize)]
struct Authorization {
    source_id: SourceId,
    dataset_id: DatasetId,
    initialization_id: InitializationId,
    manifest_sha256: Sha256Digest,
    signing_public_key: SigningPublicKey,
}

fn verify_package(config: &Config) -> Result {
    let authorization: Authorization =
        serde_json::from_slice(&fs::read(config.root.join("authorization.json")).context(IoSnafu)?)
            .context(JsonSnafu)?;
    ensure!(
        authorization.source_id.as_str() == config.source,
        InvariantSnafu
    );
    let context = ResultContext {
        source_id: authorization.source_id,
        dataset_id: authorization.dataset_id,
        initialization_id: authorization.initialization_id,
        initialization_sha256: authorization.manifest_sha256,
    };
    let mut archive =
        zip::ZipArchive::new(fs::File::open(&config.zip).context(IoSnafu)?).context(ZipSnafu)?;
    let mut manifest = Vec::new();
    archive
        .by_name("manifest.json")
        .context(ZipSnafu)?
        .read_to_end(&mut manifest)
        .context(IoSnafu)?;
    let signed =
        SignedResults::parse_and_verify(&manifest, &context, &authorization.signing_public_key)
            .context(SignatureSnafu)?;
    ensure!(
        signed.manifest.results.len() == 1 && archive.len() == 2,
        InvariantSnafu
    );
    let result = &signed.manifest.results[0];
    let mut image = Vec::new();
    archive
        .by_name(&result.image_entry())
        .context(ZipSnafu)?
        .read_to_end(&mut image)
        .context(IoSnafu)?;
    ensure!(
        image.len() as u64 == result.image_bytes && Sha256Digest::of(&image) == result.image_sha256,
        InvariantSnafu
    );
    let decoded = image::load_from_memory(&image).context(ImageSnafu)?;
    ensure!(
        decoded.width() == result.width && decoded.height() == result.height,
        InvariantSnafu
    );
    let runtime = tokio::runtime::Runtime::new().context(IoSnafu)?;
    let workspace = runtime
        .block_on(WorkspaceReader::inspect(&config.root))
        .context(WorkspaceSnafu)?;
    let finals = runtime
        .block_on(workspace.confirmed_photos())
        .context(WorkspaceSnafu)?;
    ensure!(
        finals.len() == 1
            && finals[0].account_id() == result.account_id
            && finals[0].sha256() == result.image_sha256.as_str(),
        InvariantSnafu
    );
    tracing::info!(
        teams = runtime
            .block_on(workspace.roster())
            .context(WorkspaceSnafu)?
            .teams()
            .len(),
        results = signed.manifest.results.len(),
        zip_bytes = fs::metadata(&config.zip).context(IoSnafu)?.len(),
        "live Server to native UI to signed ZIP passed"
    );
    Ok(())
}
