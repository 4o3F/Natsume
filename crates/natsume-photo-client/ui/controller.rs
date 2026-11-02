mod bindings;
mod display;
mod monitors;
mod view;
mod window;

#[cfg(test)]
#[path = "../backend/tests/unit/ui_live.rs"]
mod live_tests;

#[cfg(test)]
#[path = "../backend/tests/unit/ui_display.rs"]
mod display_tests;

#[cfg(test)]
#[path = "../backend/tests/unit/ui_performance.rs"]
mod performance_tests;

use bindings::bind;
pub(super) use display::receipt;
use display::{DisplayPhase, display_id};
use monitors::DisplayPlacement;

use super::{
    ContestantWindow, Error, PhotoRow, PhotoWindow, PlatformSnafu, RenderingSnafu, TeamRow,
    ThreadSnafu, WindowDragSnafu, WindowEventLoopSnafu,
    application::{self, Command, GroupAction, Snapshot},
};
use natsume_photo_client::capture_session::{
    DisplayId, PresentationReceipt, PresentationRequest, PresentationStatus,
};
use slint::{
    ComponentHandle as _, ModelRc, VecModel,
    winit_030::{WinitWindowAccessor as _, winit},
};
use snafu::ResultExt as _;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

struct DisplayWindow {
    component: ContestantWindow,
    pending: Rc<RefCell<Option<PresentationRequest>>>,
    lost: Rc<Cell<bool>>,
    generation: uuid::Uuid,
    placement: DisplayPlacement,
    phase: Rc<Cell<DisplayPhase>>,
}

struct Controller {
    ui: slint::Weak<PhotoWindow>,
    commands: mpsc::Sender<Command>,
    snapshot: RefCell<Snapshot>,
    display: RefCell<Option<DisplayWindow>>,
    monitors: RefCell<Vec<winit::monitor::MonitorHandle>>,
    screens: RefCell<Option<DisplayPlacement>>,
    installed: RefCell<Option<PresentationRequest>>,
    quitting: Cell<bool>,
    shutdown_sent: Cell<bool>,
    thumbnails: RefCell<HashMap<uuid::Uuid, slint::Image>>,
    final_preview: RefCell<Option<std::sync::Arc<super::images::Pixels>>>,
}

pub(super) fn run() -> Result<(), Error> {
    // Software rendering has no completed-frame notifier. Select a renderer which
    // supports that boundary; never synthesize a receipt using a timer.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .select()
        .context(PlatformSnafu)?;
    let ui = PhotoWindow::new().context(PlatformSnafu)?;
    let (commands, receiver) = mpsc::channel(32);
    let (updates, mut snapshots) = watch::channel(Snapshot::default());
    let (progress, mut progress_updates) = natsume_photo_client::progress::Reporter::channel();
    let worker = std::thread::Builder::new()
        .name("photo-application".to_owned())
        .spawn(move || application::run(receiver, &updates, progress))
        .context(ThreadSnafu)?;
    let app = Rc::new(Controller {
        ui: ui.as_weak(),
        commands,
        snapshot: RefCell::default(),
        display: RefCell::default(),
        monitors: RefCell::default(),
        screens: RefCell::default(),
        installed: RefCell::default(),
        quitting: Cell::new(false),
        shutdown_sent: Cell::new(false),
        thumbnails: RefCell::default(),
        final_preview: RefCell::default(),
    });
    bind(&ui, &app);
    ui.show().context(PlatformSnafu)?;
    app.refresh_monitors();
    let timer = slint::Timer::default();
    let app_tick = app.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let app = &app_tick;
            if progress_updates.has_changed().unwrap_or(false)
                && let Some(ui) = app.ui.upgrade()
            {
                apply_progress(&ui, &progress_updates.borrow_and_update());
            }
            let changed = snapshots.has_changed();
            if changed.is_err() {
                tracing::info!("Photo worker closed; stopping window event loop");
                // has_changed reports Closed even when the final stopped snapshot
                // has not been read. The joined worker carries its actual result.
                if let Err(error) = slint::quit_event_loop() {
                    tracing::error!(%error, "cannot quit after Photo worker closed");
                }
                return;
            }
            if matches!(changed, Ok(true)) {
                let snapshot = snapshots.borrow_and_update().clone();
                if snapshot.stopped {
                    if let Err(error) = slint::quit_event_loop() {
                        tracing::error!(%error, "cannot quit Photo event loop");
                    }
                    return;
                }
                app.apply(snapshot);
            }
            if app.monitors.borrow().is_empty() {
                app.refresh_monitors();
            }
            app.check_display();
            if app.quitting.get() && !app.shutdown_sent.get() {
                app.shutdown_sent
                    .set(app.commands.try_send(Command::Shutdown).is_ok());
            }
        },
    );
    let event_result = slint::run_event_loop().context(PlatformSnafu);
    timer.stop();
    app.clear_render();
    app.display.borrow_mut().take();
    // Also drain on an unexpected event-loop exit. An accepted export must finish
    // before its runtime is dropped; CaptureSession::shutdown waits for file work.
    if !app.shutdown_sent.get() {
        let _ = app.commands.blocking_send(Command::Shutdown);
    }
    let joined = worker.join().map_err(|_| Error::ThreadPanicked);
    event_result?;
    joined?.map_err(|_| Error::ApplicationStopped)
}

