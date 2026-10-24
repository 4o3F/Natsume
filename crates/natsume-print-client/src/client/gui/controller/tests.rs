use super::*;

mod presentation;
mod theme;
use slint::platform::{
    LayoutConstraints, Platform, PlatformError, Renderer, WindowAdapter, WindowProperties,
    software_renderer::{RepaintBufferType, SoftwareRenderer},
};
use snafu::Snafu;

#[derive(Default)]
struct HeadlessPlatform {
    constraints: Rc<Cell<Option<LayoutConstraints>>>,
    window: Rc<RefCell<Option<Rc<TestWindow>>>>,
}

struct TestWindow {
    window: slint::Window,
    renderer: SoftwareRenderer,
    constraints: Rc<Cell<Option<LayoutConstraints>>>,
    size: Cell<slint::PhysicalSize>,
}

impl WindowAdapter for TestWindow {
    fn window(&self) -> &slint::Window {
        &self.window
    }
    fn size(&self) -> slint::PhysicalSize {
        self.size.get()
    }
    fn set_size(&self, size: slint::WindowSize) {
        self.size.set(size.to_physical(self.window.scale_factor()));
        self.window
            .dispatch_event(slint::platform::WindowEvent::Resized {
                size: size.to_logical(self.window.scale_factor()),
            });
    }
    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }
    fn update_window_properties(&self, properties: WindowProperties<'_>) {
        self.constraints.set(Some(properties.layout_constraints()));
    }
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> std::result::Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = Rc::new_cyclic(|weak: &std::rc::Weak<TestWindow>| TestWindow {
            window: slint::Window::new(weak.clone()),
            renderer: SoftwareRenderer::new_with_repaint_buffer_type(RepaintBufferType::NewBuffer),
            constraints: self.constraints.clone(),
            size: Cell::new(slint::PhysicalSize::new(860, 600)),
        });
        *self.window.borrow_mut() = Some(window.clone());
        Ok(window)
    }
    fn run_event_loop(&self) -> std::result::Result<(), PlatformError> {
        Ok(())
    }
}

#[derive(Debug, Snafu)]
enum TestError {
    Platform {
        source: slint::platform::SetPlatformError,
    },
    Directory {
        source: std::io::Error,
    },
    Client {
        source: crate::client::error::ClientError,
    },
    Dimensions {
        source: std::num::TryFromIntError,
    },
}

#[test]
fn setup_has_no_execution_side_effects_and_shutdown_survives_a_full_command_channel()
-> std::result::Result<(), TestError> {
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        constraints: Rc::new(Cell::new(None)),
        ..HeadlessPlatform::default()
    }))
    .context(PlatformSnafu)?;
    let directory = tempfile::tempdir().context(DirectorySnafu)?;
    run(
        None,
        directory.path(),
        |_, _, _| panic!("setup must not start client execution"),
        || Ok(Vec::new()),
    )
    .context(ClientSnafu)?;
    assert!(!directory.path().join("client.toml").exists());
    assert!(!directory.path().join("state").exists());

    let ui = PrintWindow::new().context(GuiSnafu).context(ClientSnafu)?;
    let (sender, mut controls) = mpsc::channel(1);
    sender
        .try_send(Control::Pause)
        .map_err(|_| {
            AttentionSnafu {
                reason: "fixture channel unexpectedly full",
            }
            .build()
        })
        .context(ClientSnafu)?;
    let (snapshots, mut receiver) = watch::channel(Snapshot::default());
    let application = Application {
        controls: RefCell::new(Some(sender)),
        worker: RefCell::new(None),
        discovery: RefCell::new(None),
        quitting: Cell::new(true),
        snapshots,
    };
    application.poll(&ui, &mut receiver);
    assert!(matches!(controls.try_recv(), Ok(Control::Pause)));
    application.poll(&ui, &mut receiver);
    assert!(matches!(controls.try_recv(), Ok(Control::Stop)));
    application.shutdown().context(ClientSnafu)?;
    Ok(())
}

#[test]
fn requesting_exit_does_not_stop_work_until_the_confirmation_is_accepted()
-> std::result::Result<(), TestError> {
    slint::platform::set_platform(Box::<HeadlessPlatform>::default()).context(PlatformSnafu)?;
    let ui = PrintWindow::new().context(GuiSnafu).context(ClientSnafu)?;
    let (sender, mut commands) = mpsc::channel(2);
    let (snapshots, mut receiver) = watch::channel(Snapshot::default());
    let application = Rc::new(Application {
        controls: RefCell::new(Some(sender)),
        worker: RefCell::new(None),
        discovery: RefCell::new(None),
        quitting: Cell::new(false),
        snapshots,
    });
    bind_controls(
        &ui,
        &application,
        |_, _, _| panic!("exit confirmation must not start execution"),
        || Ok(Vec::new()),
    );
    ui.invoke_request_exit();
    assert!(ui.get_exit_confirm());
    assert!(!application.quitting.get());
    assert!(commands.try_recv().is_err());
    ui.set_exit_confirm(false);
    assert!(!application.quitting.get());
    ui.invoke_stop_client();
    assert!(ui.get_closing());
    assert!(application.quitting.get());
    application.poll(&ui, &mut receiver);
    assert!(matches!(commands.try_recv(), Ok(Control::Stop)));
    application.shutdown().context(ClientSnafu)?;
    Ok(())
}

#[test]
fn window_constraints_allow_resizing_both_dimensions() -> std::result::Result<(), TestError> {
    let constraints = Rc::new(Cell::new(None));
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        constraints: constraints.clone(),
        ..HeadlessPlatform::default()
    }))
    .context(PlatformSnafu)?;
    let ui = PrintWindow::new().context(GuiSnafu).context(ClientSnafu)?;
    ui.show().context(GuiSnafu).context(ClientSnafu)?;
    let constraints = constraints
        .get()
        .ok_or_else(|| {
            AttentionSnafu {
                reason: "window did not publish layout constraints",
            }
            .build()
        })
        .context(ClientSnafu)?;
    let minimum = constraints.min.unwrap_or_default();
    let maximum = constraints
        .max
        .unwrap_or(slint::LogicalSize::new(f32::MAX, f32::MAX));
    assert!(
        maximum.width > minimum.width,
        "window width must not be fixed"
    );
    assert!(
        maximum.height > minimum.height,
        "window height must not be fixed"
    );
    ui.hide().context(GuiSnafu).context(ClientSnafu)?;
    Ok(())
}
