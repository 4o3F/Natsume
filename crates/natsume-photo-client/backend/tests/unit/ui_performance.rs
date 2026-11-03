//! Opt-in native list responsiveness benchmark at the protocol's roster limit.
use super::*;
use slint::Model as _;
use snafu::{OptionExt as _, Snafu, ensure};
use winit::platform::windows::EventLoopBuilderExtWindows as _;

#[derive(Debug, Snafu)]
enum Failure {
    Platform { source: slint::PlatformError },
    Invariant,
}

#[test]
#[ignore = "requires an interactive Windows desktop and Skia"]
fn native_roster_search_and_page_switch_with_2000_teams() -> Result<(), Failure> {
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
    let (commands, _receive) = mpsc::channel(64);
    let app = live_tests::controller(&ui, commands);
    bind(&ui, &app);
    app.apply(Snapshot {
        root: Some(std::path::PathBuf::from("C:/PhotoBenchmark")),
        teams: std::sync::Arc::new(roster()),
        ..Snapshot::default()
    });
    ui.show().context(PlatformSnafu)?;
    let failed = Rc::new(RefCell::new(None));
    let failure = failed.clone();
    let weak = ui.as_weak();
    let mut step = 0;
    let mut started = Instant::now();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let result = (|| {
                let ui = weak.upgrade().context(InvariantSnafu)?;
                ensure!(started.elapsed() < Duration::from_secs(5), InvariantSnafu);
                tracing::info!(
                    step,
                    elapsed_ms = started.elapsed().as_millis(),
                    "native roster UI response"
                );
                started = Instant::now();
                match step {
                    0 => ui.set_page(3),
                    1 => ui.invoke_search_edited("测试大学1999".into()),
                    2 => {
                        ensure!(ui.get_teams().row_count() == 1, InvariantSnafu);
                        ui.invoke_search_edited("".into());
                        ui.set_page(4);
                    }
                    3 => {
                        ensure!(ui.get_teams().row_count() == 2000, InvariantSnafu);
                        ui.set_overview_search("University 1999".into());
                        ui.invoke_overview_filter_changed();
                    }
                    4 => {
                        ensure!(ui.get_overview_teams().row_count() == 1, InvariantSnafu);
                        ui.set_overview_search("".into());
                        ui.invoke_overview_filter_changed();
                        ui.set_page(3);
                    }
                    _ => return Ok(true),
                }
                step += 1;
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
                tracing::error!(%error, "cannot stop responsiveness fixture");
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

fn roster() -> Vec<natsume_photo_protocol::PhotoTeam> {
    (0..2000)
        .map(|index| natsume_photo_protocol::PhotoTeam {
            account_id: uuid::Uuid::from_u128(index + 1).to_string(),
            account: format!("Team{index:04}"),
            organization_id: i64::try_from(index).unwrap_or(0),
            school_name_zh: format!("测试大学{index:04}"),
            school_name_en: format!("University {index:04}"),
            team_name_zh: format!("测试队伍{index:04}"),
            team_name_en: format!("Team {index:04}"),
        })
        .collect()
}
