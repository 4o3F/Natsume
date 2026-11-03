use super::*;

#[test]
fn complete_code_paste_ignores_separator_and_uppercases() {
    assert_eq!(
        edit_pairing("", 0, "abcd-1234"),
        ("ABCD-1234".to_owned(), 7)
    );
    assert_eq!(
        edit_pairing("XXXXXXXX", 5, "abcd-1234"),
        ("ABCD-1234".to_owned(), 7)
    );
}

#[test]
fn typing_advances_and_replaces_existing_character() {
    assert_eq!(edit_pairing("", 0, "a"), ("A".to_owned(), 1));
    assert_eq!(edit_pairing("A", 0, "Ab"), ("B".to_owned(), 1));
    assert_eq!(
        edit_pairing("ABCD-1234", 7, "z"),
        ("ABCD-123Z".to_owned(), 7)
    );
}

#[test]
fn deleting_a_middle_character_preserves_other_positions() {
    assert_eq!(edit_pairing("ABCD-1234", 2, ""), ("AB D1234".to_owned(), 2));
    assert_eq!(
        edit_pairing("AB D1234", 2, "c"),
        ("ABCD-1234".to_owned(), 3)
    );
}

#[test]
fn short_paste_fills_from_current_slot_and_truncates_overflow() {
    assert_eq!(edit_pairing("AB", 2, "cd-12"), ("ABCD12".to_owned(), 6));
    assert_eq!(
        edit_pairing("ABCD123", 7, "yz"),
        ("ABCD-123Y".to_owned(), 7)
    );
}

#[derive(Debug, snafu::Snafu)]
enum Failure {
    Platform { source: slint::PlatformError },
    Image { source: image::ImageError },
    Invariant,
}

#[test]
#[ignore = "requires an interactive Windows desktop and Skia"]
fn native_pairing_keyboard_and_compact_header() -> Result<(), Failure> {
    use snafu::{OptionExt as _, ResultExt as _, ensure};
    use winit::platform::windows::EventLoopBuilderExtWindows as _;

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
    bind_pairing(&ui);
    ui.set_create_workspace(true);
    ui.set_setup_step(1);
    ui.show().context(PlatformSnafu)?;
    let display = ContestantWindow::new().context(PlatformSnafu)?;
    display.set_has_team(true);
    display.set_school("西北工业大学".into());
    display.set_school_en("Northwestern Polytechnical University".into());
    display.set_team("星辰大海".into());
    display.set_team_en("Beyond the Stars".into());
    display.set_account("Team0123".into());
    display.show().context(PlatformSnafu)?;
    let failed = Rc::new(RefCell::new(None));
    let failure = failed.clone();
    let weak = ui.as_weak();
    let weak_display = display.as_weak();
    let mut step = 0;
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(150),
        move || {
            let result = (|| {
                let ui = weak.upgrade().context(InvariantSnafu)?;
                if step == 0 {
                    ui.window()
                        .dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(true));
                    ui.set_pairing_focus(0);
                } else if step <= 8 {
                    let character = char::from(b"abcd1234"[step - 1]).to_string();
                    ui.window()
                        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                            text: character.clone().into(),
                        });
                    ui.window()
                        .dispatch_event(slint::platform::WindowEvent::KeyReleased {
                            text: character.into(),
                        });
                } else {
                    for (window, path) in [
                        (
                            ui.window(),
                            concat!(
                                env!("CARGO_MANIFEST_DIR"),
                                "/../../../target/photo-pairing-layout.png"
                            ),
                        ),
                        (
                            weak_display.upgrade().context(InvariantSnafu)?.window(),
                            concat!(
                                env!("CARGO_MANIFEST_DIR"),
                                "/../../../target/photo-compact-header.png"
                            ),
                        ),
                    ] {
                        let pixels = window.take_snapshot().context(PlatformSnafu)?;
                        image::save_buffer(
                            path,
                            pixels.as_bytes(),
                            pixels.width(),
                            pixels.height(),
                            image::ColorType::Rgba8,
                        )
                        .context(ImageSnafu)?;
                    }
                    ensure!(ui.get_pairing_code() == "ABCD-1234", InvariantSnafu);
                    return Ok(true);
                }
                step += 1;
                Ok(false)
            })();
            match result {
                Ok(false) => return,
                Ok(true) => {}
                Err(error) => *failure.borrow_mut() = Some(error),
            }
            if let Err(error) = slint::quit_event_loop() {
                tracing::error!(%error, "cannot stop pairing layout fixture");
            }
        },
    );
    slint::run_event_loop().context(PlatformSnafu)?;
    timer.stop();
    if let Some(error) = failed.borrow_mut().take() {
        return Err(error);
    }
    Ok(())
}
