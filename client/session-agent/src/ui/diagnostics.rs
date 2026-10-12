//! Display-only projection. File evidence never grants a lease or Binding input.

use natsume_local_control_api::{
    HardwareSourceStatus, SessionScreenKind, SessionUiSnapshot,
    diagnostics::{DiagnosticCode, DiagnosticReport, DiagnosticStage, DiagnosticStatus},
};
use slint::{ModelRc, VecModel};

use crate::diagnostics::{DiagnosticEvidence, ReportReadError, ReportSource, qr_image, qr_payload};

use super::{DiagnosticRow, DiagnosticTrackStep, SessionWindow};

const TRACK: [(DiagnosticStage, &str); 7] = [
    (DiagnosticStage::IdentityPreflight, "身份预检"),
    (DiagnosticStage::HardwareIdentity, "硬件身份"),
    (DiagnosticStage::ControlConfiguration, "控制配置"),
    (DiagnosticStage::Connecting, "连接"),
    (DiagnosticStage::PendingReview, "审核"),
    (DiagnosticStage::Handshake, "握手"),
    (DiagnosticStage::Active, "会话"),
];

pub(super) fn visible(snapshot: &SessionUiSnapshot, evidence: &DiagnosticEvidence) -> bool {
    if snapshot.team.is_some() || snapshot.screen != SessionScreenKind::Waiting {
        return false;
    }
    if snapshot.agent_lease_id.is_empty() {
        return true;
    }
    // A fresh live snapshot takes precedence over historical failure evidence.
    // In particular, a recovered unbound Waiting must be allowed to become ready
    // so that the Server can offer Binding. Last-failure cannot block this path.
    snapshot.offline
        && evidence.latest.as_ref().is_ok_and(|report| {
            report.failure.is_some()
                || matches!(
                    report.status,
                    DiagnosticStatus::WaitingForReview
                        | DiagnosticStatus::Denied
                        | DiagnosticStatus::Stopped
                )
        })
}

/// Applies diagnostic display properties only; never changes presentation authority.
///
/// The production caller decides visibility separately. Development probes may
/// project a synthetic report without contacting Device1.
pub fn apply(window: &SessionWindow, snapshot: &SessionUiSnapshot, evidence: &DiagnosticEvidence) {
    let selected = evidence.selected_report();
    let (title, subtitle, hint) = selected.map_or(
        (
            "本地服务不可用",
            "Local service unavailable",
            "尚未取得可用的本地诊断报告。",
        ),
        |(report, _)| report_copy(report),
    );
    window.set_diagnostic_title(title.into());
    window.set_diagnostic_title_size(match title.chars().count() {
        0..=6 => 128.0,
        7 => 110.0,
        _ => 96.0,
    });
    window.set_diagnostic_subtitle(subtitle.into());
    window.set_diagnostic_hint(format!("{hint}请联系现场工作人员扫描二维码。").into());
    window.set_diagnostic_status(
        selected
            .map_or("未知 / Unknown", |(report, _)| status_text(report.status))
            .into(),
    );
    let failure = selected.and_then(|(report, _)| report.failure.as_ref());
    window.set_diagnostic_error_code(
        failure
            .map_or_else(String::new, |failure| code_text(failure.code))
            .into(),
    );
    window.set_diagnostic_error_count(
        failure
            .filter(|failure| failure.consecutive_count > 1)
            .map_or_else(String::new, |failure| {
                format!("连续 {} 次（报告记录）", failure.consecutive_count)
            })
            .into(),
    );
    window.set_diagnostic_evidence_label(evidence_label(snapshot, evidence).into());
    let rows = selected.map_or_else(
        || {
            vec![row(
                "报告 / REPORT",
                read_error(evidence.latest.as_ref().err().copied()),
            )]
        },
        |(report, _)| report_rows(report),
    );
    window.set_diagnostic_rows(ModelRc::new(VecModel::from(rows)));
    window.set_diagnostic_track(ModelRc::new(VecModel::from(track(
        selected.map(|(report, _)| report),
    ))));
    window.set_diagnostic_footer(
        selected
            .map_or_else(
                || {
                    format!(
                        "agent {} · boot {} · 本地观察，不代表 Daemon 状态",
                        env!("CARGO_PKG_VERSION"),
                        short_boot(&snapshot.session.boot_id)
                    )
                },
                |(report, _)| {
                    format!(
                        "daemon {} · {} · boot {} · 采集于开机 {} · 阶段：{}",
                        report.daemon_version,
                        report.control_protocol,
                        report.boot_id.as_deref().map_or("未知", short_boot),
                        seconds(report.captured_at_boot_ms),
                        stage_text(report.stage),
                    )
                },
            )
            .into(),
    );
    let payload = selected.map_or_else(
        || Some(fallback_payload(snapshot, evidence)),
        |(report, _)| qr_payload(report).ok(),
    );
    let image = payload
        .as_deref()
        .and_then(|payload| qr_image(payload).ok());
    window.set_diagnostic_has_qr(image.is_some());
    window.set_diagnostic_qr(image.unwrap_or_default());
    window.set_diagnostic_qr_message("二维码暂不可用，请向工作人员提供错误码。".into());
}

