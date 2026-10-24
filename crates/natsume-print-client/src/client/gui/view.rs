use super::{PrintWindow, Screen};
use crate::client::application::{PairingStatus, Phase, Problem, Snapshot};
use slint::ComponentHandle as _;

pub(super) fn phase_note(snapshot: &Snapshot) -> &'static str {
    match snapshot.phase {
        Phase::Loading => "正在加载配置及核实本地记录。",
        Phase::Idle if snapshot.paused => "无进行中的任务。",
        Phase::Idle => "当前无进行中的任务。客户端先检查本地队列，再按 Server 规则领取。",
        Phase::Claiming => "正在核实领取请求；已发出的请求仍需确认，不能当作未领取。",
        Phase::Preparing => "正在下载并校验 PDF，尚未开始提交本次打印。",
        Phase::Submitting => {
            "正在提交原生打印；准确 Job ID 一经取得即保存，不以此证明页面已经全部提交。"
        }
        Phase::Observing => "正在按原队列、Job ID 和尝试标签观察作业。重启只恢复观察，不重复提交。",
        Phase::Reporting if snapshot.report_printed => {
            "打印系统报告完成（PRINTED），正在向 Server 报告。此状态不代表纸张已实际交付。"
        }
        Phase::Reporting => "正在向 Server 报告失败或未知结果，不代表打印成功，也不证明没有出纸。",
        Phase::Stopped => "客户端已停止。原有工作保留，不会通过清理记录重新开始。",
    }
}

fn notice(snapshot: &Snapshot) -> (&'static str, &'static str, i32) {
    match snapshot.problem {
        Problem::None => ("", "", 0),
        Problem::Configuration => (
            "无法读取或使用配置",
            "核对原配置、磁盘和读取权限，不要用新状态目录恢复旧任务。",
            2,
        ),
        Problem::Network => (
            "暂时无法完成 Server 请求",
            "检查本机网络及 Server。客户端保留当前工作并重试原请求，不盲目重复打印。",
            1,
        ),
        Problem::Printer => (
            "Windows 打印操作暂时不可用",
            "检查所选队列及设备状态。保留原记录，客户端继续按原任务规则处理。",
            1,
        ),
        Problem::Attention => (
            "需要人工核实，客户端已停止",
            "核对原尝试、Windows 原作业及打印结果，并联系管理员。不要删除记录、重置次数或重新配对来恢复任务。",
            2,
        ),
    }
}

pub(super) fn present(ui: &PrintWindow, snapshot: &Snapshot) {
    let screen = if snapshot.problem == Problem::Configuration {
        Screen::ConfigurationError
    } else if matches!(
        snapshot.pairing,
        PairingStatus::NeedsCode | PairingStatus::Confirming
    ) {
        Screen::Pairing
    } else if snapshot.pairing == PairingStatus::Confirmed
        || snapshot.task.is_some()
        || snapshot.problem == Problem::Attention
    {
        Screen::Dashboard
    } else {
        Screen::Setup
    };
    ui.set_screen(screen);
    ui.set_running(snapshot.running);
    ui.set_pairing_busy(snapshot.pairing == PairingStatus::Confirming);
    ui.set_paused(snapshot.paused);
    ui.set_paired(snapshot.pairing == PairingStatus::Confirmed);
    ui.set_can_control(
        snapshot.running
            && snapshot.pairing == PairingStatus::Confirmed
            && snapshot.problem != Problem::Attention,
    );
    ui.set_message(snapshot.message.as_str().into());
    ui.set_form_error(snapshot.problem != Problem::None);
    if !snapshot.origin.is_empty() {
        ui.set_server_url(snapshot.origin.as_str().into());
        ui.set_queue_name(snapshot.queue.as_str().into());
        ui.set_state_directory(snapshot.state_directory.as_str().into());
        ui.set_ca_path(snapshot.ca_path.as_str().into());
    }
    ui.set_task_present(snapshot.task.is_some());
    ui.set_task_sequence(
        snapshot
            .task
            .as_ref()
            .map_or_else(String::new, |task| task.sequence.to_string())
            .into(),
    );
    ui.set_task_attempt(
        snapshot
            .task
            .as_ref()
            .map_or_else(String::new, |task| task.attempt.to_string())
            .into(),
    );
    ui.set_job_id(
        snapshot
            .task
            .as_ref()
            .and_then(|task| task.job)
            .map_or_else(|| "未记录".to_owned(), |job| job.to_string())
            .into(),
    );
    ui.set_stage_index(match snapshot.phase {
        Phase::Preparing => 0,
        Phase::Submitting => 1,
        Phase::Observing => 2,
        Phase::Reporting => 3,
        _ => -1,
    });
    ui.set_phase_note(phase_note(snapshot).into());
    ui.set_claim_title(
        if !snapshot.running {
            "已停止"
        } else if snapshot.paused {
            "已暂停"
        } else {
            "已启用"
        }
        .into(),
    );
    ui.set_claim_description(
        if !snapshot.running {
            "客户端未运行，不会领取新任务。"
        } else if !snapshot.paused {
            "自动领取并处理任务。启用领取不代表打印机已就绪，请留意设备本身状态。"
        } else if snapshot.task.is_some() {
            "不再领取新任务；当前工作继续处理。"
        } else {
            "恢复后自动领取新任务，无需逐份确认。"
        }
        .into(),
    );
    let (title, next, tone) = notice(snapshot);
    ui.set_notice_title(title.into());
    ui.set_notice_detail(snapshot.message.as_str().into());
    ui.set_notice_next(next.into());
    ui.set_notice_tone(tone);
    ui.set_attention(snapshot.problem == Problem::Attention);
    ui.set_diagnostic(diagnostic(snapshot).into());
}

pub(super) fn operation_error(ui: &PrintWindow, message: &str) {
    ui.set_message(message.into());
    ui.set_form_error(true);
    if ui.get_screen() == Screen::Dashboard {
        toast(ui, &format!("操作未完成：{message}"));
    }
}

pub(super) fn toast(ui: &PrintWindow, message: &str) {
    ui.set_toast_message(message.into());
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(4), move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_toast_message(slint::SharedString::default());
        }
    });
}

fn diagnostic(snapshot: &Snapshot) -> String {
    let task = snapshot.task.as_ref().map_or_else(
        || "none".to_owned(),
        |task| {
            format!(
                "sequence={} attempt={} job={} label={}",
                task.sequence,
                task.attempt,
                task.job
                    .map_or_else(|| "not recorded".to_owned(), |job| job.to_string()),
                task.label
            )
        },
    );
    format!(
        "Natsume Print\nserver={}\nqueue={}\nphase={:?}\nproblem={:?}\npaused={}\ntask={}\ndetail={}",
        snapshot.origin,
        snapshot.queue,
        snapshot.phase,
        snapshot.problem,
        snapshot.paused,
        task,
        snapshot.message
    )
}
