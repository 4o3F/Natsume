use super::*;

impl Controller {
    pub(super) fn refresh_monitors(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if let Some(monitors) = ui
            .window()
            .with_winit_window(|window| window.available_monitors().collect::<Vec<_>>())
        {
            let labels: Vec<slint::SharedString> = monitors
                .iter()
                .enumerate()
                .map(|(index, monitor)| {
                    let size = monitor.size();
                    format!(
                        "显示器 {} · {} · {}×{}",
                        index + 1,
                        monitor.name().unwrap_or_default(),
                        size.width,
                        size.height
                    )
                    .into()
                })
                .collect();
            ui.set_monitors(ModelRc::new(VecModel::from(labels)));
            *self.monitors.borrow_mut() = monitors;
        }
    }

    pub(super) fn open_display(self: &Rc<Self>) -> Result<(), Error> {
        if self.display.borrow().is_some() {
            return Ok(());
        }
        let display = ContestantWindow::new().context(PlatformSnafu)?;
        if self
            .snapshot
            .borrow()
            .capture
            .as_ref()
            .and_then(display_id)
            .is_some()
        {
            return super::super::DisplayClosingSnafu.fail();
        }
        let pending = Rc::new(RefCell::new(None::<PresentationRequest>));
        let slot = pending.clone();
        let lost = Rc::new(Cell::new(false));
        let renderer_lost = lost.clone();
        let generation = uuid::Uuid::now_v7();
        let commands = self.commands.clone();
        display
            .window()
            .set_rendering_notifier(move |state, _| {
                if matches!(state, slint::RenderingState::RenderingTeardown) {
                    renderer_lost.set(true);
                    slot.borrow_mut().take();
                    return;
                }
                if !matches!(state, slint::RenderingState::AfterRendering) || renderer_lost.get() {
                    return;
                }
                let mut pending = slot.borrow_mut();
                if let Some(request) = pending.as_ref() {
                    let receipt = super::receipt(request);
                    if commands.try_send(Command::Rendered(receipt)).is_ok() {
                        pending.take();
                    }
                }
            })
            .context(RenderingSnafu)?;
        let weak = Rc::downgrade(self);
        display.window().on_close_requested(move || {
            if let Some(app) = weak.upgrade() {
                app.detach();
            }
            slint::CloseRequestResponse::HideWindow
        });
        display.show().context(PlatformSnafu)?;
        if let Some(ui) = self.ui.upgrade() {
            let monitors = self.monitors.borrow();
            if monitors.len() > 1 {
                let operator = usize::try_from(ui.get_operator_monitor())
                    .ok()
                    .and_then(|index| monitors.get(index));
                let chosen = usize::try_from(ui.get_display_monitor())
                    .ok()
                    .and_then(|index| monitors.get(index));
                if let (Some(operator), Some(chosen)) = (operator, chosen) {
                    ui.window()
                        .with_winit_window(|window| window.set_outer_position(operator.position()));
                    display.window().with_winit_window(|window| {
                        window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(Some(
                            chosen.clone(),
                        ))));
                    });
                    *self.display_monitor.borrow_mut() = Some(chosen.clone());
                }
            } else if let Some(monitor) = monitors.first() {
                // Centered defaults completely overlap on one screen. Keep the
                // contestant window visible when the operator takes focus;
                // Windows may stop painting a fully occluded window.
                ui.window().with_winit_window(|window| {
                    window.set_outer_position(monitor.position());
                });
                display.window().with_winit_window(|window| {
                    let origin = monitor.position();
                    let available = monitor
                        .size()
                        .width
                        .saturating_sub(window.outer_size().width);
                    let offset = i32::try_from(available).unwrap_or(i32::MAX);
                    window.set_outer_position(winit::dpi::PhysicalPosition::new(
                        origin.x.saturating_add(offset),
                        origin.y,
                    ));
                });
            }
        }
        *self.display.borrow_mut() = Some(DisplayWindow {
            component: display,
            pending,
            lost,
            generation,
        });
        self.installed.borrow_mut().take();
        self.send(Command::Attach(generation), true);
        Ok(())
    }

    pub(super) fn detach(&self) {
        self.clear_render();
        self.installed.borrow_mut().take();
        if let Some(id) = self.snapshot.borrow().capture.as_ref().and_then(display_id) {
            self.send(Command::Detach(id), false);
        }
        if let Some(display) = self.display.borrow_mut().take() {
            display.component.set_photo(slint::Image::default());
            if let Err(error) = display.component.hide() {
                tracing::error!(%error, "cannot hide contestant window");
            }
        }
        self.display_monitor.borrow_mut().take();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_display_online(false);
            ui.set_presented(false);
            ui.set_verbal(false);
        }
    }

    pub(super) fn check_display(&self) {
        if self.display.borrow().is_none() {
            return;
        }
        let disconnected = self
            .ui
            .upgrade()
            .and_then(|ui| {
                ui.window().with_winit_window(|window| {
                    self.display_monitor
                        .borrow()
                        .as_ref()
                        .is_some_and(|monitor| {
                            !window
                                .available_monitors()
                                .any(|available| available == *monitor)
                        })
                })
            })
            .unwrap_or(true);
        let minimized = self
            .display
            .borrow()
            .as_ref()
            .and_then(|display| {
                display
                    .component
                    .window()
                    .with_winit_window(slint::winit_030::winit::window::Window::is_minimized)
            })
            .flatten()
            .unwrap_or(false);
        let lost = self
            .display
            .borrow()
            .as_ref()
            .is_some_and(|display| display.lost.get());
        if disconnected || minimized || lost {
            self.detach();
        } else if let Some(display) = self.display.borrow().as_ref()
            && display.pending.borrow().is_some()
        {
            display.component.window().request_redraw();
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
