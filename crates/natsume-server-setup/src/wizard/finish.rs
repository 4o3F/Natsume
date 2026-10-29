use super::{Host, InstallationSummary, Interaction, Step, Wizard, WizardFailure, WizardRollback};
use crate::{
    cli::ProbeCommand,
    deployment::{
        apply::{self, ApplyError, RollbackFailure},
        file::{self, DirectoryRemoval},
    },
    handoff,
    interaction::{
        Answer, PromptId,
        notice::{self, Notice},
    },
    probe::ProbeExit,
    process::BootstrapExit,
    service::ServiceAction,
};
use std::net::SocketAddr;

impl<I: Interaction, H: Host> Wizard<'_, I, H> {
    pub(super) fn apply(&mut self) -> Result<Step, WizardFailure> {
        self.applied = Some(apply::apply_plan(self.answers.deployment_plan())?);
        Ok(Step::Bootstrap)
    }

    pub(super) fn bootstrap(&mut self) -> Result<Step, WizardFailure> {
        let status = self.host.probe(ProbeCommand::Deployment)?;
        if status != ProbeExit::Passed {
            return super::ProbeSnafu { status }.fail();
        }
        match self.host.probe(ProbeCommand::Bootstrap)? {
            ProbeExit::Passed => {
                self.interaction.show(&Notice::BootstrapSkipped)?;
                return Ok(Step::Start);
            }
            ProbeExit::NotInitialized | ProbeExit::DatabaseMissing => {}
            status => return super::ProbeSnafu { status }.fail(),
        }
        self.interaction.show(&Notice::BootstrapPasswordPolicy)?;
        for attempt in 0..3 {
            let failure = match self.host.bootstrap() {
                Ok(BootstrapExit::Succeeded) => return Ok(Step::Start),
                Ok(BootstrapExit::Failed { code }) => super::BootstrapSnafu { code }.build(),
                Err(error) => error.into(),
            };
            let status = self.host.probe(ProbeCommand::Bootstrap)?;
            if status == ProbeExit::Passed {
                return Ok(Step::Start);
            }
            if !matches!(
                status,
                ProbeExit::NotInitialized | ProbeExit::DatabaseMissing
            ) {
                return super::ProbeSnafu { status }.fail();
            }
            if attempt == 2 {
                return Err(failure);
            }
            let id = PromptId::BootstrapRetry;
            if !matches!(
                self.interaction.confirm(id, notice::prompt(id), true)?,
                Answer::Value(true)
            ) {
                return Err(failure);
            }
        }
        unreachable!("bootstrap returns after at most three attempts")
    }

    pub(super) fn start(&mut self) -> Result<Step, WizardFailure> {
        self.host.systemctl(ServiceAction::DaemonReload)?;
        self.host.systemctl(ServiceAction::Enable)?;
        self.enabled = true;
        self.host.systemctl(ServiceAction::Start)?;
        let params = self.answers.parameters();
        let Some(ip) = params.server_ips().first() else {
            unreachable!("validated parameters contain an IP")
        };
        let address = SocketAddr::new(*ip, params.https_port());
        self.host
            .wait_for_health(&self.answers.control_ca().certificate, address)?;
        Ok(Step::Finish)
    }

    pub(super) fn finish(&mut self) -> InstallationSummary {
        let params = self.answers.parameters();
        let control = self.answers.control_ca();
        let origin = self.answers.origin_ca();
        let handoff = handoff::write_handoff(
            &self.context.target_root,
            &self.context.pki_dir,
            params,
            &control.certificate,
            &origin.certificate,
            self.host.now(),
            self.context.owners.root,
        );
        if let Err(error) = &handoff {
            tracing::error!(error = %snafu::Report::from_error(error), "Client handoff failed; deployment remains installed");
        }
        if let Ok(report) = &handoff {
            self.pki_files.extend_from_slice(&report.created_files);
            self.pki_directories
                .extend_from_slice(&report.created_directories);
        }
        let mut summary = InstallationSummary {
            addresses: params
                .server_ips()
                .iter()
                .map(|ip| SocketAddr::new(*ip, params.https_port()))
                .collect(),
            control_ca: (&control.certificate).into(),
            origin_ca: (&origin.certificate).into(),
            control_key: control.retention.clone(),
            origin_key: origin.retention.clone(),
            handoff,
            summary_display: Ok(()),
            pending_cleanup: Ok(()),
        };
        let notice = Notice::InstallationSummary {
            addresses: summary.addresses.clone(),
            control_ca: summary.control_ca.clone(),
            origin_ca: summary.origin_ca.clone(),
            control_key: summary.control_key.clone(),
            origin_key: summary.origin_key.clone(),
            handoff_directory: summary
                .handoff
                .as_ref()
                .ok()
                .map(|report| report.directory.clone()),
        };
        summary.summary_display = self.interaction.show(&notice);
        if let Err(error) = &summary.summary_display {
            tracing::error!(error = %snafu::Report::from_error(error), "cannot display summary; deployment remains installed");
        }
        if let Some(pending) = self.pending.take() {
            summary.pending_cleanup = pending.remove(&self.context.target_root).map(|_| ());
            if let Err(error) = &summary.pending_cleanup {
                tracing::error!(error = %snafu::Report::from_error(error), "cannot remove pending request; deployment remains installed");
            }
        }
        tracing::info!(summary = %notice, "Server installed successfully");
        summary
    }

    pub(super) fn cleanup(&mut self, source: &mut WizardFailure) -> Option<WizardRollback> {
        let system_writes = self.applied.is_some() || matches!(source, WizardFailure::Apply { .. });
        if !system_writes
            && self.pki_files.is_empty()
            && self.pki_directories.is_empty()
            && !self.revealed
        {
            return None;
        }
        let mut outcome = WizardRollback::default();
        if system_writes {
            if let Err(error) = self.host.systemctl(ServiceAction::Stop) {
                outcome.service_failures.push(error);
            }
            if self.enabled
                && let Err(error) = self.host.systemctl(ServiceAction::Disable)
            {
                outcome.service_failures.push(error);
            }
            if let Some(applied) = &self.applied {
                outcome.files = apply::rollback(applied);
            }
            if let WizardFailure::Apply {
                source:
                    ApplyError::EnsureDirectory { rollback, .. }
                    | ApplyError::CreateFile { rollback, .. },
            } = source
            {
                outcome.files = std::mem::take(rollback.as_mut());
            }
        }
        for created in self.pki_files.iter().rev() {
            match file::remove_created_file(created) {
                Ok(()) => outcome.files.removed.push(created.path.clone()),
                Err(source) => outcome.files.failures.push(RollbackFailure {
                    path: created.path.clone(),
                    source,
                }),
            }
        }
        for path in self.pki_directories.iter().rev() {
            match file::remove_created_directory(path) {
                Ok(DirectoryRemoval::Removed) => outcome.files.removed.push(path.clone()),
                Ok(DirectoryRemoval::KeptNotEmpty) => outcome.files.kept.push(path.clone()),
                Err(source) => outcome.files.failures.push(RollbackFailure {
                    path: path.clone(),
                    source,
                }),
            }
        }
        let notice = Notice::RollbackReport {
            removed: outcome.files.removed.clone(),
            kept: outcome.files.kept.clone(),
            failures: outcome
                .files
                .failures
                .iter()
                .map(|failure| failure.path.clone())
                .collect(),
            service_failures: outcome.service_failures.len(),
            revoke_revealed: self.revealed,
        };
        if let Err(error) = self.interaction.show(&notice) {
            tracing::error!(error = %snafu::Report::from_error(error), "cannot display rollback result");
        }
        tracing::warn!(report = %notice, "installation attempt rolled back");
        Some(outcome)
    }
}
