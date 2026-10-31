use super::*;

pub(super) fn bind(ui: &PhotoWindow, app: &Rc<Controller>) {
    let weak = Rc::downgrade(app);
    ui.window().on_close_requested(move || {
        if let Some(app) = weak.upgrade() {
            tracing::info!("operator requested Photo shutdown");
            app.quitting.set(true);
            app.clear_render();
            if let Some(ui) = app.ui.upgrade() {
                ui.set_busy(true);
                ui.set_status("正在等待已开始的文件与导出任务结束，请勿强制关闭…".into());
            }
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    bind_workspace(ui, app);
    bind_capture(ui, app);
    bind_files(ui, app);
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
            app_copy.refresh_monitors();
            let count = app_copy.monitors.borrow().len();
            let operator = usize::try_from(ui.get_operator_monitor()).ok();
            let display = usize::try_from(ui.get_display_monitor()).ok();
            if count == 0
                || count == 1 && !ui.get_single_confirm()
                || count > 1
                    && (operator == display
                        || operator.is_none_or(|index| index >= count)
                        || display.is_none_or(|index| index >= count))
            {
                ui.set_problem(
                    "显示器配置已变化。请重新选择两台显示器，或确认选择单屏启动。".into(),
                );
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
    let app_copy = app.clone();
    ui.on_assign_photo(move |id| {
        if let Ok(id) = uuid::Uuid::parse_str(&id) {
            app_copy.send_group(GroupAction::Assign(id));
        }
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
        if let Err(error) = app_copy.open_display()
            && let Some(ui) = app_copy.ui.upgrade()
        {
            ui.set_problem(error.to_string().into());
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
