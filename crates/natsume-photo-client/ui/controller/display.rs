use super::*;
use snafu::{OptionExt as _, ensure};

#[derive(Clone, Copy)]
pub(super) enum DisplayPhase {
    Opening(Instant),
    Positioning(Instant),
    Attaching(Instant),
    Ready,
    Closed,
}

impl DisplayWindow {
    pub fn ready(&self) -> bool {
        matches!(self.phase.get(), DisplayPhase::Ready) && !self.lost.get()
    }

    fn healthy(&self) -> bool {
        !self.lost.get()
            && self
                .component
                .window()
                .with_winit_window(|window| self.placement.healthy(window))
                .unwrap_or(false)
    }
}

impl Controller {
    pub(super) fn open_display(self: &Rc<Self>) -> Result<(), Error> {
        if self.display.borrow().is_some() {
            return Ok(());
        }
        ensure!(
            self.snapshot
                .borrow()
                .capture
                .as_ref()
                .and_then(display_id)
                .is_none(),
            super::super::DisplayClosingSnafu
        );
        let placement = self
            .screens
            .borrow()
            .clone()
            .context(super::super::DisplaySelectionSnafu)?;
        let component = ContestantWindow::new().context(PlatformSnafu)?;
        let pending = Rc::new(RefCell::new(None));
        let lost = Rc::new(Cell::new(false));
        let phase = Rc::new(Cell::new(DisplayPhase::Opening(Instant::now())));
        let generation = uuid::Uuid::now_v7();
        super::window::bind_contestant(
            &component,
            matches!(placement, DisplayPlacement::Windowed(_)),
        );
        self.bind_display(&component, &placement, &pending, &lost, &phase, generation)?;
        component.show().context(PlatformSnafu)?;
        *self.display.borrow_mut() = Some(DisplayWindow {
            component,
            pending,
            lost,
            generation,
            placement,
            phase,
        });
        self.installed.borrow_mut().take();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_display_problem("".into());
            ui.set_display_opening(true);
            ui.set_display_online(false);
        }
        Ok(())
    }

    fn bind_display(
        self: &Rc<Self>,
        component: &ContestantWindow,
        placement: &DisplayPlacement,
        pending: &Rc<RefCell<Option<PresentationRequest>>>,
        lost: &Rc<Cell<bool>>,
        phase: &Rc<Cell<DisplayPhase>>,
        generation: uuid::Uuid,
    ) -> Result<(), Error> {
        let slot = pending.clone();
        let renderer_lost = lost.clone();
        let phase = phase.clone();
        let placement = placement.clone();
        let weak_window = component.as_weak();
        let commands = self.commands.clone();
        component
            .window()
            .set_rendering_notifier(move |state, _| {
                if matches!(state, slint::RenderingState::RenderingTeardown) {
                    renderer_lost.set(true);
                    slot.borrow_mut().take();
                    return;
                }
                if matches!(state, slint::RenderingState::RenderingSetup)
                    && !matches!(phase.get(), DisplayPhase::Ready | DisplayPhase::Closed)
                {
                    renderer_lost.set(false);
                }
                if !matches!(state, slint::RenderingState::AfterRendering)
                    || !matches!(phase.get(), DisplayPhase::Ready)
                    || renderer_lost.get()
                {
                    return;
                }
                let healthy = weak_window.upgrade().is_some_and(|window| {
                    window
                        .window()
                        .with_winit_window(|native| placement.healthy(native))
                        .unwrap_or(false)
                });
                if !healthy {
                    renderer_lost.set(true);
                    slot.borrow_mut().take();
                    return;
                }
                let mut pending = slot.borrow_mut();
                if let Some(request) = pending.as_ref()
                    && commands
                        .try_send(Command::Rendered(super::receipt(request)))
                        .is_ok()
                {
                    pending.take();
                }
            })
            .context(RenderingSnafu)?;
        let weak = Rc::downgrade(self);
        component.window().on_close_requested(move || {
            if let Some(app) = weak.upgrade() {
                let current = app
                    .display
                    .borrow()
                    .as_ref()
                    .is_some_and(|d| d.generation == generation);
                if current {
                    app.detach();
                }
            }
            slint::CloseRequestResponse::HideWindow
        });
        Ok(())
    }

    fn advance_display(&self) -> Result<bool, Error> {
        let ui = self
            .ui
            .upgrade()
            .context(super::super::DisplayClosingSnafu)?;
        let displays = self.display.borrow();
        let Some(display) = displays.as_ref() else {
            return Ok(false);
        };
        match display.phase.get() {
            DisplayPhase::Opening(started) => {
                ensure!(
                    started.elapsed() < Duration::from_secs(5),
                    super::super::DisplayTimeoutSnafu
                );
                let available = display
                    .component
                    .window()
                    .with_winit_window(|w| display.placement.available(w));
                ensure!(
                    available != Some(false),
                    super::super::DisplaySelectionSnafu
                );
                if available == Some(true)
                    && display
                        .placement
                        .place(ui.window(), display.component.window())
                {
                    display.phase.set(DisplayPhase::Positioning(started));
                }
            }
            DisplayPhase::Positioning(started) => {
                ensure!(
                    started.elapsed() < Duration::from_secs(5),
                    super::super::DisplayTimeoutSnafu
                );
                let positioned = !display.lost.get()
                    && display
                        .component
                        .window()
                        .with_winit_window(|w| display.placement.positioned(ui.window(), w))
                        .unwrap_or(false);
                if positioned {
                    if self.snapshot.borrow().final_team.is_some() {
                        display.phase.set(DisplayPhase::Ready);
                        return Ok(true);
                    }
                    // Do not hide backpressure behind the general fire-and-forget UI sender.
                    self.commands
                        .try_send(Command::Attach(display.generation))
                        .map_err(|error| match error {
                            mpsc::error::TrySendError::Full(_) => {
                                super::super::DisplayQueueFullSnafu.build()
                            }
                            mpsc::error::TrySendError::Closed(_) => {
                                super::super::DisplayWorkerStoppedSnafu.build()
                            }
                        })?;
                    display.phase.set(DisplayPhase::Attaching(Instant::now()));
                }
            }
            DisplayPhase::Attaching(started) => {
                ensure!(
                    started.elapsed() < Duration::from_secs(5),
                    super::super::DisplayTimeoutSnafu
                );
                ensure!(display.healthy(), super::super::DisplayInvalidSnafu);
                let snapshot = self.snapshot.borrow();
                if snapshot.final_team.is_some()
                    || snapshot.display_generation == Some(display.generation)
                        && snapshot.capture.as_ref().and_then(display_id).is_some()
                {
                    display.phase.set(DisplayPhase::Ready);
                    return Ok(true);
                }
            }
            DisplayPhase::Ready => {
                ensure!(display.healthy(), super::super::DisplayInvalidSnafu);
                if display.pending.borrow().is_some() {
                    display.component.window().request_redraw();
                }
            }
            DisplayPhase::Closed => {}
        }
        Ok(false)
    }

    pub(super) fn check_display(self: &Rc<Self>) {
        if self.quitting.get() {
            self.detach();
            return;
        }
        match self.advance_display() {
            Ok(true) => {
                let snapshot = self.snapshot.borrow().clone();
                self.apply(snapshot);
            }
            Ok(false) => {}
            Err(error) => {
                self.detach();
                self.report_display_error(&error);
            }
        }
    }

    pub(super) fn report_display_error(&self, error: &Error) {
        tracing::warn!(%error, "Photo display unavailable");
        if let Some(ui) = self.ui.upgrade() {
            ui.set_display_problem(error.to_string().into());
        }
    }

    pub(super) fn detach(&self) {
        self.clear_render();
        self.installed.borrow_mut().take();
        if let Some(id) = self.snapshot.borrow().capture.as_ref().and_then(display_id) {
            self.send(Command::Detach(id), false);
        }
        let display = self.display.borrow_mut().take();
        if let Some(display) = display {
            display.phase.set(DisplayPhase::Closed);
            display.component.set_photo(slint::Image::default());
            if let Err(error) = display.component.hide() {
                tracing::error!(%error, "cannot hide contestant window");
            }
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_display_opening(false);
            ui.set_display_online(false);
            ui.set_presented(false);
            ui.set_verbal(false);
        }
    }
}

pub(crate) fn receipt(request: &PresentationRequest) -> PresentationReceipt {
    PresentationReceipt {
        display_id: request.display_id(),
        revision: request.revision(),
        group_id: request.group_id(),
        account_id: request.account_id().to_owned(),
        photo_id: request.photo_id(),
        image_sha256: request.image().sha256().to_owned(),
    }
}

pub(super) fn display_id(
    snapshot: &natsume_photo_client::capture_session::HotFolderSnapshot,
) -> Option<DisplayId> {
    match &snapshot.presentation {
        PresentationStatus::Unavailable => None,
        PresentationStatus::Blank { display_id } => Some(*display_id),
        PresentationStatus::Pending(request) | PresentationStatus::Presented(request) => {
            Some(request.display_id())
        }
    }
}
