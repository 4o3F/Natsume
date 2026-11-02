use super::*;
use snafu::{OptionExt as _, ensure};

#[derive(Clone)]
pub(super) enum DisplayPlacement {
    Windowed(winit::monitor::MonitorHandle),
    Fullscreen {
        operator: winit::monitor::MonitorHandle,
        viewer: winit::monitor::MonitorHandle,
    },
}

impl DisplayPlacement {
    pub fn available(&self, window: &winit::window::Window) -> bool {
        let monitors: Vec<_> = window.available_monitors().collect();
        match self {
            Self::Windowed(m) => monitors.contains(m),
            Self::Fullscreen { operator, viewer } => {
                monitors.contains(operator) && monitors.contains(viewer)
            }
        }
    }

    pub fn positioned(
        &self,
        operator_window: &slint::Window,
        viewer_window: &winit::window::Window,
    ) -> bool {
        let monitor = match self {
            Self::Windowed(m) => m,
            Self::Fullscreen { operator, .. } => operator,
        };
        if operator_window.with_winit_window(|w| w.current_monitor().as_ref() == Some(monitor))
            != Some(true)
            || !self.healthy(viewer_window)
        {
            return false;
        }
        match self {
            Self::Fullscreen { .. } => true,
            Self::Windowed(m) => {
                if viewer_window.fullscreen().is_some() {
                    return true;
                }
                let offset = m
                    .size()
                    .width
                    .saturating_sub(viewer_window.outer_size().width);
                let expected = winit::dpi::PhysicalPosition::new(
                    m.position()
                        .x
                        .saturating_add(i32::try_from(offset).unwrap_or(i32::MAX)),
                    m.position().y,
                );
                viewer_window.outer_position().is_ok_and(|p| p == expected)
            }
        }
    }

    pub fn healthy(&self, window: &winit::window::Window) -> bool {
        if window.is_minimized() == Some(true) || window.is_visible() == Some(false) {
            return false;
        }
        let available: Vec<_> = window.available_monitors().collect();
        match self {
            Self::Windowed(monitor) => {
                available.contains(monitor)
                    && window.current_monitor().as_ref() == Some(monitor)
                    && match window.fullscreen() {
                        None => true,
                        Some(winit::window::Fullscreen::Borderless(selected)) => {
                            selected.as_ref().is_none_or(|selected| selected == monitor)
                        }
                        _ => false,
                    }
            }
            Self::Fullscreen { operator, viewer } => {
                available.contains(operator)
                    && available.contains(viewer)
                    && window.current_monitor().as_ref() == Some(viewer)
                    && window.inner_size() == viewer.size()
                    && window
                        .outer_position()
                        .is_ok_and(|p| p == viewer.position())
                    && matches!(window.fullscreen(), Some(winit::window::Fullscreen::Borderless(Some(ref m))) if m == viewer)
            }
        }
    }

    pub fn place(&self, operator_window: &slint::Window, viewer_window: &slint::Window) -> bool {
        let operator = match self {
            Self::Windowed(monitor) => monitor,
            Self::Fullscreen { operator, .. } => operator,
        };
        // Restore before moving: a maximized window otherwise retains its old monitor.
        operator_window.set_maximized(false);
        if operator_window
            .with_winit_window(|w| w.set_outer_position(operator.position()))
            .is_none()
        {
            return false;
        }
        viewer_window
            .with_winit_window(|window| match self {
                Self::Windowed(monitor) => {
                    viewer_window.set_fullscreen(false);
                    window.set_fullscreen(None);
                    let offset = monitor
                        .size()
                        .width
                        .saturating_sub(window.outer_size().width);
                    let x = monitor
                        .position()
                        .x
                        .saturating_add(i32::try_from(offset).unwrap_or(i32::MAX));
                    window.set_outer_position(winit::dpi::PhysicalPosition::new(
                        x,
                        monitor.position().y,
                    ));
                }
                Self::Fullscreen { viewer, .. } => {
                    window.set_outer_position(viewer.position());
                    window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(Some(
                        viewer.clone(),
                    ))));
                    // Keep Slint's property and the native fullscreen state consistent.
                    viewer_window.set_fullscreen(true);
                }
            })
            .is_some()
    }
}

impl Controller {
    pub(super) fn refresh_monitors(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some((monitors, primary)) = ui.window().with_winit_window(|w| {
            (
                w.available_monitors().collect::<Vec<_>>(),
                w.primary_monitor(),
            )
        }) else {
            return;
        };
        let previous = self.monitors.borrow();
        let selected = |index| {
            usize::try_from(index)
                .ok()
                .and_then(|i| previous.get(i))
                .cloned()
        };
        let operator = selected(ui.get_operator_monitor());
        let viewer = selected(ui.get_display_monitor());
        let first = previous.is_empty();
        drop(previous);
        let (operator_index, viewer_index) = if first {
            let operator = primary
                .as_ref()
                .and_then(|p| monitors.iter().position(|m| m == p))
                .unwrap_or(0);
            let viewer = (0..monitors.len())
                .find(|i| *i != operator)
                .unwrap_or(operator);
            (
                i32::try_from(operator).unwrap_or(-1),
                i32::try_from(viewer).unwrap_or(-1),
            )
        } else {
            (
                restore_index(&monitors, operator.as_ref()),
                restore_index(&monitors, viewer.as_ref()),
            )
        };
        let labels: Vec<slint::SharedString> = monitors
            .iter()
            .map(|m| {
                let size = m.size();
                let position = m.position();
                format!(
                    "{} · {} · {}×{} · ({},{})",
                    m.name().unwrap_or_default(),
                    if primary.as_ref() == Some(m) {
                        "主屏"
                    } else {
                        "副屏"
                    },
                    size.width,
                    size.height,
                    position.x,
                    position.y
                )
                .into()
            })
            .collect();
        *self.monitors.borrow_mut() = monitors;
        ui.set_monitors(ModelRc::new(VecModel::from(labels)));
        ui.set_operator_monitor(operator_index);
        ui.set_display_monitor(viewer_index);
    }

    pub(super) fn select_screens(&self) -> Result<(), Error> {
        self.refresh_monitors();
        let ui = self
            .ui
            .upgrade()
            .context(super::super::DisplayClosingSnafu)?;
        let monitors = self.monitors.borrow();
        let at = |index| {
            usize::try_from(index)
                .ok()
                .and_then(|i| monitors.get(i))
                .cloned()
        };
        let operator =
            at(ui.get_operator_monitor()).context(super::super::DisplaySelectionSnafu)?;
        let placement = if monitors.len() == 1 {
            ensure!(ui.get_single_confirm(), super::super::DisplaySelectionSnafu);
            DisplayPlacement::Windowed(operator)
        } else {
            let viewer =
                at(ui.get_display_monitor()).context(super::super::DisplaySelectionSnafu)?;
            ensure!(operator != viewer, super::super::DisplaySelectionSnafu);
            DisplayPlacement::Fullscreen { operator, viewer }
        };
        *self.screens.borrow_mut() = Some(placement);
        Ok(())
    }
}

fn restore_index(
    monitors: &[winit::monitor::MonitorHandle],
    selected: Option<&winit::monitor::MonitorHandle>,
) -> i32 {
    selected
        .and_then(|s| monitors.iter().position(|m| m == s))
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1)
}
