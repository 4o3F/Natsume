use super::*;
use crate::client::{
    application::Task,
    gui::{Screen, code, view},
};

fn capture(window: &TestWindow, name: &str) -> std::result::Result<(), TestError> {
    let size = window.size();
    let width = usize::try_from(size.width).context(DimensionsSnafu)?;
    let height = usize::try_from(size.height).context(DimensionsSnafu)?;
    let mut pixels = vec![slint::Rgb8Pixel::default(); width * height];
    window.renderer.render(&mut pixels, width);
    if name == "main-narrow" {
        assert_eq!(
            pixels[(height - 53) * width + width - 100],
            slint::Rgb8Pixel {
                r: 255,
                g: 255,
                b: 255
            },
            "scrolled controls must not paint over the fixed footer"
        );
    }
    let mut image = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in pixels {
        image.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    let directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/print-gui-design-review");
    std::fs::create_dir_all(&directory).context(DirectorySnafu)?;
    std::fs::write(directory.join(format!("{name}.ppm")), image).context(DirectorySnafu)?;
    Ok(())
}

fn base() -> Snapshot {
    Snapshot {
        origin: "https://print.natsume.contest-site.example.org:8443".to_owned(),
        queue: "Hall-B 二层东侧 · HP LaserJet Enterprise M607 PCL-6 (V4) 备用队列".to_owned(),
        state_directory: "C:\\Users\\Contest\\AppData\\Local\\NatsumePrint\\state".to_owned(),
        running: true,
        pairing: PairingStatus::Confirmed,
        ..Snapshot::default()
    }
}

#[test]
fn prototype_views_render_real_states_without_inventing_success_or_online_status()
-> std::result::Result<(), TestError> {
    let window = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        window: window.clone(),
        ..HeadlessPlatform::default()
    }))
    .context(PlatformSnafu)?;
    let ui = PrintWindow::new().context(GuiSnafu).context(ClientSnafu)?;
    code::bind(&ui);
    ui.set_config_path("C:\\Users\\Contest\\AppData\\Local\\NatsumePrint\\client.toml".into());
    ui.show().context(GuiSnafu).context(ClientSnafu)?;
    let window = window
        .borrow()
        .clone()
        .ok_or_else(|| {
            AttentionSnafu {
                reason: "no test window",
            }
            .build()
        })
        .context(ClientSnafu)?;
    capture(&window, "setup")?;
    ui.set_new_deployment(true);
    capture(&window, "new-deployment")?;
    let mut state = base();
    state.paused = true;
    state.pairing = PairingStatus::NeedsCode;
    view::present(&ui, &state);
    assert_eq!(ui.get_screen(), Screen::Pairing);
    ui.invoke_code_edited(0, "abcd-2345".into());
    assert_eq!(ui.get_pairing_code().as_str(), "ABCD2345");
    assert_eq!(ui.get_code_focus_index(), 7);
    assert!(!ui.get_diagnostic().contains("ABCD2345"));
    ui.invoke_code_backspace(7);
    assert!(ui.get_pairing_code().len() < 8);
    code::clear(&ui);
    capture(&window, "pairing")?;
    state = base();
    view::present(&ui, &state);
    assert_eq!(ui.get_screen(), Screen::Dashboard);
    assert_eq!(ui.get_claim_title().as_str(), "已启用");
    assert!(ui.get_can_control());
    capture(&window, "main-idle")?;
    state.paused = true;
    state.task = Some(Task {
        sequence: 1042,
        attempt: 1,
        job: Some(87),
        label: "Natsume/018f0000-0000-7000-8000-000000000042".to_owned(),
    });
    for (phase, index) in [
        (Phase::Preparing, 0),
        (Phase::Submitting, 1),
        (Phase::Observing, 2),
        (Phase::Reporting, 3),
    ] {
        state.phase = phase;
        view::present(&ui, &state);
        assert_eq!(ui.get_stage_index(), index);
        assert_eq!(ui.get_claim_title().as_str(), "已暂停");
        assert!(ui.get_task_present());
    }
    assert!(!ui.get_phase_note().contains("PRINTED"));
    state.report_printed = true;
    view::present(&ui, &state);
    assert!(ui.get_phase_note().contains("不代表纸张已实际交付"));
    capture(&window, "main-reporting")?;
    state.problem = Problem::Network;
    state.message = "print client request failed".to_owned();
    view::present(&ui, &state);
    assert!(ui.get_can_control());
    capture(&window, "network-error")?;
    state.problem = Problem::Attention;
    state.running = false;
    if let Some(task) = state.task.as_mut() {
        task.job = None;
    }
    view::present(&ui, &state);
    assert!(!ui.get_can_control());
    assert_eq!(ui.get_job_id().as_str(), "未记录");
    assert!(!ui.get_diagnostic().contains(&state.state_directory));
    capture(&window, "attention")?;
    ui.set_settings_open(true);
    capture(&window, "settings")?;
    ui.set_settings_open(false);
    ui.set_exit_confirm(true);
    capture(&window, "exit-confirmation")?;
    ui.set_exit_confirm(false);
    ui.set_closing(true);
    capture(&window, "exiting")?;
    ui.set_closing(false);
    state.problem = Problem::Configuration;
    view::present(&ui, &state);
    assert_eq!(ui.get_screen(), Screen::ConfigurationError);
    capture(&window, "configuration-error")?;
    ui.set_screen(Screen::Dashboard);
    ui.window().set_size(slint::LogicalSize::new(420.0, 420.0));
    capture(&window, "main-narrow")?;
    ui.hide().context(GuiSnafu).context(ClientSnafu)?;
    Ok(())
}
