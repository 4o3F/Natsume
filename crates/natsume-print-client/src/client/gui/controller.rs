use slint::{ComponentHandle as _, ModelRc, VecModel};
use snafu::{IntoError as _, ResultExt as _};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::{mpsc, watch};

use super::PrintWindow;
use crate::client::{
    application::{self, Control, PairingStatus, Phase, Problem, Snapshot},
    config::Config,
    error::{ApplicationThreadSnafu, AttentionSnafu, GuiSnafu, InvalidSnafu, Result, StorageSnafu},
};

pub(super) type Runner =
    fn(PathBuf, mpsc::Receiver<Control>, &watch::Sender<Snapshot>) -> Result<()>;

struct Application {
    controls: RefCell<Option<mpsc::Sender<Control>>>,
    worker: RefCell<Option<JoinHandle<()>>>,
    discovery: RefCell<Option<JoinHandle<()>>>,
    quitting: Cell<bool>,
    snapshots: watch::Sender<Snapshot>,
}

fn quit() {
    if let Err(error) = slint::quit_event_loop() {
        tracing::error!(%error, "could not stop the window event loop");
    }
}

impl Application {
    fn start(&self, ui: &PrintWindow, runner: Runner) {
        if self.worker.borrow().is_some() || self.quitting.get() {
            return;
        }
        let path = PathBuf::from(ui.get_config_path().as_str());
        let (sender, receive) = mpsc::channel(8);
        let updates = self.snapshots.clone();
        self.snapshots.send_replace(Snapshot {
            running: true,
            paused: true,
            phase: Phase::Loading,
            ..Snapshot::default()
        });
        let result = std::thread::Builder::new()
            .name("print-client-application".to_owned())
            .spawn(move || {
                let result = runner(path, receive, &updates);
                let mut final_state = updates.borrow().clone();
                final_state.running = false;
                if final_state.pairing != PairingStatus::Confirmed {
                    final_state.pairing = PairingStatus::Unknown;
                }
                final_state.message = match result {
                    Ok(()) => {
                        final_state.phase = Phase::Stopped;
                        "客户端已停止，原有工作保留供下次核实、观察及补报。".to_owned()
                    }
                    Err(error) => {
                        if final_state.problem != Problem::Configuration {
                            final_state.problem = Problem::Attention;
                        }
                        tracing::error!(%error, "client application stopped; no new claims");
                        error.to_string()
                    }
                };
                updates.send_replace(final_state);
            });
        match result {
            Ok(handle) => {
                *self.worker.borrow_mut() = Some(handle);
                *self.controls.borrow_mut() = Some(sender);
                ui.set_running(true);
                ui.set_screen(super::Screen::Setup);
                ui.set_settings_open(false);
                ui.set_message("正在加载配置及核实本地记录…".into());
            }
            Err(source) => {
                let error = StorageSnafu {
                    operation: "start application thread",
                }
                .into_error(source);
                self.snapshots.send_replace(Snapshot {
                    message: error.to_string(),
                    ..Snapshot::default()
                });
                ui.set_running(false);
                ui.set_message(error.to_string().into());
            }
        }
    }

    fn control(&self, ui: &PrintWindow, control: Control) {
        let result = self.controls.borrow().as_ref().map_or_else(
            || {
                AttentionSnafu {
                    reason: "client is not running",
                }
                .fail()
            },
            |sender| application::send(control, sender),
        );
        if let Err(error) = result {
            super::view::operation_error(ui, &error.to_string());
        }
    }

    fn stop(&self, ui: &PrintWindow) {
        self.quitting.set(true);
        ui.set_closing(true);
        ui.set_can_control(false);
        ui.set_claim_title("已停止".into());
        ui.set_claim_description("正在停止新工作，并等待已启动的原生调用返回。".into());
        ui.set_message("正在正常退出，请勿强制结束进程或清理记录。".into());
        if self.worker.borrow().is_none() {
            quit();
        }
    }

