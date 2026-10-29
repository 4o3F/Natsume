use axum::extract::RawPathParams;
use uuid::Uuid;

use crate::{
    component::device::{DeviceId, EnrollmentReviewId},
    journal::event::types::{AuditRoute, AuditTarget, OperatorId, ReviewId},
};

pub(in crate::http) fn classify(path: &str) -> Option<AuditRoute> {
    let path = path.strip_prefix("/api/v2").unwrap_or(path);
    Some(match path {
        "/session" => AuditRoute::Session,
        "/devices/{device_id}" => AuditRoute::Device,
        "/devices/{device_id}/binding" => AuditRoute::DeviceBinding,
        "/enrollment-reviews/{review_id}/actions/approve" => AuditRoute::ReviewApprove,
        "/enrollment-reviews/{review_id}/actions/deny" => AuditRoute::ReviewDeny,
        "/imports" => AuditRoute::Imports,
        "/imports/{import_id}" => AuditRoute::Import,
        "/imports/{import_id}/actions/commit" => AuditRoute::ImportCommit,
        "/operator/accounts/{operator_id}" => AuditRoute::OperatorAccount,
        "/operator/accounts/{operator_id}/password-resets" => AuditRoute::PasswordResetIssue,
        "/operator/invitations" => AuditRoute::Invitations,
        "/operator/invitations/{invite_id}" => AuditRoute::Invitation,
        "/operator/invitations/{invite_id}/actions/regenerate" => AuditRoute::InvitationRegenerate,
        "/operator/password/change" => AuditRoute::PasswordChange,
        "/operator/password/reset" => AuditRoute::PasswordReset,
        "/operator/password/reset/inspect" => AuditRoute::PasswordResetInspect,
        "/operator/register" => AuditRoute::Registration,
        "/operator/register/inspect" => AuditRoute::RegistrationInspect,
        "/print/executions/{ready_sequence}/requeue" => AuditRoute::PrintRequeue,
        "/print/printers" => AuditRoute::Printers,
        "/print/printers/{printer_id}/binding" => AuditRoute::PrinterBinding,
        "/print/printers/{printer_id}/pairing-code" => AuditRoute::PrinterPairingCode,
        "/print/sources" => AuditRoute::PrintSources,
        "/print/sources/{source_id}" => AuditRoute::PrintSource,
        "/provisioning-window" => AuditRoute::Provisioning,
        "/target-submissions" => AuditRoute::TargetSubmission,
        _ => return None,
    })
}

pub(super) fn path_target(route: AuditRoute, params: &RawPathParams) -> Option<AuditTarget> {
    let value = |name| {
        params
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
    };
    match route {
        AuditRoute::Device | AuditRoute::DeviceBinding => {
            DeviceId::parse(value("device_id")?).map(AuditTarget::Device)
        }
        AuditRoute::ReviewApprove | AuditRoute::ReviewDeny => {
            EnrollmentReviewId::parse(value("review_id")?)
                .map(ReviewId::from_enrollment_review)
                .map(AuditTarget::Review)
        }
        AuditRoute::OperatorAccount | AuditRoute::PasswordResetIssue => {
            canonical_uuid(value("operator_id")?)
                .map(OperatorId::from_route_target)
                .map(AuditTarget::Operator)
        }
        AuditRoute::Invitation | AuditRoute::InvitationRegenerate => {
            canonical_uuid(value("invite_id")?).map(AuditTarget::Invitation)
        }
        AuditRoute::Import | AuditRoute::ImportCommit => {
            canonical_uuid(value("import_id")?).map(AuditTarget::Import)
        }
        AuditRoute::PrintSource => {
            canonical_uuid(value("source_id")?).map(AuditTarget::PrintSource)
        }
        AuditRoute::PrinterBinding | AuditRoute::PrinterPairingCode => {
            canonical_uuid(value("printer_id")?).map(AuditTarget::Printer)
        }
        AuditRoute::PrintRequeue => {
            let raw = value("ready_sequence")?;
            let sequence = raw.parse::<i64>().ok()?;
            (sequence > 0 && sequence.to_string() == raw)
                .then_some(AuditTarget::PrintExecution(sequence))
        }
        AuditRoute::Session
        | AuditRoute::Imports
        | AuditRoute::Invitations
        | AuditRoute::PasswordChange
        | AuditRoute::PasswordReset
        | AuditRoute::PasswordResetInspect
        | AuditRoute::Registration
        | AuditRoute::RegistrationInspect
        | AuditRoute::Printers
        | AuditRoute::PrintSources
        | AuditRoute::Provisioning
        | AuditRoute::TargetSubmission => None,
    }
}

pub(in crate::http) fn canonical_uuid(raw: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(raw).ok()?;
    (id.get_version_num() == 7 && id.to_string() == raw).then_some(id)
}
