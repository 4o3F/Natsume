//! Native lifecycle checks without Server credentials or a workspace.
use super::*;
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use winit::platform::windows::EventLoopBuilderExtWindows as _;

#[derive(Debug, Snafu)]
enum Failure {
    Platform { source: slint::PlatformError },
    Display { source: Error },
    Invariant,
}
type Result<T = ()> = std::result::Result<T, Failure>;

enum Stage {
    Start,
    Full,
    Retry,
    Windowed,
}

struct Journey {
    stage: Stage,
    started: Instant,
    old: Option<ContestantWindow>,
    receive: mpsc::Receiver<Command>,
}

impl Journey {
    fn advance(&mut self, app: &Rc<Controller>, ui: &PhotoWindow) -> Result<bool> {
        ensure!(
            self.started.elapsed() < Duration::from_secs(20),
            InvariantSnafu
        );
        match self.stage {
            Stage::Start => {
                if !ui.window().has_winit_window() {
                    return Ok(false);
                }
                app.refresh_monitors();
                preserve_monitor_identity(app, ui)?;
                ui.set_single_confirm(true);
                app.select_screens().context(DisplaySnafu)?;
                app.open_display().context(DisplaySnafu)?;
                let old = app
                    .display
                    .borrow()
                    .as_ref()
                    .context(InvariantSnafu)?
                    .component
                    .clone_strong();
                // Close before the event loop creates the native display window.
                old.window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested);
                ensure!(app.display.borrow().is_none(), InvariantSnafu);
                ensure!(self.receive.try_recv().is_err(), InvariantSnafu);
                self.old = Some(old);
                ensure!(
                    app.commands.try_send(Command::Cleanup).is_ok(),
                    InvariantSnafu
                );
                app.open_display().context(DisplaySnafu)?;
                self.stage = Stage::Full;
            }
            Stage::Full => {
                app.check_display();
                if app.display.borrow().is_none() {
                    ensure!(!ui.get_display_problem().is_empty(), InvariantSnafu);
                    ensure!(
                        !ui.get_display_online() && !ui.get_display_opening(),
                        InvariantSnafu
                    );
                    ensure!(
                        matches!(self.receive.try_recv(), Ok(Command::Cleanup)),
                        InvariantSnafu
                    );
                    app.open_display().context(DisplaySnafu)?;
                    self.stage = Stage::Retry;
                }
            }
            Stage::Retry => {
                app.check_display();
                if let Ok(command) = self.receive.try_recv() {
                    let Command::Attach(generation) = command else {
                        return InvariantSnafu.fail();
                    };
                    ensure!(
                        app.display
                            .borrow()
                            .as_ref()
                            .is_some_and(|d| d.generation == generation),
                        InvariantSnafu
                    );
                    self.old
                        .take()
                        .context(InvariantSnafu)?
                        .window()
                        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
                    ensure!(
                        app.display
                            .borrow()
                            .as_ref()
                            .is_some_and(|d| d.generation == generation),
                        InvariantSnafu
                    );
                    // Sending Attach alone must not mark the display online.
                    ensure!(!ui.get_display_online(), InvariantSnafu);
                    open_windowed(app)?;
                    self.stage = Stage::Windowed;
                }
            }
            Stage::Windowed => {
                app.check_display();
                if let Ok(command) = self.receive.try_recv() {
                    ensure!(matches!(command, Command::Attach(_)), InvariantSnafu);
                    ensure!(
                        app.display
                            .borrow()
                            .as_ref()
                            .is_some_and(|d| matches!(d.phase.get(), DisplayPhase::Attaching(_))),
                        InvariantSnafu
                    );
                    app.detach();
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

fn open_windowed(app: &Rc<Controller>) -> Result {
    app.detach();
    let monitor = app
        .monitors
        .borrow()
        .first()
        .cloned()
        .context(InvariantSnafu)?;
    *app.screens.borrow_mut() = Some(DisplayPlacement::Windowed(monitor));
    app.open_display().context(DisplaySnafu)
}

fn preserve_monitor_identity(app: &Controller, ui: &PhotoWindow) -> Result {
    let operator = app
        .monitors
        .borrow()
        .get(usize::try_from(ui.get_operator_monitor()).map_err(|_| InvariantSnafu.build())?)
        .cloned()
        .context(InvariantSnafu)?;
    let viewer = app
        .monitors
        .borrow()
        .get(usize::try_from(ui.get_display_monitor()).map_err(|_| InvariantSnafu.build())?)
        .cloned()
        .context(InvariantSnafu)?;
    // Simulate a different prior enumeration order while keeping selections intact.
    app.monitors.borrow_mut().reverse();
    let monitors = app.monitors.borrow();
    ui.set_operator_monitor(
        i32::try_from(
            monitors
                .iter()
                .position(|m| m == &operator)
                .context(InvariantSnafu)?,
        )
        .map_err(|_| InvariantSnafu.build())?,
    );
    ui.set_display_monitor(
        i32::try_from(
            monitors
                .iter()
                .position(|m| m == &viewer)
                .context(InvariantSnafu)?,
        )
        .map_err(|_| InvariantSnafu.build())?,
    );
    drop(monitors);
    app.refresh_monitors();
    let monitors = app.monitors.borrow();
    ensure!(
        monitors
            .get(usize::try_from(ui.get_operator_monitor()).map_err(|_| InvariantSnafu.build())?)
            == Some(&operator),
        InvariantSnafu
    );
    ensure!(
        monitors
            .get(usize::try_from(ui.get_display_monitor()).map_err(|_| InvariantSnafu.build())?)
            == Some(&viewer),
        InvariantSnafu
    );
    Ok(())
}

#[test]
#[ignore = "requires an interactive Windows desktop and the Skia renderer"]
fn native_display_queue_failure_retry_and_stale_close() -> Result {
    let _ = tracing_subscriber::fmt().with_target(false).try_init();
    let mut builder = winit::event_loop::EventLoop::with_user_event();
    builder.with_any_thread(true);
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .with_winit_event_loop_builder(builder)
        .select()
        .context(PlatformSnafu)?;
    let ui = PhotoWindow::new().context(PlatformSnafu)?;
    let (commands, receive) = mpsc::channel(1);
    let app = live_tests::controller(&ui, commands);
    ui.show().context(PlatformSnafu)?;
    let mut journey = Journey {
        stage: Stage::Start,
        started: Instant::now(),
        old: None,
        receive,
    };
    let failed = Rc::new(RefCell::new(None));
    let failure = failed.clone();
    let weak = ui.as_weak();
    let tick = app.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let result = weak
                .upgrade()
                .context(InvariantSnafu)
                .and_then(|ui| journey.advance(&tick, &ui));
            match result {
                Ok(false) => return,
                Ok(true) => {}
                Err(error) => {
                    *failure.borrow_mut() = Some(error);
                }
            }
            if let Err(error) = slint::quit_event_loop() {
                tracing::error!(%error, "cannot stop native display test");
            }
        },
    );
    let event = slint::run_event_loop().context(PlatformSnafu);
    timer.stop();
    app.detach();
    event?;
    if let Some(error) = failed.borrow_mut().take() {
        return Err(error);
    }
    Ok(())
}