    fn poll(&self, ui: &PrintWindow, receiver: &mut watch::Receiver<Snapshot>) {
        if receiver.has_changed().unwrap_or(false) {
            let snapshot = receiver.borrow_and_update().clone();
            super::view::present(ui, &snapshot);
            if self.quitting.get() {
                ui.set_can_control(false);
                ui.set_claim_title("已停止".into());
            }
        }
        if self.quitting.get()
            && let Some(sender) = self.controls.borrow().as_ref()
        {
            // A full command channel must not lose a shutdown request. Retry without blocking the UI.
            let _ = sender.try_send(Control::Stop);
        }
        if self
            .worker
            .borrow()
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            if self
                .worker
                .borrow_mut()
                .take()
                .is_some_and(|handle| handle.join().is_err())
            {
                let error = ApplicationThreadSnafu.build();
                tracing::error!(%error);
                let mut snapshot = self.snapshots.borrow().clone();
                snapshot.running = false;
                snapshot.problem = Problem::Attention;
                snapshot.message = error.to_string();
                if snapshot.pairing != PairingStatus::Confirmed {
                    snapshot.pairing = PairingStatus::Unknown;
                }
                self.snapshots.send_replace(snapshot.clone());
                super::view::present(ui, &snapshot);
            }
            self.controls.borrow_mut().take();
            if self.quitting.get() {
                quit();
            }
        }
    }

    fn discover(&self, ui: &PrintWindow, discover: fn() -> crate::Result<Vec<String>>) {
        if self
            .discovery
            .borrow()
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            return;
        }
        if self
            .discovery
            .borrow_mut()
            .take()
            .is_some_and(|handle| handle.join().is_err())
        {
            tracing::error!("queue discovery thread stopped unexpectedly");
        }
        let weak = ui.as_weak();
        match std::thread::Builder::new()
            .name("print-queue-discovery".to_owned())
            .spawn(move || {
                let result = discover();
                let _ = weak.upgrade_in_event_loop(move |ui| match result {
                    Ok(names) => ui.set_queues(ModelRc::new(VecModel::from(
                        names
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<slint::SharedString>>(),
                    ))),
                    Err(error) => ui.set_message(error.to_string().into()),
                });
            }) {
            Ok(handle) => {
                *self.discovery.borrow_mut() = Some(handle);
            }
            Err(source) => {
                let error = StorageSnafu {
                    operation: "start queue discovery",
                }
                .into_error(source);
                ui.set_message(error.to_string().into());
            }
        }
    }

    fn shutdown(&self) -> Result<()> {
        if let Some(sender) = self.controls.borrow_mut().take() {
            let _ = sender.blocking_send(Control::Stop);
        }
        let work = self.worker.borrow_mut().take().map(JoinHandle::join);
        let discovery = self.discovery.borrow_mut().take().map(JoinHandle::join);
        if work.is_some_and(|result| result.is_err())
            || discovery.is_some_and(|result| result.is_err())
        {
            return ApplicationThreadSnafu.fail();
        }
        Ok(())
    }
}

fn configure(ui: &PrintWindow) {
    let weak = ui.as_weak();
    ui.on_create_config(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let ca = ui.get_ca_path();
        let config = Config {
            server_url: ui.get_server_url().to_string(),
            queue_name: ui.get_queue_name().to_string(),
            state_directory: PathBuf::from(ui.get_state_directory().as_str()),
            ca_certificate: if ca.is_empty() {
                None
            } else {
                Some(PathBuf::from(ca.as_str()))
            },
        };
        let result = config
            .validated()
            .and_then(|config| config.save_new(&PathBuf::from(ui.get_config_path().as_str())));
        ui.set_message(match result {
            Ok(()) => {
                ui.set_new_deployment(false);
                ui.set_form_error(false);
                "配置已创建。点击“加载配置”继续；原有记录不会被重置。".into()
            }
            Err(error) => {
                ui.set_form_error(true);
                error.to_string().into()
            }
        });
    });
}

