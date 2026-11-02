use super::*;
use slint::ComponentHandle as _;
use snafu::{OptionExt as _, Snafu, ensure};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};
use winit::platform::windows::EventLoopBuilderExtWindows as _;

#[derive(Debug, Snafu)]
enum Failure {
    Platform { source: slint::PlatformError },
    Invariant,
}

#[test]
fn resize_hit_testing_covers_edges_corners_and_scaled_borders() {
    let size = PhysicalSize::new(1440, 960);
    for (x, y, direction) in [
        (0.0, 0.0, ResizeDirection::NorthWest),
        (1439.0, 0.0, ResizeDirection::NorthEast),
        (0.0, 959.0, ResizeDirection::SouthWest),
        (1439.0, 959.0, ResizeDirection::SouthEast),
        (0.0, 480.0, ResizeDirection::West),
        (1439.0, 480.0, ResizeDirection::East),
        (720.0, 0.0, ResizeDirection::North),
        (720.0, 959.0, ResizeDirection::South),
    ] {
        assert_eq!(
            resize_direction(PhysicalPosition::new(x, y), size, 6.0),
            Some(direction)
        );
    }
    assert_eq!(
        resize_direction(PhysicalPosition::new(8.0, 480.0), size, 12.0),
        Some(ResizeDirection::West)
    );
    for point in [
        PhysicalPosition::new(8.0, 480.0),
        PhysicalPosition::new(720.0, 480.0),
        PhysicalPosition::new(-1.0, 0.0),
        PhysicalPosition::new(1440.0, 480.0),
    ] {
        assert_eq!(resize_direction(point, size, 6.0), None);
    }
}

#[test]
#[ignore = "interactive Windows test: resize both windows, then F11 twice on the viewer"]
fn native_frameless_resize_and_f11_restore() -> Result<(), Failure> {
    let _ = tracing_subscriber::fmt().with_target(false).try_init();
    let mut builder = winit::event_loop::EventLoop::with_user_event();
    builder.with_any_thread(true);
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .with_winit_event_loop_builder(builder)
        .select()
        .context(PlatformSnafu)?;
    let ui = crate::gui::PhotoWindow::new().context(PlatformSnafu)?;
    let viewer = crate::gui::ContestantWindow::new().context(PlatformSnafu)?;
    bind(ui.window(), false);
    bind(viewer.window(), true);
    ui.show().context(PlatformSnafu)?;
    viewer.show().context(PlatformSnafu)?;
    let mut initial = None;
    let mut operator_resized = false;
    let mut viewer_resized = false;
    let mut fullscreen = None;
    let mut windowed_size = None;
    let started = Instant::now();
    let failed = Rc::new(RefCell::new(None));
    let failure = failed.clone();
    let operator = ui.as_weak();
    let display = viewer.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let result = (|| {
                ensure!(started.elapsed() < Duration::from_secs(90), InvariantSnafu);
                let ui = operator.upgrade().context(InvariantSnafu)?;
                let viewer = display.upgrade().context(InvariantSnafu)?;
                let a = ui
                    .window()
                    .with_winit_window(winit::window::Window::inner_size);
                let b = viewer
                    .window()
                    .with_winit_window(|w| (w.inner_size(), w.fullscreen().is_some()));
                let (Some(a), Some((b, full))) = (a, b) else {
                    return Ok(false);
                };
                let Some((original_a, original_b)) = initial else {
                    ui.window().with_winit_window(|w| {
                        w.set_title("Photo resize operator test");
                        w.set_outer_position(PhysicalPosition::new(0, 0));
                    });
                    viewer.window().with_winit_window(|w| {
                        w.set_title("Photo resize viewer test");
                        w.set_outer_position(PhysicalPosition::new(1440, 0));
                    });
                    initial = Some((a, b));
                    tracing::info!("native resize and F11 fixture ready");
                    return Ok(false);
                };
                operator_resized |= a != original_a;
                if !full && fullscreen.is_none() {
                    viewer_resized |= b != original_b;
                    windowed_size = Some(b);
                }
                if full {
                    fullscreen.get_or_insert(b);
                    ensure!(
                        viewer
                            .window()
                            .with_winit_window(|w| w.current_monitor().is_some_and(|m| {
                                super::super::monitors::DisplayPlacement::Windowed(m).healthy(w)
                            }))
                            == Some(true),
                        InvariantSnafu
                    );
                }
                let restored = fullscreen.is_some() && windowed_size == Some(b) && !full;
                if operator_resized && viewer_resized && restored {
                    tracing::info!(?a, ?b, "native resize and F11 roundtrip passed");
                    return Ok(true);
                }
                Ok(false)
            })();
            match result {
                Ok(false) => return,
                Ok(true) => {}
                Err(error) => {
                    *failure.borrow_mut() = Some(error);
                }
            }
            if let Err(error) = slint::quit_event_loop() {
                tracing::error!(%error, "cannot quit resize fixture");
            }
        },
    );
    let event = slint::run_event_loop().context(PlatformSnafu);
    timer.stop();
    event?;
    if let Some(error) = failed.borrow_mut().take() {
        return Err(error);
    }
    Ok(())
}