fn evidence_label(snapshot: &SessionUiSnapshot, evidence: &DiagnosticEvidence) -> String {
    let local = if snapshot.agent_lease_id.is_empty() {
        "本地连接未建立"
    } else {
        "本地最近通信成功"
    };
    evidence.selected_report().map_or_else(
        || {
            format!(
                "{local} · 最近报告：{}",
                read_error(evidence.latest.as_ref().err().copied())
            )
        },
        |(_, source)| {
            format!(
                "{} · {local} · 非实时状态",
                match source {
                    ReportSource::Latest => "最近采集报告",
                    ReportSource::LastFailure => "历史失败记录",
                }
            )
        },
    )
}

fn fallback_payload(snapshot: &SessionUiSnapshot, evidence: &DiagnosticEvidence) -> String {
    serde_json::json!({
        "format_version": 1,
        "kind": "agent_observation",
        "agent_version": env!("CARGO_PKG_VERSION"),
        "boot_id": snapshot.session.boot_id,
        "device1": if snapshot.agent_lease_id.is_empty() { "not_connected" } else { "registered" },
        "latest_report": read_error_code(evidence.latest.as_ref().err().copied()),
        "last_failure_report": read_error_code(evidence.last_failure.as_ref().err().copied()),
    })
    .to_string()
}

fn row(label: &str, value: impl Into<String>) -> DiagnosticRow {
    DiagnosticRow {
        label: label.into(),
        value: value.into().into(),
    }
}

fn report_rows(report: &DiagnosticReport) -> Vec<DiagnosticRow> {
    let identity = &report.identity;
    let connection = &report.connection;
    let sources = identity.sources.as_ref().map_or_else(
        || "来源未取得".to_owned(),
        |sources| {
            format!(
                "UUID {} · 主板 {} · 磁盘 {}",
                source_text(sources.dmi_system_uuid),
                source_text(sources.dmi_board_serial),
                source_text(sources.first_disk_serial),
            )
        },
    );
    let count = identity
        .valid_source_count
        .map_or_else(|| "—/3".to_owned(), |count| format!("{count}/3"));
    let endpoint = match (&connection.endpoint_ip, connection.endpoint_port) {
        (Some(ip), Some(port)) if ip.contains(':') => format!("[{ip}]:{port}"),
        (Some(ip), Some(port)) => format!("{ip}:{port}"),
        _ => "未取得".to_owned(),
    };
    let network = &connection.network;
    let carrier = match network.carrier {
        natsume_local_control_api::diagnostics::DiagnosticLinkState::Unknown => "未知",
        natsume_local_control_api::diagnostics::DiagnosticLinkState::Up => "有链路",
        natsume_local_control_api::diagnostics::DiagnosticLinkState::Down => "无链路",
    };
    let attempt = match (connection.attempt, connection.retry_delay_ms) {
        (0, _) => "未记录连接尝试".to_owned(),
        (attempt, Some(delay)) => format!("第 {attempt} 次 · 记录等待 {}", seconds(Some(delay))),
        (attempt, None) => format!("第 {attempt} 次"),
    };
    let identifier = connection.device_id.as_ref().map_or_else(
        || {
            row(
                "硬件 ID / HARDWARE",
                identity.machine_hardware_id.as_deref().unwrap_or("未取得"),
            )
        },
        |id| row("设备 ID / DEVICE", id.as_str()),
    );
    vec![
        row("设备身份 / IDENTITY", format!("{count} · {sources}")),
        row("服务器 / ENDPOINT", endpoint),
        row(
            "网络观测 / NETWORK",
            format!(
                "{} · {} · {}",
                network.interface.as_deref().unwrap_or("接口未知"),
                carrier,
                connection.local_ip.as_deref().unwrap_or("本地 IP 未取得")
            ),
        ),
        row("尝试 / ATTEMPT", attempt),
        identifier,
    ]
}