fn bind_controls(
    ui: &PrintWindow,
    application: &Rc<Application>,
    runner: Runner,
    discover: fn() -> crate::Result<Vec<String>>,
) {
    let weak = ui.as_weak();
    let owner = application.clone();
    ui.on_start_client(move || {
        if let Some(ui) = weak.upgrade() {
            owner.start(&ui, runner);
        }
    });
    let weak = ui.as_weak();
    let owner = application.clone();
    ui.on_refresh_queues(move || {
        if let Some(ui) = weak.upgrade() {
            owner.discover(&ui, discover);
        }
    });
    let weak = ui.as_weak();
    let owner = application.clone();
    ui.on_pair(move |code| {
        if let Some(ui) = weak.upgrade()
            && let Some(code) = super::code::canonical(code.as_str())
        {
            owner.control(&ui, Control::Pair(code));
            super::code::clear(&ui);
        }
    });
    let weak = ui.as_weak();
    let owner = application.clone();
    ui.on_pause_client(move || {
        if let Some(ui) = weak.upgrade() {
            owner.control(&ui, Control::Pause);
        }
    });
    let weak = ui.as_weak();
    let owner = application.clone();
    ui.on_resume_client(move || {
        if let Some(ui) = weak.upgrade() {
            owner.control(&ui, Control::Resume);
        }
    });
    let weak = ui.as_weak();
    ui.on_request_exit(move || {
        if let Some(ui) = weak.upgrade() {
            if let Err(error) = ui.show() {
                tracing::warn!(%error, "could not show exit confirmation");
            }
            ui.set_settings_open(false);
            ui.set_exit_confirm(true);
        }
    });
    let weak = ui.as_weak();
    let owner = application.clone();
    ui.on_stop_client(move || {
        if let Some(ui) = weak.upgrade() {
            owner.stop(&ui);
        }
    });
}

pub(super) fn run(
    path: Option<PathBuf>,
    base: &Path,
    runner: Runner,
    discover: fn() -> crate::Result<Vec<String>>,
) -> Result<()> {
    let configured = path.is_some();
    let path = path.unwrap_or_else(|| base.join("client.toml"));
    let ui = PrintWindow::new().context(GuiSnafu)?;
    ui.set_config_path(
        path.to_str()
            .ok_or_else(|| {
                InvalidSnafu {
                    field: "configuration path Unicode",
                }
                .build()
            })?
            .into(),
    );
    ui.set_state_directory(
        base.join("state")
            .to_str()
            .ok_or_else(|| {
                InvalidSnafu {
                    field: "state path Unicode",
                }
                .build()
            })?
            .into(),
    );
    let (snapshots, mut receiver) = watch::channel(Snapshot::default());
    let application = Rc::new(Application {
        controls: RefCell::new(None),
        worker: RefCell::new(None),
        discovery: RefCell::new(None),
        quitting: Cell::new(false),
        snapshots,
    });
    configure(&ui);
    super::code::bind(&ui);
    #[cfg(windows)]
    super::desktop::bind(&ui);
    bind_controls(&ui, &application, runner, discover);
    #[cfg(windows)]
    let tray = Rc::new(super::tray::Tray::new(&ui)?);
    let weak = ui.as_weak();
    #[cfg(windows)]
    let tray_owner = tray.clone();
    ui.window().on_close_requested(move || {
        if let Some(ui) = weak.upgrade() {
            #[cfg(windows)]
            if tray_owner.available() {
                if let Err(error) = ui.hide() {
                    tracing::warn!(%error, "could not hide client window");
                }
                return slint::CloseRequestResponse::KeepWindowShown;
            }
            ui.invoke_request_exit();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    let weak = ui.as_weak();
    let owner = application.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(200),
        move || {
            if let Some(ui) = weak.upgrade() {
                owner.poll(&ui, &mut receiver);
                #[cfg(windows)]
                tray.poll(&ui);
            }
        },
    );
    ui.invoke_refresh_queues();
    if configured {
        ui.invoke_start_client();
    }
    let result = ui
        .show()
        .and_then(|()| slint::run_event_loop_until_quit())
        .context(GuiSnafu);
    drop(timer);
    let cleanup = application.shutdown();
    result.and(cleanup)
}

#[cfg(test)]
mod tests;
