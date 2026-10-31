//! One application owner serializes UI commands; `CaptureSession` owns ingestion and persistence.
use natsume_photo_client::{
    capture::GroupId,
    capture_session::{
        CaptureSession, DisplayId, HotFolderSnapshot, PresentationReceipt, PresentationRequest,
        PresentationStatus,
    },
    initialization::{Config, Initializer, VaultSecrets},
    workspace::{ConfirmedPhoto, Workspace},
};
use natsume_photo_protocol::PhotoTeam;
use snafu::{OptionExt as _, ResultExt as _, Snafu};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::images::{self, Pixels};

pub(super) enum Command {
    Open(PathBuf),
    Initialize {
        config: Config,
        code: Zeroizing<String>,
    },
    Session(PathBuf),
    Select(String),
    Start(String),
    Group {
        id: GroupId,
        action: GroupAction,
    },
    Confirm(PresentationRequest),
    Attach(Uuid),
    Detach(DisplayId),
    Rendered(PresentationReceipt),
    Cleanup,
    AcknowledgeMissing(Vec<PathBuf>),
    Export(PathBuf),
    PreviewFinal(String),
    Shutdown,
}

pub(super) enum GroupAction {
    Choose(Uuid),
    Automatic,
    Finish,
    Retake,
    Abandon,
    Assign(Uuid),
}

#[derive(Debug, Snafu)]
pub(super) enum Error {
    #[snafu(display("工作区校验失败：{source}"))]
    Workspace {
        source: natsume_photo_client::workspace::Error,
    },
    #[snafu(display("授权或初始化失败：{source}"))]
    Initialization {
        source: natsume_photo_client::initialization::Error,
    },
    #[snafu(display("拍摄操作未完成：{source}"))]
    Capture {
        source: natsume_photo_client::capture_session::Error,
    },
    #[snafu(display("后台任务未完成"))]
    Worker { source: tokio::task::JoinError },
    #[snafu(display("显示图片校验失败：{source}"))]
    Image { source: images::Error },
    #[snafu(display("请先准备工作区或开始拍摄组"))]
    NotReady,
    #[snafu(display("请先结束当前会话"))]
    ActiveSession,
    #[snafu(display("无法启动异步运行时"))]
    Runtime { source: std::io::Error },
    #[snafu(display("导出未完成：{source}"))]
    Export {
        source: natsume_photo_client::export::ExportError,
    },
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub(super) struct Thumbnail {
    pub id: Uuid,
    pub pixels: Arc<Pixels>,
    pub note: String,
}

#[derive(Clone)]
pub(super) struct Frame {
    pub request: PresentationRequest,
    pub pixels: Arc<Pixels>,
}

#[derive(Clone, Default)]
pub(super) struct Snapshot {
    pub root: Option<PathBuf>,
    pub teams: Arc<Vec<PhotoTeam>>,
    pub finals: Arc<Vec<ConfirmedPhoto>>,
    pub capture: Option<HotFolderSnapshot>,
    pub thumbnails: Vec<Thumbnail>,
    pub frame: Option<Frame>,
    pub logo: Option<Arc<Pixels>>,
    pub selected: Option<String>,
    pub status: String,
    pub problem: String,
    pub export_status: String,
    pub stopped: bool,
    pub display_generation: Option<Uuid>,
    pub final_preview: Option<Arc<Pixels>>,
    pub final_details: String,
}

#[derive(Default)]
struct Application {
    workspace: Option<Workspace>,
    session: Option<CaptureSession>,
    view: Snapshot,
    thumbnails: HashMap<Uuid, Thumbnail>,
}

pub(super) fn run(
    mut commands: mpsc::Receiver<Command>,
    updates: &watch::Sender<Snapshot>,
) -> Result<()> {
    let result = (|| {
        let runtime = tokio::runtime::Runtime::new().context(RuntimeSnafu)?;
        runtime.block_on(async {
            let mut app = Application::default();
            let mut tick = tokio::time::interval(Duration::from_millis(250));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let command = tokio::select! {
                    command = commands.recv() => command,
                    _ = tick.tick(), if app.session.is_some() => {
                        if let Err(error) = app.refresh().await {
                            app.failure(&error);
                        }
                        updates.send_replace(app.view.clone());
                        continue;
                    }
                };
                let Some(command) = command else {
                    break;
                };
                if matches!(command, Command::Shutdown) {
                    tracing::info!("Photo application is draining accepted work");
                    break;
                }
                app.view.problem.clear();
                let started = std::time::Instant::now();
                if let Err(error) = app.execute(command).await {
                    app.failure(&error);
                }
                tracing::debug!(
                    elapsed_ms = started.elapsed().as_millis(),
                    "Photo command completed"
                );
                if let Err(error) = app.refresh().await {
                    app.failure(&error);
                }
                updates.send_replace(app.view.clone());
            }
            if let Some(session) = app.session.take() {
                session.shutdown().await.context(CaptureSnafu)?;
            }
            Ok::<_, Error>(())
        })
    })();
    let mut final_view = updates.borrow().clone();
    if let Err(error) = &result {
        tracing::error!(%error, "Photo application stopped");
        final_view.problem = error.to_string();
    }
    final_view.stopped = true;
    tracing::info!("Photo application stopped");
    updates.send_replace(final_view);
    result
}

