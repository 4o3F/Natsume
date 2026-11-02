//! Frameless native resize handling, before Slint captures pointer presses.
use super::winit;
use slint::winit_030::{EventResult, WinitWindowAccessor as _};
use snafu::ResultExt as _;
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, MouseButton, WindowEvent},
    keyboard::{KeyCode, PhysicalKey},
    window::ResizeDirection,
};

pub(super) fn bind(window: &slint::Window, allow_fullscreen: bool) {
    let mut cursor = None;
    let mut resize_cursor = false;
    window.on_winit_window_event(move |window, event| {
        if allow_fullscreen
            && matches!(event, WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat
                    && event.physical_key == PhysicalKey::Code(KeyCode::F11))
            && window.with_winit_window(winit::window::Window::has_focus) == Some(true)
        {
            window.set_fullscreen(!window.is_fullscreen());
            return EventResult::PreventDefault;
        }
        match event {
            WindowEvent::CursorMoved { position, .. } => cursor = Some(*position),
            WindowEvent::CursorLeft { .. } => cursor = None,
            _ => {}
        }
        window
            .with_winit_window(|native| {
                if native.fullscreen().is_some() || native.is_maximized() || !native.is_resizable()
                {
                    return EventResult::Propagate;
                }
                let edge = cursor.and_then(|point| {
                    resize_direction(point, native.inner_size(), 6.0 * native.scale_factor())
                });
                match (event, edge) {
                    (WindowEvent::CursorMoved { .. }, Some(direction)) => {
                        native.set_cursor(winit::window::CursorIcon::from(direction));
                        resize_cursor = true;
                        EventResult::PreventDefault
                    }
                    (
                        WindowEvent::MouseInput {
                            state: ElementState::Pressed,
                            button: MouseButton::Left,
                            ..
                        },
                        Some(direction),
                    ) => {
                        if let Err(error) = native
                            .drag_resize_window(direction)
                            .context(super::super::WindowResizeSnafu)
                        {
                            tracing::warn!(%error, "cannot resize frameless Photo window");
                        }
                        EventResult::PreventDefault
                    }
                    (WindowEvent::CursorMoved { .. }, None) if resize_cursor => {
                        native.set_cursor(winit::window::CursorIcon::Default);
                        resize_cursor = false;
                        EventResult::Propagate
                    }
                    _ => EventResult::Propagate,
                }
            })
            .unwrap_or(EventResult::Propagate)
    });
}

fn resize_direction(
    point: PhysicalPosition<f64>,
    size: PhysicalSize<u32>,
    border: f64,
) -> Option<ResizeDirection> {
    if point.x < 0.0
        || point.y < 0.0
        || point.x >= f64::from(size.width)
        || point.y >= f64::from(size.height)
    {
        return None;
    }
    let left = point.x < border;
    let right = point.x >= f64::from(size.width) - border;
    let top = point.y < border;
    let bottom = point.y >= f64::from(size.height) - border;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (_, true, true, _) => Some(ResizeDirection::NorthEast),
        (true, _, _, true) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, _, _, _) => Some(ResizeDirection::West),
        (_, true, _, _) => Some(ResizeDirection::East),
        (_, _, true, _) => Some(ResizeDirection::North),
        (_, _, _, true) => Some(ResizeDirection::South),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../backend/tests/unit/ui_window.rs"]
mod tests;
