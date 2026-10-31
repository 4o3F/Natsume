mod bindings;
mod display;
mod view;

use bindings::bind;
use display::display_id;
pub(super) use display::receipt;

use super::{
    ContestantWindow, Error, PhotoRow, PhotoWindow, PlatformSnafu, RenderingSnafu, TeamRow,
    ThreadSnafu,
    application::{self, Command, GroupAction, Snapshot},
};
use natsume_photo_client::capture_session::{
    CandidateSelection, DisplayId, PresentationReceipt, PresentationRequest, PresentationStatus,
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
    time::Duration,
};
use tokio::sync::{mpsc, watch};

struct DisplayWindow {
    component: ContestantWindow,
    pending: Rc<RefCell<Option<PresentationRequest>>>,
    lost: Rc<Cell<bool>>,
    generation: uuid::Uuid,
}

struct Controller {
    ui: slint::Weak<PhotoWindow>,
    commands: mpsc::Sender<Command>,
    snapshot: RefCell<Snapshot>,
    display: RefCell<Option<DisplayWindow>>,
    monitors: RefCell<Vec<winit::monitor::MonitorHandle>>,
    display_monitor: RefCell<Option<winit::monitor::MonitorHandle>>,
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
    let worker = std::thread::Builder::new()
        .name("photo-application".to_owned())
        .spawn(move || application::run(receiver, &updates))
        .context(ThreadSnafu)?;
    let app = Rc::new(Controller {
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

impl Controller {
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
        match self.commands.try_send(command) {
            Ok(()) => {
                if busy {
                    ui.set_busy(true);
                    ui.set_status("正在处理，请稍候…".into());
                }
            }
            Err(_) => {
                ui.set_problem("后台正忙或已经停止，请稍后重试".into());
            }
        }
    }
}
