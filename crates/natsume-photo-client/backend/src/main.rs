use std::{path::PathBuf, process::ExitCode};

use clap::Parser as _;
use natsume_photo_client::roster::Roster;
use natsume_photo_client::workspace::Workspace;
use snafu::{ResultExt as _, Snafu};

#[derive(clap::Parser)]
#[command(version, about = "Natsume Photo 后端开发入口（无 UI）")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// 检查本地非秘密名单；不会登记来源或保存工作区。
    CheckRoster { path: PathBuf },
    /// 从验证后的名单创建新工作区；不覆盖现有目录。
    CreateWorkspace {
        path: PathBuf,
        #[arg(long)]
        roster: PathBuf,
        /// 本地资源 JSON 清单，可省略。
        #[arg(long)]
        resources: Option<PathBuf>,
    },
    /// 打开工作区并验证版本、完成状态和本地名单。
    InspectWorkspace { path: PathBuf },
}

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("命令行参数无效，请使用 --help 查看用法"))]
    Arguments { source: clap::Error },
    #[snafu(display("名单检查失败：{source}"))]
    Roster {
        source: natsume_photo_client::roster::Error,
    },
    #[snafu(display("工作区操作失败：{source}"))]
    Workspace {
        source: natsume_photo_client::workspace::Error,
    },
}

fn execute() -> Result<(), Error> {
    let arguments = Arguments::try_parse().context(ArgumentsSnafu)?;
    match arguments.command {
        Command::CheckRoster { path } => {
            let roster = Roster::load(&path).context(RosterSnafu)?;
            tracing::info!(
                team_count = roster.teams().len(),
                "名单检查通过；未执行来源登记或持久化"
            );
        }
        Command::CreateWorkspace {
            path,
            roster,
            resources,
        } => {
            let roster = Roster::load(&roster).context(RosterSnafu)?;
            let _workspace = match resources {
                Some(manifest) => Workspace::create_with_resources(&path, &roster, &manifest),
                None => Workspace::create(&path, &roster),
            }
            .context(WorkspaceSnafu)?;
            tracing::info!(
                team_count = roster.teams().len(),
                "本地工作区创建完成；未执行来源登记"
            );
        }
        Command::InspectWorkspace { path } => {
            let mut workspace = Workspace::open(&path).context(WorkspaceSnafu)?;
            let roster = workspace.roster().context(WorkspaceSnafu)?;
            tracing::info!(
                team_count = roster.teams().len(),
                "本地工作区验证通过；未执行来源登记"
            );
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    tracing_subscriber::fmt().with_target(false).init();
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Arguments { source })
            if matches!(
                source.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            tracing::info!(help = %source, "使用说明");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "Photo Client 操作失败");
            ExitCode::FAILURE
        }
    }
}
