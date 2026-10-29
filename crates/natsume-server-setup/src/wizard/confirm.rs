use super::{CaRole, Host, Interaction, Step, TlsMaterial, Wizard, WizardFailure, value};
use crate::{
    config,
    deployment::{
        file::{self, FileSpec},
        plan::{self, PlanMaterials},
    },
    handoff::CertificateSummary,
    interaction::{
        Choice, PromptId,
        notice::{self, Notice, PlannedCertificate},
    },
    pki::{
        generate::{self, GenerateError},
        validate,
    },
};
use natsume_server::{MaterialRole, config::ConfigError};

#[derive(Clone)]
enum Declined {
    Edit,
    Abort,
}

impl<I: Interaction, H: Host> Wizard<'_, I, H> {
    pub(super) fn validate(&mut self) -> Result<Step, WizardFailure> {
        if matches!(self.answers.tls, Some(TlsMaterial::Issue)) {
            let next = self.issue_leaf()?;
            if next != Step::Validate {
                return Ok(next);
            }
        }
        let params = self.answers.parameters();
        let text = config::render_server_config(params)?;
        let config = match config::validate_server_config(&text) {
            Ok(config) => config,
            Err(error) => {
                let field = match &error {
                    config::ConfigRenderError::Validation {
                        source: ConfigError::InvalidGatewayHostname,
                    } => super::params::Field::Hostname,
                    config::ConfigRenderError::Validation {
                        source: ConfigError::InvalidDomjudgeOrigin,
                    } => super::params::Field::Origin,
                    config::ConfigRenderError::Validation {
                        source: ConfigError::InvalidContestEnd,
                    } => super::params::Field::ContestEnd,
                    config::ConfigRenderError::Validation {
                        source: ConfigError::InvalidGatewayNotAfter,
                    } => super::params::Field::GatewayExpiry,
                    _ => return Err(error.into()),
                };
                self.rejected(&snafu::Report::from_error(&error).to_string())?;
                let retry = if self.pending.is_some() {
                    Step::SignedCertificate
                } else {
                    Step::Parameters(field)
                };
                self.reset(retry);
                return Ok(retry);
            }
        };
        if matches!(self.answers.tls, Some(TlsMaterial::Request { .. })) {
            return Ok(Step::Confirm);
        }
        let control = self.answers.control_ca();
        let Some(TlsMaterial::Identity {
            certificate: leaf,
            key: leaf_key,
        }) = &self.answers.tls
        else {
            unreachable!("issuance and import both produce a Server TLS identity");
        };
        let origin = self.answers.origin_ca();
        if let Err(error) = validate::validate_materials(
            &config,
            &control.certificate,
            leaf,
            leaf_key,
            &origin.certificate,
            origin.private_key(),
            params.server_ips(),
            self.host.now().into(),
        ) {
            let retry = match error.material_role() {
                Some(MaterialRole::ControlCa) if self.pending.is_none() => Step::ControlCa,
                Some(MaterialRole::OriginCa | MaterialRole::LocalOriginRoot) => Step::OriginCa,
                _ if self.pending.is_some() => Step::SignedCertificate,
                _ => Step::ServerTls,
            };
            self.rejected(&format!(
                "{}: {}",
                notice::certificate_policy(error.material_role(), error.certificate_rejection()),
                snafu::Report::from_error(&error)
            ))?;
            self.reset(retry);
            return Ok(retry);
        }
        let Some(service_owner) = self.service_owner else {
            unreachable!("preflight precedes planning")
        };
        self.answers.plan = Some(plan::build_plan(
            &self.context.target_root,
            super::Owners {
                root: self.context.root_owner,
                service: service_owner,
            },
            PlanMaterials {
                control_ca: &control.certificate,
                tls_leaf: leaf,
                tls_key: leaf_key,
                origin_ca: &origin.certificate,
                origin_key: origin.private_key(),
                config: &text,
            },
        )?);
        Ok(Step::Confirm)
    }

    fn issue_leaf(&mut self) -> Result<Step, WizardFailure> {
        let control = self.answers.control_ca();
        let params = self.answers.parameters();
        match generate::issue_server_leaf(
            &control.certificate,
            control.private_key(),
            params.server_ips(),
            params.gateway_not_after_timestamp(),
            self.host.now(),
        ) {
            Ok((certificate, key)) => {
                self.answers.tls = Some(TlsMaterial::Identity { certificate, key });
                Ok(Step::Validate)
            }
            Err(GenerateError::InsufficientValidity) => {
                self.rejected(
                    &snafu::Report::from_error(&GenerateError::InsufficientValidity).to_string(),
                )?;
                self.reset(Step::ServerTls);
                Ok(Step::ServerTls)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn confirm(&mut self) -> Result<Step, WizardFailure> {
        let params = self.answers.parameters();
        let control = self.answers.control_ca();
        let pending_only = matches!(self.answers.tls, Some(TlsMaterial::Request { .. }));
        let mut certificates = vec![PlannedCertificate {
            role: notice::CONTROL_NAME.into(),
            source: notice::source_label(control.source).into(),
            subject: control.certificate.subject(),
            identity: CertificateSummary::from(&control.certificate),
        }];
        if let Some(TlsMaterial::Identity { certificate, .. }) = &self.answers.tls {
            let Some(source) = self.answers.tls_source else {
                unreachable!("TLS source precedes confirmation")
            };
            certificates.push(PlannedCertificate {
                role: notice::SERVER_TLS_NAME.into(),
                source: notice::tls_source_label(source).into(),
                subject: certificate.subject(),
                identity: certificate.into(),
            });
        }
        if let Some(origin) = &self.answers.origin {
            certificates.push(PlannedCertificate {
                role: notice::ORIGIN_NAME.into(),
                source: notice::source_label(origin.source).into(),
                subject: origin.certificate.subject(),
                identity: (&origin.certificate).into(),
            });
        }
        let generated: Vec<_> = [CaRole::Control, CaRole::Origin]
            .into_iter()
            .filter(|role| self.generated(*role))
            .collect();
        let mut entries = if pending_only {
            vec![FileSpec {
                path: file::rooted_pki(&self.context.target_root, &self.context.pki_dir)?
                    .join("pending"),
                owner: self.context.root_owner,
                mode: 0o700,
            }]
        } else {
            self.answers.deployment_plan().entries().collect()
        };
        if !pending_only && self.generated(CaRole::Control) {
            entries.push(FileSpec {
                path: file::rooted_pki(&self.context.target_root, &self.context.pki_dir)?
                    .join("control-ca.crt"),
                owner: self.context.root_owner,
                mode: 0o644,
            });
        }
        self.interaction.show(&Notice::PlanSummary {
            parameters: params.clone(),
            certificates,
            entries,
            pending_only,
            generated,
        })?;
        let id = PromptId::PlanConfirmation;
        if value(self.interaction.confirm(id, notice::prompt(id), false)?)? {
            return Ok(if pending_only {
                Step::Apply
            } else {
                Step::Reveal
            });
        }
        let id = PromptId::PlanDeclined;
        let options = [
            Choice {
                value: Declined::Edit,
                label: if self.pending.is_some() {
                    notice::EDIT_ORIGIN_LABEL
                } else {
                    notice::EDIT_LABEL
                }
                .into(),
                unavailable: None,
            },
            Choice {
                value: Declined::Abort,
                label: notice::ABORT_LABEL.into(),
                unavailable: None,
            },
        ];
        match value(
            self.interaction
                .select(id, notice::prompt(id), &options, 0)?,
        )? {
            Declined::Edit => {
                let step = if self.pending.is_some() {
                    Step::OriginCa
                } else {
                    Step::Parameters(super::params::Field::Ips)
                };
                self.reset(step);
                Ok(step)
            }
            Declined::Abort => super::CancelledSnafu.fail(),
        }
    }
}