fn track(report: Option<&DiagnosticReport>) -> Vec<DiagnosticTrackStep> {
    TRACK
        .iter()
        .enumerate()
        .map(|(index, (stage, label))| {
            let state = report.map_or(0, |report| {
                if report.stage == *stage {
                    2
                } else {
                    i32::from(report.last_completed_stage == Some(*stage))
                }
            });
            let detail = match state {
                2 => "报告阶段",
                1 => "曾完成",
                _ => "",
            };
            DiagnosticTrackStep {
                label: (*label).into(),
                position: format!("{:02}", index + 1).into(),
                state,
                detail: detail.into(),
            }
        })
        .collect()
}

fn short_boot(value: &str) -> &str {
    value.get(..8).unwrap_or("未知")
}

fn seconds(value: Option<u64>) -> String {
    value.map_or_else(
        || "未取得".to_owned(),
        |millis| format!("{}.{:01} s", millis / 1000, millis % 1000 / 100),
    )
}

fn source_text(source: HardwareSourceStatus) -> &'static str {
    match source {
        HardwareSourceStatus::Present => "有效",
        HardwareSourceStatus::Unavailable => "不可用",
        HardwareSourceStatus::Unsupported => "不支持",
        HardwareSourceStatus::PermissionDenied => "权限不足",
        HardwareSourceStatus::Malformed => "格式错误",
        HardwareSourceStatus::RejectedPlaceholder => "占位值",
    }
}

fn status_text(status: DiagnosticStatus) -> &'static str {
    match status {
        DiagnosticStatus::Starting => "启动（报告） / Starting",
        DiagnosticStatus::Retrying => "重试（报告） / Retrying",
        DiagnosticStatus::WaitingForReview => "等待审核（报告） / Waiting",
        DiagnosticStatus::Denied => "拒绝（报告） / Denied",
        DiagnosticStatus::Active => "会话曾建立 / Recorded active",
        DiagnosticStatus::Stopped => "停止（报告） / Stopped",
    }
}

fn stage_text(stage: DiagnosticStage) -> &'static str {
    match stage {
        DiagnosticStage::Starting => "开始启动",
        DiagnosticStage::SystemBus => "系统总线",
        DiagnosticStage::Helper => "Helper",
        DiagnosticStage::IdentityPreflight => "身份预检",
        DiagnosticStage::HardwareIdentity => "硬件身份",
        DiagnosticStage::LocalGateway => "本地网关",
        DiagnosticStage::ControlIdentity => "控制身份",
        DiagnosticStage::Reconciliation => "本地资源初始化",
        DiagnosticStage::ControlConfiguration => "控制配置",
        DiagnosticStage::Connecting => "建立连接",
        DiagnosticStage::Handshake => "握手",
        DiagnosticStage::PendingReview => "等待审核",
        DiagnosticStage::AuthorityInstallation => "安装注册身份",
        DiagnosticStage::Active => "控制会话",
    }
}

fn read_error(error: Option<ReportReadError>) -> &'static str {
    match error {
        None => "可读取",
        Some(ReportReadError::Missing) => "未取得",
        Some(ReportReadError::Unavailable) => "无法读取",
        Some(ReportReadError::Untrusted) => "归属或权限不可信",
        Some(ReportReadError::TooLarge) => "超过大小限制",
        Some(ReportReadError::Invalid) => "内容无效",
        Some(ReportReadError::UnsupportedVersion) => "格式版本不支持",
        Some(ReportReadError::WrongBoot) => "不属于本次启动",
    }
}

