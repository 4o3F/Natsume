use super::*;

pub(super) fn bind(ui: &PhotoWindow, app: &Rc<Controller>) {
    let weak = Rc::downgrade(app);
    ui.window().on_close_requested(move || {
        if let Some(app) = weak.upgrade() {
            app.request_shutdown();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    bind_workspace(ui, app);
    bind_capture(ui, app);
    bind_files(ui, app);
    bind_window_controls(ui, app);
}

fn bind_window_controls(ui: &PhotoWindow, app: &Rc<Controller>) {
    let weak = ui.as_weak();
    ui.on_drag_window(move || {
        // Let Slint finish handling the mouse press/capture before Windows
        // starts its native move loop. The physical button remains pressed.
        let weak = weak.clone();
        if let Err(error) = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade()
                && let Some(result) = ui
                    .window()
                    .with_winit_window(winit::window::Window::drag_window)
                && let Err(error) = result.context(WindowDragSnafu)
            {
                tracing::error!(%error, "cannot drag Photo window");
            }
        })
        .context(WindowEventLoopSnafu)
        {
            tracing::error!(%error, "cannot schedule Photo window drag");
        }
    });
    let weak = ui.as_weak();
    ui.on_minimize_window(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window()
                .with_winit_window(|window| window.set_minimized(true));
        }
    });
    let weak = ui.as_weak();
    ui.on_maximize_window(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window()
                .with_winit_window(|window| window.set_maximized(!window.is_maximized()));
        }
    });
    let weak = Rc::downgrade(app);
    ui.on_close_window(move || {
        if let Some(app) = weak.upgrade() {
            app.request_shutdown();
        }
    });
    let app = app.clone();
    ui.on_overview_filter_changed(move || app.filter_overview());
}

fn bind_workspace(ui: &PhotoWindow, app: &Rc<Controller>) {
    let app_copy = app.clone();
    ui.on_choose_path(move |kind| {
        choose_path(&app_copy, &kind);
    });
    let app_copy = app.clone();
    ui.on_open_workspace(move || {
        if let Some(ui) = app_copy.ui.upgrade() {
            app_copy.send(Command::Open(ui.get_workspace_path().as_str().into()), true);
        }
    });
    let app_copy = app.clone();
    ui.on_initialize(move || {
        if let Some(ui) = app_copy.ui.upgrade() {
            let code = zeroize::Zeroizing::new(ui.get_pairing_code().to_string());
            ui.set_pairing_code("".into());
            let ca = ui.get_ca_path();
            let config = natsume_photo_client::initialization::Config {
                server_url: ui.get_server_url().to_string(),
                workspace: ui.get_workspace_path().as_str().into(),
                ca_certificate: (ui.get_custom_ca() && !ca.is_empty()).then(|| ca.as_str().into()),
            };
            if !ui.get_busy() {
                ui.set_setup_step(2);
                app_copy.send(Command::Initialize { config, code }, true);
            }
        }
    });
    let app_copy = app.clone();
    ui.on_start_session(move || {
        if let Some(ui) = app_copy.ui.upgrade() {
            if let Err(error) = app_copy.select_screens() {
                app_copy.report_display_error(&error);
                return;
            }
            app_copy.send(Command::Session(ui.get_hot_folder().as_str().into()), true);
        }
    });
}

fn bind_capture(ui: &PhotoWindow, app: &Rc<Controller>) {
    let app_copy = app.clone();
    ui.on_select_team(move |id| {
        app_copy.send(Command::Select(id.to_string()), true);
    });
    let app_copy = app.clone();
    ui.on_start_group(move || {
        if let Some(ui) = app_copy.ui.upgrade() {
            app_copy.send(Command::Start(ui.get_selected_id().to_string()), true);
        }
    });
    let app_copy = app.clone();
    ui.on_choose_photo(move |id| {
        if let Ok(id) = uuid::Uuid::parse_str(&id) {
            app_copy.send_group(GroupAction::Choose(id));
        }
    });
    let app_copy = app.clone();
    ui.on_auto_select(move || {
        app_copy.send_group(GroupAction::Automatic);
    });
    let app_copy = app.clone();
    ui.on_confirm_photo(move || {
        app_copy.check_display();
        let request =
            app_copy
                .snapshot
                .borrow()
                .capture
                .as_ref()
                .and_then(|capture| match &capture.presentation {
                    PresentationStatus::Presented(request) => Some(request.clone()),
                    _ => None,
                });
        if app_copy.ui.upgrade().is_some_and(|ui| ui.get_verbal())
            && let Some(request) = request
        {
            app_copy.send(Command::Confirm(request), true);
        }
    });
    let app_copy = app.clone();
    ui.on_finish_group(move || {
        app_copy.send_group(GroupAction::Finish);
    });
    let app_copy = app.clone();
    ui.on_retake(move || {
        app_copy.send_group(GroupAction::Retake);
    });
    let app_copy = app.clone();
    ui.on_abandon(move || {
        app_copy.send_group(GroupAction::Abandon);
    });
}

fn bind_files(ui: &PhotoWindow, app: &Rc<Controller>) {
    let app_copy = app.clone();
    ui.on_preview_final(move |account| {
        app_copy.send(Command::PreviewFinal(account.to_string()), true);
    });
    let app_copy = app.clone();
    ui.on_retry_cleanup(move || {
        app_copy.send(Command::Cleanup, true);
    });
    let app_copy = app.clone();
    ui.on_acknowledge_missing(move || {
        let paths = app_copy
            .snapshot
            .borrow()
            .capture
            .as_ref()
            .map(|capture| {
                capture
                    .missing_sources
                    .iter()
                    .map(|missing| missing.source.clone())
                    .collect()
            })
            .unwrap_or_default();
        app_copy.send(Command::AcknowledgeMissing(paths), true);
    });
    let app_copy = app.clone();
    ui.on_export_results(move || {
        if let Some(ui) = app_copy.ui.upgrade() {
            app_copy.send(Command::Export(ui.get_export_path().as_str().into()), true);
        }
    });
    let app_copy = app.clone();
    ui.on_reopen_display(move || {
        if let Err(error) = app_copy
            .select_screens()
            .and_then(|()| app_copy.open_display())
        {
            app_copy.report_display_error(&error);
        }
    });
    let app_copy = app.clone();
    ui.on_search_edited(move |query| {
        app_copy.filter(&query);
    });
}

fn choose_path(app: &Controller, kind: &str) {
    let Some(ui) = app.ui.upgrade() else {
        return;
    };
    let dialog = rfd::FileDialog::new();
    let path = match kind {
        "ca" => dialog
            .add_filter("CA certificate", &["pem", "crt", "cer"])
            .pick_file(),
        "export" => dialog
            .add_filter("Photo ZIP package", &["zip"])
            .set_file_name("natsume-photos.zip")
            .save_file(),
        _ => dialog.pick_folder(),
    };
    if let Some(path) = path {
        let path = if kind == "new-workspace" {
            path.join("photo-workspace")
        } else {
            path
        };
        let path = path.display().to_string().into();
        match kind {
            "ca" => ui.set_ca_path(path),
            "hot" => ui.set_hot_folder(path),
            "export" => ui.set_export_path(path),
            _ => ui.set_workspace_path(path),
        }
    }
}
