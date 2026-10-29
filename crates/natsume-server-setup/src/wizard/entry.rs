use super::params::Field;
use super::{Host, Interaction, Step, Wizard, WizardFailure, WizardOutcome, value};
use crate::{
    deployment::plan,
    interaction::{
        PromptId,
        notice::{self, Notice},
    },
    pki::{import, pending::PendingRequest},
    wizard::{CaMaterial, CaSource, KeyRetention, TlsMaterial},
};

impl<I: Interaction, H: Host> Wizard<'_, I, H> {
    pub(super) fn execute(&mut self) -> Result<WizardOutcome, WizardFailure> {
        let mut report = self.host.preflight()?;
        self.check_interrupted()?;
        self.service_owner = Some(crate::deployment::file::Ownership {
            uid: report.account.uid,
            gid: report.account.gid,
        });
        let timezone = self.host.timezone();
        if timezone.name.is_none() {
            report
                .warnings
                .push(crate::preflight::PreflightWarning::TimezoneUnknown);
        }
        self.interaction
            .show(&Notice::PreflightTimezone { timezone })?;
        if !report.warnings.is_empty() {
            self.interaction.show(&Notice::PreflightWarnings {
                warnings: report.warnings,
            })?;
            let id = PromptId::ContinueAfterWarnings;
            if !value(self.interaction.confirm(id, notice::prompt(id), false)?)? {
                return super::CancelledSnafu.fail();
            }
        }
        let paths = plan::detect_existing_deployment(&self.context.target_root)?;
        if !paths.is_empty() {
            self.interaction
                .show(&Notice::ExistingDeployment { paths })?;
            return super::AlreadyDeployedSnafu.fail();
        }
        self.pending = PendingRequest::load(
            &self.context.target_root,
            &self.context.pki_dir,
            self.host.now(),
            self.context.root_owner,
        )?;
        let mut step = if let Some(pending) = &self.pending {
            self.interaction.show(&Notice::PendingRequestFound {
                path: pending.path().into(),
            })?;
            Step::SignedCertificate
        } else {
            Step::Parameters(Field::Ips)
        };
        loop {
            if step == Step::Finish {
                return Ok(WizardOutcome::Installed(self.finish()));
            }
            self.check_interrupted()?;
            if step == Step::Apply && matches!(self.answers.tls, Some(TlsMaterial::Request { .. }))
            {
                let request = self.publish_request()?;
                if self.host.interrupted() {
                    request.remove(&self.context.target_root)?;
                    return super::CancelledSnafu.fail();
                }
                return Ok(WizardOutcome::PendingSignature {
                    csr_path: request.csr_path(),
                });
            }
            let result = match step {
                Step::Parameters(field) => self.parameter(field),
                Step::SignedCertificate => self.signed_certificate(),
                Step::ControlCa => self.control(),
                Step::ServerTls => self.server_tls(),
                Step::OriginCa => self.origin(),
                Step::PkiDirectory => self.pki_directory(),
                Step::Passphrase(role) => self.passphrase(role),
                Step::Validate => self.validate(),
                Step::Confirm => self.confirm(),
                Step::Reveal => self.reveal(),
                Step::Apply => self.apply(),
                Step::Bootstrap => self.bootstrap(),
                Step::Start => self.start(),
                Step::Finish => unreachable!("finish is handled before advancing"),
            };
            match result {
                Ok(next) => {
                    self.check_interrupted()?;
                    step = next;
                }
                Err(WizardFailure::Back) if self.applied.is_none() && !self.revealed => {
                    step = self.back(step)?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn signed_certificate(&mut self) -> Result<Step, WizardFailure> {
        let certificate = self.certificate(PromptId::SignedCertificatePath)?;
        let Some(pending) = &self.pending else {
            unreachable!("resume starts with a pending request")
        };
        if let Err(error) = pending.accept_signed(&certificate) {
            self.rejected(&snafu::Report::from_error(&error).to_string())?;
            return Ok(Step::SignedCertificate);
        }
        self.answers.params = Some(pending.params().clone());
        self.answers.control = Some(CaMaterial {
            certificate: pending.control_ca().clone(),
            key: None,
            source: CaSource::CertificateOnly,
            passphrase: None,
            retention: KeyRetention::Offline,
        });
        self.answers.tls_source = Some(crate::pki::rules::ServerTlsOption::SigningRequest);
        self.answers.tls = Some(TlsMaterial::Identity {
            certificate,
            key: import::import_private_key(pending.key().pkcs8_der(), None)?,
        });
        Ok(Step::OriginCa)
    }

    pub(super) fn reset(&mut self, step: Step) {
        match step {
            Step::Parameters(field) => {
                self.answers.draft.clear_from(field);
                self.answers.params = None;
                self.reset(Step::ControlCa);
            }
            Step::ControlCa => {
                self.answers.control = None;
                self.reset(Step::ServerTls);
            }
            Step::SignedCertificate | Step::ServerTls => {
                self.answers.tls = None;
                self.answers.tls_source = None;
                self.reset(Step::OriginCa);
            }
            Step::OriginCa => {
                self.answers.origin = None;
                self.reset(Step::Validate);
            }
            Step::Passphrase(role) => {
                if let Some(ca) = match role {
                    super::CaRole::Control => &mut self.answers.control,
                    super::CaRole::Origin => &mut self.answers.origin,
                } {
                    ca.passphrase = None;
                }
                if role == super::CaRole::Control
                    && let Some(origin) = &mut self.answers.origin
                {
                    origin.passphrase = None;
                }
                self.reset(Step::Validate);
            }
            _ => {
                self.answers.plan = None;
            }
        }
    }

    fn back(&mut self, step: Step) -> Result<Step, WizardFailure> {
        let previous = match step {
            Step::Parameters(field) => {
                Step::Parameters(field.previous().ok_or(WizardFailure::Cancelled)?)
            }
            Step::ControlCa => Step::Parameters(Field::GatewayExpiryTime),
            Step::ServerTls => Step::ControlCa,
            Step::OriginCa if self.pending.is_some() => Step::SignedCertificate,
            Step::OriginCa => Step::ServerTls,
            Step::PkiDirectory if matches!(self.answers.tls, Some(TlsMaterial::Request { .. })) => {
                Step::ServerTls
            }
            Step::Passphrase(super::CaRole::Origin) if self.generated(super::CaRole::Control) => {
                Step::Passphrase(super::CaRole::Control)
            }
            Step::PkiDirectory | Step::Passphrase(_) => Step::OriginCa,
            Step::Confirm if self.pending.is_some() => Step::OriginCa,
            Step::Confirm => Step::Parameters(Field::Ips),
            _ => return super::CancelledSnafu.fail(),
        };
        self.reset(previous);
        Ok(previous)
    }

    fn publish_request(&mut self) -> Result<PendingRequest, WizardFailure> {
        let Some(TlsMaterial::Request { key, csr }) = &self.answers.tls else {
            unreachable!("pending publication requires a signing request");
        };
        let request = PendingRequest::create(
            &self.context.target_root,
            &self.context.pki_dir,
            self.answers.parameters(),
            &self.answers.control_ca().certificate,
            key,
            csr,
            self.context.root_owner,
        )?;
        if self.host.interrupted() {
            request.remove(&self.context.target_root)?;
            return super::CancelledSnafu.fail();
        }
        let result = self.interaction.show(&Notice::OfflineSigningInstructions {
            csr_path: request.csr_path(),
            control_ca_path: request.control_ca_path(),
        });
        if let Err(source) = result {
            if let Err(error) = request.remove(&self.context.target_root) {
                tracing::error!(error = %snafu::Report::from_error(error), "cannot remove pending request after interaction failure");
            }
            return Err(source.into());
        }
        if self.host.interrupted() {
            request.remove(&self.context.target_root)?;
            return super::CancelledSnafu.fail();
        }
        Ok(request)
    }
}