fn read_error_code(error: Option<ReportReadError>) -> &'static str {
    match error {
        None => "available",
        Some(ReportReadError::Missing) => "missing",
        Some(ReportReadError::Unavailable) => "unavailable",
        Some(ReportReadError::Untrusted) => "untrusted",
        Some(ReportReadError::TooLarge) => "too_large",
        Some(ReportReadError::Invalid) => "invalid",
        Some(ReportReadError::UnsupportedVersion) => "unsupported_version",
        Some(ReportReadError::WrongBoot) => "wrong_boot",
    }
}

fn code_text(code: DiagnosticCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn report_copy(report: &DiagnosticReport) -> (&'static str, &'static str, &'static str) {
    if let Some(failure) = &report.failure {
        return failure_copy(failure.code);
    }
    match report.status {
        DiagnosticStatus::WaitingForReview => (
            "等待审核",
            "Waiting for review",
            "报告记录注册申请等待审核。",
        ),
        DiagnosticStatus::Denied => (
            "注册被拒绝",
            "Enrollment denied",
            "报告记录当前注册请求被拒绝。",
        ),
        DiagnosticStatus::Active => (
            "会话曾建立",
            "Session established (recorded)",
            "报告记录会话已建立，不代表当前就绪。",
        ),
        DiagnosticStatus::Starting | DiagnosticStatus::Retrying => {
            ("启动记录", "Startup observation", "尚未取得具体失败分类。")
        }
        DiagnosticStatus::Stopped => (
            "停止记录",
            "Stopped observation",
            "报告记录初始化或控制流程停止。",
        ),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive fixed UI copy table, not protocol orchestration"
)]
fn failure_copy(code: DiagnosticCode) -> (&'static str, &'static str, &'static str) {
    use DiagnosticCode as Code;
    match code {
        Code::IdentityInsufficientSources => (
            "硬件身份不足",
            "Insufficient hardware identity",
            "可用硬件标识不足，无法生成设备身份。",
        ),
        Code::IdentityMismatch => (
            "设备身份不一致",
            "Hardware identity mismatch",
            "派生身份与本地记录不一致。",
        ),
        Code::IdentityRecordMissingOrCorrupt | Code::IdentityArtifactScanFailed => (
            "身份记录不可用",
            "Identity record unavailable",
            "本地身份记录或关联工件无法通过检查。",
        ),
        Code::IdentityUnsupported | Code::IdentityUnavailable => (
            "硬件身份不可用",
            "Hardware identity unavailable",
            "无法取得可验证的硬件身份。",
        ),
        Code::IdentityPersistenceFailed => (
            "身份保存失败",
            "Identity persistence failed",
            "无法确认设备身份已持久化。",
        ),
        Code::SystemBusUnavailable | Code::HelperUnavailable => (
            "本地组件不可用",
            "Local component unavailable",
            "无法完成本地组件通信或状态查询。",
        ),
        Code::SiteConfigurationInvalid
        | Code::EndpointConfigurationInvalid
        | Code::TrustRootInvalid
        | Code::TlsConfigurationFailed => (
            "部署配置无效",
            "Invalid deployment configuration",
            "部署配置或控制信任设置无法通过检查。",
        ),
        Code::LocalGatewayFailed => (
            "网关准备失败",
            "Gateway preparation failed",
            "无法完成本地网关配置同步。",
        ),
        Code::ReconciliationFailed => (
            "资源初始化失败",
            "Resource initialization failed",
            "无法完成本地资源初始化。",
        ),
        Code::ControlKeyInvalid
        | Code::ControlKeyEntropyUnavailable
        | Code::ControlKeyEncodingFailed
        | Code::ControlKeyPersistenceFailed
        | Code::ControlManifestInvalid
        | Code::ControlManifestSerializationFailed
        | Code::ControlManifestPersistenceFailed => (
            "控制身份不可用",
            "Control identity unavailable",
            "控制密钥或身份记录无法完成初始化。",
        ),
        Code::ReconnectEntropyUnavailable => (
            "重连准备失败",
            "Reconnect preparation failed",
            "无法生成安全的随机重连等待。",
        ),
        Code::LocalDeactivationFailed => (
            "安全收敛失败",
            "Local deactivation failed",
            "无法确认本地访问已阻断；诊断不证明安全状态。",
        ),
        Code::PresentationCacheFailed => (
            "展示缓存失败",
            "Presentation cache failed",
            "无法确认展示缓存已保存。",
        ),
        Code::AuthorityPersistenceFailed => (
            "注册身份保存失败",
            "Authority persistence failed",
            "无法确认注册身份已安装。",
        ),
        Code::ConnectTimeout => (
            "连接超时",
            "Connection timed out",
            "控制连接过程超时；不限定为 TCP 阶段。",
        ),
        Code::TcpConnectionRefused => (
            "无法连接服务器",
            "Connection refused",
            "报告记录服务器拒绝连接。",
        ),
        Code::TcpNetworkUnreachable => (
            "服务器网络不可达",
            "Network unreachable",
            "连接尝试报告网络不可达。",
        ),
        Code::TcpConnectionFailed => (
            "服务器连接失败",
            "TCP connection failed",
            "控制连接的 TCP 阶段失败。",
        ),
        Code::TlsCertificateInvalid => (
            "证书校验失败",
            "Certificate validation failed",
            "服务器证书未通过校验，未放宽信任检查。",
        ),
        Code::TlsHandshakeFailed => (
            "加密握手失败",
            "TLS handshake failed",
            "控制连接的 TLS 握手失败。",
        ),
        Code::WebsocketUpgradeFailed
        | Code::WebsocketSubprotocolMismatch
        | Code::WebsocketConnectionFailed => (
            "控制连接失败",
            "WebSocket connection failed",
            "控制 WebSocket 升级或子协议检查失败。",
        ),
        Code::HandshakeTimeout => ("握手超时", "Handshake timed out", "应用层身份握手超时。"),
        Code::HandshakeDisconnected => (
            "握手连接中断",
            "Handshake disconnected",
            "应用层身份握手期间连接结束。",
        ),
        Code::HandshakeInvalidMessage => (
            "握手消息无效",
            "Invalid handshake message",
            "收到的握手消息无法通过协议检查。",
        ),
        Code::HandshakeWriteFailed => (
            "握手发送失败",
            "Handshake write failed",
            "无法完成握手消息发送。",
        ),
        Code::EnrollmentDenied => (
            "注册被拒绝",
            "Enrollment denied",
            "本次注册请求被拒绝，不代表永久设备封禁。",
        ),
        Code::LocalAddressUnavailable => (
            "本机地址不可用",
            "Local address unavailable",
            "无法取得实际控制连接的本地地址。",
        ),
        Code::ActiveConnectionEnded => (
            "控制会话结束",
            "Control session ended",
            "报告记录控制会话结束，未确定具体原因。",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use natsume_local_control_api::{
        GraphicalSession,
        diagnostics::{ConnectionDiagnostic, DiagnosticFailure, IdentityDiagnostic},
    };

    fn snapshot() -> SessionUiSnapshot {
        SessionUiSnapshot {
            agent_lease_id: "current-lease".into(),
            session: GraphicalSession {
                logind_session_id: "waiting".into(),
                boot_id: "7138aed1-5921-4c4b-8e35-110806942def".into(),
            },
            ui_revision: 1,
            screen: SessionScreenKind::Waiting,
            team: None,
            logo_path: None,
            offline: true,
            binding_error_code: None,
            negotiation_id: None,
            submission_epoch: None,
        }
    }

    fn report() -> DiagnosticReport {
        DiagnosticReport {
            format_version: 1,
            boot_id: Some(snapshot().session.boot_id),
            daemon_instance_id: "11223344556677889900aabbccddeeff".into(),
            daemon_pid: 1234,
            daemon_version: "2.8.0".into(),
            control_protocol: "natsume.control.v3".into(),
            sequence: 1,
            captured_at_unix_ms: None,
            captured_at_boot_ms: Some(1234),
            stage: DiagnosticStage::Connecting,
            last_completed_stage: Some(DiagnosticStage::ControlConfiguration),
            status: DiagnosticStatus::Retrying,
            failure: Some(DiagnosticFailure {
                code: DiagnosticCode::TcpConnectionRefused,
                first_at_boot_ms: Some(1234),
                last_at_boot_ms: Some(1234),
                consecutive_count: 3,
            }),
            identity: IdentityDiagnostic::default(),
            connection: ConnectionDiagnostic::default(),
            events: Vec::new(),
            events_truncated: false,
        }
    }

    #[test]
    fn live_binding_and_recovery_take_precedence_over_old_failure() {
        let mut current = snapshot();
        let evidence = DiagnosticEvidence {
            latest: Ok(report()),
            last_failure: Ok(report()),
        };
        assert!(visible(&current, &evidence));
        current.screen = SessionScreenKind::BindingPrompt;
        assert!(!visible(&current, &evidence));
        current.screen = SessionScreenKind::BindingPending;
        assert!(!visible(&current, &evidence));
        current.screen = SessionScreenKind::Waiting;
        current.offline = false;
        assert!(!visible(&current, &evidence));
        let historical = DiagnosticEvidence {
            latest: Err(ReportReadError::Missing),
            last_failure: Ok(report()),
        };
        current.offline = true;
        assert!(!visible(&current, &historical));
        current.agent_lease_id.clear();
        assert!(visible(&current, &historical));
        let mut active = report();
        active.failure = None;
        active.status = DiagnosticStatus::Active;
        assert!(!visible(
            &snapshot(),
            &DiagnosticEvidence {
                latest: Ok(active),
                last_failure: Ok(report())
            }
        ));
    }

    #[test]
    fn bound_team_is_preserved_and_history_is_labelled_as_history() {
        let mut current = snapshot();
        current.agent_lease_id.clear();
        current.team = Some(natsume_local_control_api::WaitingTeam {
            binding_id: "binding".into(),
            account_id: "account".into(),
            seat_code: "A-108".into(),
            organization_id: "INST-001".into(),
            team_name_zh: "队伍".into(),
            team_name_en: String::new(),
            school_name_zh: "学校".into(),
            school_name_en: String::new(),
        });
        let evidence = DiagnosticEvidence {
            latest: Err(ReportReadError::Missing),
            last_failure: Ok(report()),
        };
        assert!(!visible(&current, &evidence));
        assert!(evidence_label(&current, &evidence).contains("历史失败记录"));
        assert!(evidence_label(&current, &evidence).contains("非实时状态"));
    }

    #[test]
    fn stage_track_marks_only_explicit_report_facts() {
        let report = report();
        let steps = track(Some(&report));
        assert_eq!(
            steps.iter().map(|step| step.state).collect::<Vec<_>>(),
            vec![0, 0, 1, 2, 0, 0, 0]
        );
        assert!(track(None).iter().all(|step| step.state == 0));
    }

    #[test]
    fn rows_keep_unknowns_and_identifier_types_distinct() {
        let mut report = report();
        report.connection.endpoint_ip = Some("2001:db8::1".into());
        report.connection.endpoint_port = Some(8443);
        report.connection.attempt = 3;
        report.connection.retry_delay_ms = Some(6123);
        report.identity.machine_hardware_id = Some("0c0fef01-1126-5297-a522-92cfe494ce48".into());
        let rows = report_rows(&report);
        assert!(rows[0].value.contains("—/3"));
        assert_eq!(rows[1].value, "[2001:db8::1]:8443");
        assert!(rows[3].value.contains("记录等待 6.1 s"));
        assert!(!rows[3].value.contains("后重试"));
        assert!(rows[4].label.contains("HARDWARE"));
        report.connection.device_id = Some("01951479-498a-7f3d-929d-fd118db192ba".into());
        assert!(report_rows(&report)[4].label.contains("DEVICE"));
        assert!(
            failure_copy(DiagnosticCode::ConnectTimeout)
                .2
                .contains("不限定为 TCP")
        );
        assert!(!status_text(DiagnosticStatus::Active).contains("正常"));
    }

    #[test]
    fn fallback_qr_contains_only_fixed_local_observation_and_no_daemon_report() {
        let mut snapshot = snapshot();
        snapshot.agent_lease_id.clear();
        let evidence = DiagnosticEvidence {
            latest: Err(ReportReadError::WrongBoot),
            last_failure: Err(ReportReadError::Invalid),
        };
        let value: serde_json::Value =
            serde_json::from_str(&fallback_payload(&snapshot, &evidence))
                .unwrap_or_else(|error| panic!("decode: {error}"));
        assert_eq!(value["kind"], "agent_observation");
        assert_eq!(value["latest_report"], "wrong_boot");
        assert_eq!(value["device1"], "not_connected");
        assert!(value.get("failure").is_none());
        assert!(value.get("identity").is_none());
    }
}
