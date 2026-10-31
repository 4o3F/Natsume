use super::*;

impl Controller {
    pub(super) fn apply(self: &Rc<Self>, snapshot: Snapshot) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_busy(false);
        ui.set_problem(snapshot.problem.clone().into());
        if !snapshot.problem.is_empty() && snapshot.root.is_none() && ui.get_setup_step() == 2 {
            ui.set_setup_step(1);
        }
        ui.set_status(snapshot.status.clone().into());
        ui.set_export_status(snapshot.export_status.clone().into());
        if snapshot.final_preview.as_ref().map(std::sync::Arc::as_ptr)
            != self
                .final_preview
                .borrow()
                .as_ref()
                .map(std::sync::Arc::as_ptr)
        {
            ui.set_final_preview(
                snapshot
                    .final_preview
                    .as_ref()
                    .map_or_else(slint::Image::default, |pixels| pixels.image()),
            );
            self.final_preview
                .borrow_mut()
                .clone_from(&snapshot.final_preview);
        }
        ui.set_final_details(snapshot.final_details.clone().into());
        let newly_ready = !ui.get_ready() && snapshot.root.is_some();
        ui.set_ready(snapshot.root.is_some());
        if let Some(root) = &snapshot.root {
            ui.set_workspace_path(root.display().to_string().into());
        }
        if newly_ready {
            ui.set_page(1);
        }
        ui.set_team_count(i32::try_from(snapshot.teams.len()).unwrap_or(i32::MAX));
        ui.set_final_count(i32::try_from(snapshot.finals.len()).unwrap_or(i32::MAX));
        let new_session = !ui.get_session_active() && snapshot.capture.is_some();
        ui.set_session_active(snapshot.capture.is_some());
        let capture = snapshot.capture.as_ref();
        ui.set_group_active(capture.is_some_and(|capture| capture.group_id.is_some()));
        ui.set_confirmed(capture.is_some_and(|capture| capture.confirmed.is_some()));
        ui.set_pending(capture.map_or(0, |capture| {
            i32::try_from(capture.pending_tasks).unwrap_or(i32::MAX)
        }));
        ui.set_automatic(capture.is_none_or(|capture| {
            matches!(capture.selection, CandidateSelection::Automatic { .. })
        }));
        ui.set_display_online(
            self.display.borrow().as_ref().is_some_and(|display| {
                snapshot.display_generation == Some(display.generation) && !display.lost.get()
            }) && capture.and_then(display_id).is_some(),
        );
        ui.set_presented(
            ui.get_display_online()
                && snapshot.frame.as_ref().is_some_and(|frame| capture.is_some_and(|capture| {
                    matches!(&capture.presentation, PresentationStatus::Presented(request) if *request == frame.request)
                })),
        );
        ui.set_selected_id(snapshot.selected.clone().unwrap_or_default().into());
        Self::apply_team(&ui, &snapshot);
        self.apply_photos(&ui, &snapshot);
        apply_diagnostics(&ui, capture);
        self.apply_frame(&ui, &snapshot);
        let orphaned_display = if self
            .display
            .borrow()
            .as_ref()
            .is_none_or(|display| snapshot.display_generation != Some(display.generation))
        {
            capture.and_then(display_id)
        } else {
            None
        };
        *self.snapshot.borrow_mut() = snapshot;
        if let Some(id) = orphaned_display {
            self.send(Command::Detach(id), false);
        }
        self.filter(&ui.get_search());
        if new_session {
            ui.set_page(3);
            if let Err(error) = self.open_display() {
                ui.set_problem(error.to_string().into());
            }
        }
    }

    fn apply_team(ui: &PhotoWindow, snapshot: &Snapshot) {
        let team = snapshot
            .teams
            .iter()
            .find(|team| Some(&team.account_id) == snapshot.selected.as_ref());
        ui.set_account(team.map_or("", |team| team.account.as_str()).into());
        ui.set_school(team.map_or("", |team| team.school_name_zh.as_str()).into());
        ui.set_school_en(team.map_or("", |team| team.school_name_en.as_str()).into());
        ui.set_team(team.map_or("", |team| team.team_name_zh.as_str()).into());
        ui.set_team_en(team.map_or("", |team| team.team_name_en.as_str()).into());
        if let Some(logo) = &snapshot.logo {
            ui.set_logo(logo.image());
        } else {
            ui.set_logo(slint::Image::default());
        }
    }

    fn apply_photos(&self, ui: &PhotoWindow, snapshot: &Snapshot) {
        let capture = snapshot.capture.as_ref();
        let selected = capture.and_then(|capture| capture.selection.photo_id());
        let mut thumbnails = self.thumbnails.borrow_mut();
        thumbnails.retain(|id, _| snapshot.thumbnails.iter().any(|item| item.id == *id));
        let rows: Vec<_> = snapshot
            .thumbnails
            .iter()
            .enumerate()
            .map(|(index, item)| PhotoRow {
                id: item.id.to_string().into(),
                title: format!("#{:02}", index + 1).into(),
                note: item.note.clone().into(),
                thumbnail: thumbnails
                    .entry(item.id)
                    .or_insert_with(|| item.pixels.image())
                    .clone(),
                selected: Some(item.id) == selected,
            })
            .collect();
        ui.set_photo_label(
            rows.iter()
                .find(|row| row.selected)
                .map_or_else(|| "—".into(), |row| row.title.clone()),
        );
        ui.set_candidates(ModelRc::new(VecModel::from(rows)));
        let unassigned: Vec<_> = capture
            .into_iter()
            .flat_map(|capture| &capture.photos)
            .filter(|photo| photo.group_id.is_none())
            .map(|photo| PhotoRow {
                id: photo.id.to_string().into(),
                title: photo
                    .source
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
                    .into(),
                note: "未归组 · 请人工核对".into(),
                ..PhotoRow::default()
            })
            .collect();
        ui.set_unassigned(ModelRc::new(VecModel::from(unassigned)));
    }

    fn apply_frame(&self, ui: &PhotoWindow, snapshot: &Snapshot) {
        let frame = snapshot.frame.as_ref();
        let changed = self.installed.borrow().as_ref() != frame.map(|frame| &frame.request);
        if changed {
            self.clear_render();
            ui.set_verbal(false);
            ui.set_preview(frame.map_or_else(slint::Image::default, |frame| frame.pixels.image()));
            *self.installed.borrow_mut() = frame.map(|frame| frame.request.clone());
        }
        if let Some(display) = self.display.borrow().as_ref()
            && snapshot.display_generation == Some(display.generation)
            && !display.lost.get()
        {
            display
                .component
                .set_has_team(!ui.get_selected_id().is_empty());
            display.component.set_account(ui.get_account());
            display.component.set_school(ui.get_school());
            display.component.set_school_en(ui.get_school_en());
            display.component.set_team(ui.get_team());
            display.component.set_team_en(ui.get_team_en());
            display.component.set_logo(ui.get_logo());
            display.component.set_has_photo(frame.is_some());
            display.component.set_confirmed(ui.get_confirmed());
            if changed {
                display.component.set_photo(ui.get_preview());
                // This slot is installed AFTER the complete public projection. Only the actual
                // AfterRendering callback consumes it. Empty/loading windows never acknowledge.
                *display.pending.borrow_mut() = frame.map(|frame| frame.request.clone());
                display.component.window().request_redraw();
            }
        }
    }
    pub(super) fn filter(&self, query: &str) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let snapshot = self.snapshot.borrow();
        let query = query.trim().to_lowercase();
        let rows: Vec<_> = snapshot
            .teams
            .iter()
            .filter(|team| {
                [
                    team.account.as_str(),
                    &team.school_name_zh,
                    &team.school_name_en,
                    &team.team_name_zh,
                    &team.team_name_en,
                ]
                .iter()
                .any(|field| field.to_lowercase().contains(&query))
            })
            .map(|team| TeamRow {
                id: team.account_id.clone().into(),
                account: team.account.clone().into(),
                school: team.school_name_zh.clone().into(),
                school_en: team.school_name_en.clone().into(),
                name: team.team_name_zh.clone().into(),
                name_en: team.team_name_en.clone().into(),
                r#final: snapshot
                    .finals
                    .iter()
                    .any(|result| result.account_id() == team.account_id),
            })
            .collect();
        ui.set_teams(ModelRc::new(VecModel::from(rows)));
    }
}

fn apply_diagnostics(
    ui: &PhotoWindow,
    capture: Option<&natsume_photo_client::capture_session::HotFolderSnapshot>,
) {
    let mut diagnostics = Vec::new();
    if let Some(capture) = capture {
        if capture.paused {
            diagnostics.push(
                format!(
                    "收图已暂停：{:?}；请检查热文件夹、磁盘及相机传图状态",
                    capture.state
                )
                .into(),
            );
        }
        if capture.cleanup_pending > 0 || capture.cleanup_failures > 0 {
            diagnostics.push(
                format!(
                    "本地清理：{} 项待清理，{} 项失败。成品已保存，可重试清理。",
                    capture.cleanup_pending, capture.cleanup_failures
                )
                .into(),
            );
        }
        for missing in &capture.missing_sources {
            diagnostics.push(format!("源照片在归档前消失：{}", missing.source.display()).into());
        }
        for check in &capture.photo_checks {
            if let Err(error) = check.result() {
                diagnostics.push(format!("照片处理被拒绝：{error}").into());
            }
        }
    }
    ui.set_diagnostics(ModelRc::new(VecModel::from(diagnostics)));
}