fn apply_progress(ui: &PhotoWindow, progress: &natsume_photo_client::progress::Progress) {
    use natsume_photo_client::progress::Stage;
    ui.set_progress_step(match progress.stage {
        Stage::Prepare => 0,
        Stage::Process => 1,
        Stage::Verify => 2,
        Stage::Publish => 3,
    });
    ui.set_progress_label(progress.label.into());
    ui.set_progress_known(progress.total > 0);
    ui.set_progress_value(if progress.total > 0 {
        let thousandths =
            u128::from(progress.completed.min(progress.total)) * 1000 / u128::from(progress.total);
        f32::from(u16::try_from(thousandths).unwrap_or(1000)) / 1000.0
    } else {
        0.0
    });
    ui.set_progress_count(if progress.total > 0 {
        format!("{} / {}", progress.completed, progress.total).into()
    } else {
        "处理中".into()
    });
}

impl Controller {
    fn request_shutdown(&self) {
        tracing::info!("operator requested Photo shutdown");
        self.quitting.set(true);
        self.clear_render();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_busy(true);
            ui.set_status("正在等待已开始的文件与导出任务结束，请勿强制关闭…".into());
        }
    }
    fn clear_render(&self) {
        if let Some(display) = self.display.borrow().as_ref() {
            display.pending.borrow_mut().take();
        }
    }

    fn send_group(&self, action: GroupAction) {
        if let Some(id) = self
            .snapshot
            .borrow()
            .capture
            .as_ref()
            .and_then(|capture| capture.group_id)
        {
            self.send(Command::Group { id, action }, true);
        }
    }

    fn send(&self, command: Command, busy: bool) {
        if self.quitting.get() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if busy && ui.get_busy() {
            return;
        }
        let status = match &command {
            Command::Open(_) => "正在校验本地工作区和离线授权，无需连接服务器…",
            Command::Initialize { .. } => "正在授权并下载离线资料，请稍候…",
            Command::Export(_) => "正在校验成品并生成 ZIP，请稍候…",
            _ => "正在处理，请稍候…",
        };
        let exporting = matches!(command, Command::Export(_));
        if matches!(
            command,
            Command::Open(_) | Command::Initialize { .. } | Command::Export(_)
        ) {
            ui.set_progress_visible(true);
            ui.set_progress_export(matches!(command, Command::Export(_)));
            ui.set_progress_step(0);
            ui.set_progress_value(0.0);
            ui.set_progress_known(false);
            ui.set_progress_label(status.into());
            ui.set_progress_count("处理中".into());
        }
        match self.commands.try_send(command) {
            Ok(()) => {
                if busy {
                    ui.set_busy(true);
                    ui.set_status(status.into());
                    if exporting {
                        ui.set_export_status("正在准备导出…".into());
                    }
                }
            }
            Err(_) => {
                ui.set_problem("后台正忙或已经停止，请稍后重试".into());
            }
        }
    }
}