impl Application {
    fn failure(&mut self, error: &Error) {
        tracing::error!(%error, "Photo UI operation failed");
        self.view.problem = error.to_string();
        if matches!(error, Error::Image { .. }) {
            self.view.frame = None;
            if let Some(session) = &self.session
                && let Ok(snapshot) = session.snapshot()
            {
                self.view.capture = Some(snapshot);
            }
        }
    }

    async fn use_workspace(&mut self, workspace: Workspace) -> Result<()> {
        self.view.teams = Arc::new(
            workspace
                .roster()
                .await
                .context(WorkspaceSnafu)?
                .teams()
                .to_vec(),
        );
        self.view.finals = Arc::new(workspace.confirmed_photos().await.context(WorkspaceSnafu)?);
        self.view.root = Some(workspace.root().to_owned());
        self.workspace = Some(workspace);
        "工作区与授权已校验；日常操作全程离线".clone_into(&mut self.view.status);
        Ok(())
    }

    fn session(&self) -> Result<&CaptureSession> {
        self.session.as_ref().context(NotReadySnafu)
    }

    async fn execute(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Open(root) => {
                snafu::ensure!(self.session.is_none(), ActiveSessionSnafu);
                let workspace =
                    natsume_photo_client::initialization::open_authorized(&root, &VaultSecrets)
                        .await
                        .context(InitializationSnafu)?;
                self.use_workspace(workspace).await?;
            }
            Command::Initialize { config, code } => {
                self.initialize(config, code).await?;
            }
            Command::Session(folder) => {
                snafu::ensure!(self.session.is_none(), ActiveSessionSnafu);
                self.session = Some(
                    CaptureSession::prepare(
                        self.workspace.as_ref().context(NotReadySnafu)?,
                        &folder,
                    )
                    .await
                    .context(CaptureSnafu)?,
                );
                "会话已开始，请选择队伍并核对".clone_into(&mut self.view.status);
            }
            Command::Select(account) => {
                self.select(account).await?;
            }
            Command::Start(account) => {
                snafu::ensure!(self.view.selected.as_ref() == Some(&account), NotReadySnafu);
                self.session()?.start_group().await.context(CaptureSnafu)?;
            }
            Command::Attach(generation) => {
                self.session()?
                    .attach_display()
                    .await
                    .context(CaptureSnafu)?;
                self.view.display_generation = Some(generation);
            }
            Command::Detach(id) => {
                self.session()?
                    .detach_display(id)
                    .await
                    .context(CaptureSnafu)?;
                self.view.display_generation = None;
            }
            Command::Rendered(receipt) => self.rendered(receipt).await?,
            Command::Cleanup => {
                let report = self
                    .session()?
                    .retry_cleanup()
                    .await
                    .context(CaptureSnafu)?;
                self.view.status =
                    cleanup_status(report.pending_paths.len(), report.failures.len());
            }
            Command::AcknowledgeMissing(paths) => {
                for source in paths {
                    self.session()?
                        .acknowledge_missing(&source)
                        .await
                        .context(CaptureSnafu)?;
                }
            }
            Command::Confirm(request) => {
                self.confirm(request).await?;
            }
            Command::Export(target) => {
                self.export(target).await?;
            }
            Command::PreviewFinal(account) => {
                self.preview_final(&account).await?;
            }
            Command::Group { id: group, action } => {
                self.group(group, action).await?;
            }
            Command::Shutdown => return NotReadySnafu.fail(),
        }
        Ok(())
    }

    async fn rendered(&self, receipt: PresentationReceipt) -> Result<()> {
        let session = self.session()?;
        let snapshot = session.snapshot().context(CaptureSnafu)?;
        let (PresentationStatus::Pending(current) | PresentationStatus::Presented(current)) =
            &snapshot.presentation
        else {
            return Ok(());
        };
        if receipt.display_id != current.display_id()
            || receipt.revision != current.revision()
            || receipt.group_id != current.group_id()
            || receipt.account_id != current.account_id()
            || receipt.photo_id != current.photo_id()
            || receipt.image_sha256 != current.image().sha256()
        {
            // Queued callbacks for a superseded frame must never reach the backend.
            tracing::debug!("discarded superseded Photo render receipt");
            return Ok(());
        }
        session
            .acknowledge_presentation(receipt)
            .await
            .context(CaptureSnafu)
    }

    async fn initialize(&mut self, config: Config, code: Zeroizing<String>) -> Result<()> {
        snafu::ensure!(self.session.is_none(), ActiveSessionSnafu);
        let initializer =
            tokio::task::spawn_blocking(move || Initializer::open(config, VaultSecrets))
                .await
                .context(WorkerSnafu)?
                .context(InitializationSnafu)?;
        let workspace = initializer
            .run(if code.is_empty() {
                None
            } else {
                Some(code.as_str())
            })
            .await
            .context(InitializationSnafu)?;
        self.use_workspace(workspace).await?;
        Ok(())
    }

    async fn select(&mut self, account: String) -> Result<()> {
        self.session()?
            .select_team(&account)
            .await
            .context(CaptureSnafu)?;
        self.view.selected = Some(account.clone());
        self.view.logo = None;
        let workspace = self.workspace.as_ref().context(NotReadySnafu)?;
        let logo = workspace
            .logo_path(&account)
            .await
            .context(WorkspaceSnafu)?;
        self.view.logo = if let Some(path) = logo {
            Some(
                tokio::task::spawn_blocking(move || images::logo(&path))
                    .await
                    .context(WorkerSnafu)?
                    .context(ImageSnafu)?,
            )
        } else {
            None
        };
        self.view.selected = Some(account);
        Ok(())
    }

    async fn confirm(&mut self, request: PresentationRequest) -> Result<()> {
        self.view.final_preview = None;
        self.view.final_details.clear();
        let intent = self
            .session()?
            .prepare_confirmation(request)
            .await
            .context(CaptureSnafu)?;
        let saved = self
            .session()?
            .confirm_photo(intent)
            .await
            .context(CaptureSnafu)?;
        let finals = Arc::make_mut(&mut self.view.finals);
        finals.retain(|old| old.account_id() != saved.account_id());
        finals.push(saved);
        "成品已可靠保存；可以完成本队，或选择重拍".clone_into(&mut self.view.status);
        Ok(())
    }

    async fn export(&mut self, target: PathBuf) -> Result<()> {
        "本次导出尚未完成。".clone_into(&mut self.view.export_status);
        let workspace = self.workspace.as_ref().context(NotReadySnafu)?;
        match natsume_photo_client::export::export_results(workspace, &target, &VaultSecrets).await
        {
            Ok(package) => {
                self.view.export_status = format!(
                    "导出完成\n{}\n{} 张成品 · {} 字节\nSHA256 {}",
                    target.display(),
                    package.result_count,
                    package.bytes,
                    package.sha256
                );
            }
            Err(natsume_photo_client::export::ExportError::PublishedButUnrecorded {
                package,
                source,
            }) => {
                tracing::error!(%source, "ZIP published; completion record needs retry");
                self.view.export_status = format!(
                    "ZIP 已生成，但完成记录写入失败。\n{}\nSHA256 {}\n请使用同一路径重试，核实并补写记录。",
                    target.display(),
                    package.sha256
                );
            }
            Err(source) => return Err(Error::Export { source }),
        }
        Ok(())
    }

    async fn preview_final(&mut self, account: &str) -> Result<()> {
        self.view.final_preview = None;
        self.view.final_details.clear();
        let workspace = self.workspace.as_ref().context(NotReadySnafu)?;
        let photo = workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?
            .into_iter()
            .find(|photo| photo.account_id() == account)
            .context(NotReadySnafu)?;
        let root = workspace.root().to_owned();
        self.view.final_details = format!(
            "成品修订 {} · {} × {}\n{} 字节\nSHA256 {}",
            photo.revision(),
            photo.dimensions().width,
            photo.dimensions().height,
            photo.bytes(),
            photo.sha256()
        );
        self.view.final_preview = Some(
            tokio::task::spawn_blocking(move || images::confirmed(&root, &photo))
                .await
                .context(WorkerSnafu)?
                .context(ImageSnafu)?,
        );
        Ok(())
    }

    async fn group(&mut self, group: GroupId, action: GroupAction) -> Result<()> {
        let session = self.session()?;
        let snapshot = session.snapshot().context(CaptureSnafu)?;
        snafu::ensure!(snapshot.group_id == Some(group), NotReadySnafu);
        match action {
            GroupAction::Choose(id) => session
                .select_candidate(group, id)
                .await
                .context(CaptureSnafu)?,
            GroupAction::Automatic => session
                .use_latest_candidate(group)
                .await
                .context(CaptureSnafu)?,
            GroupAction::Assign(id) => session
                .assign_reviewed(id, group)
                .await
                .context(CaptureSnafu)?,
            GroupAction::Finish => {
                let report = session.finish_group(group).await.context(CaptureSnafu)?;
                self.view.status =
                    cleanup_status(report.pending_paths.len(), report.failures.len());
            }
            GroupAction::Retake => {
                if snapshot.confirmed.is_some() {
                    session.finish_group(group).await.context(CaptureSnafu)?;
                } else {
                    session.abandon_group(group).await.context(CaptureSnafu)?;
                }
                session.start_group().await.context(CaptureSnafu)?;
                "已开始本队重拍；原成品保留，直到新的确认成功".clone_into(&mut self.view.status);
            }
            GroupAction::Abandon => session.abandon_group(group).await.context(CaptureSnafu)?,
        }
        Ok(())
    }

    async fn refresh(&mut self) -> Result<()> {
        let Some(session) = &self.session else {
            return Ok(());
        };
        let snapshot = session.snapshot().context(CaptureSnafu)?;
        match &snapshot.presentation {
            PresentationStatus::Pending(request) | PresentationStatus::Presented(request) => {
                if self
                    .view
                    .frame
                    .as_ref()
                    .is_none_or(|frame| frame.request != *request)
                {
                    let image = request.image().clone();
                    // No UI work can run in this owner until validation completes. The queued UI
                    // update still checks the exact display/revision before installing its pixels.
                    let pixels = tokio::task::spawn_blocking(move || images::candidate(&image))
                        .await
                        .context(WorkerSnafu)?
                        .context(ImageSnafu)?;
                    self.view.frame = Some(Frame {
                        request: request.clone(),
                        pixels,
                    });
                }
            }
            PresentationStatus::Unavailable | PresentationStatus::Blank { .. } => {
                self.view.frame = None;
            }
        }
        let ids: Vec<_> = snapshot
            .candidates()
            .map(natsume_photo_client::capture_session::PhotoCandidate::photo_id)
            .collect();
        self.thumbnails.retain(|id, _| ids.contains(id));
        for candidate in snapshot.candidates() {
            if let std::collections::hash_map::Entry::Vacant(e) =
                self.thumbnails.entry(candidate.photo_id())
            {
                let image = candidate.thumbnail().clone();
                let pixels = tokio::task::spawn_blocking(move || images::candidate(&image))
                    .await
                    .context(WorkerSnafu)?
                    .context(ImageSnafu)?;
                e.insert(Thumbnail {
                    id: candidate.photo_id(),
                    pixels,
                    note: photo_time_label(candidate.captured_at()),
                });
                break;
            }
        }
        self.view.thumbnails = ids
            .iter()
            .filter_map(|id| self.thumbnails.get(id).cloned())
            .collect();
        let latest = session.snapshot().context(CaptureSnafu)?;
        // Camera ingestion continues during decode. Discard a completed load if its
        // revision is no longer the current target, before any UI can install it.
        if self.view.frame.as_ref().is_some_and(|frame| !matches!(&latest.presentation,
            PresentationStatus::Pending(request) | PresentationStatus::Presented(request) if *request == frame.request)) {
            self.view.frame = None;
        }
        self.view.capture = Some(latest);
        Ok(())
    }
}

fn cleanup_status(pending: usize, failed: usize) -> String {
    if pending == 0 && failed == 0 {
        "本队已结束，本地候选已清理；热文件夹原照片保留".to_owned()
    } else {
        format!("成品保留，候选清理仍需重试：{pending} 项待清理，{failed} 项失败")
    }
}

fn photo_time_label(time: natsume_photo_client::capture_session::PhotoTime) -> String {
    use natsume_photo_client::capture_session::PhotoTimeSource;
    let instant: chrono::DateTime<chrono::Local> = time.instant().into();
    let origin = match time.source() {
        PhotoTimeSource::ExifOriginal => "EXIF",
        PhotoTimeSource::FileModified => "修改时间*",
    };
    format!("{} {origin}", instant.format("%H:%M:%S"))
}

#[cfg(test)]
#[path = "../backend/tests/unit/ui_application.rs"]
mod tests;
