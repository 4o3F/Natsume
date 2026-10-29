use super::{CaRole, CaSource, Host, Interaction, Step, Wizard, WizardFailure, value};
use crate::{
    deployment::file,
    interaction::{
        PromptId,
        notice::{self, Notice},
    },
};

impl<I: Interaction, H: Host> Wizard<'_, I, H> {
    pub(super) fn generated(&self, role: CaRole) -> bool {
        let ca = match role {
            CaRole::Control => &self.answers.control,
            CaRole::Origin => &self.answers.origin,
        };
        ca.as_ref()
            .is_some_and(|ca| ca.source == CaSource::Generated)
    }

    pub(super) fn pki_directory(&mut self) -> Result<Step, WizardFailure> {
        self.interaction.show(&Notice::PkiDirectory {
            path: file::rooted_pki(&self.context.target_root, &self.context.pki_dir)?,
        })?;
        if self.generated(CaRole::Control) {
            Ok(Step::Passphrase(CaRole::Control))
        } else if self.generated(CaRole::Origin) {
            Ok(Step::Passphrase(CaRole::Origin))
        } else {
            Ok(Step::Validate)
        }
    }

    pub(super) fn passphrase(&mut self, role: CaRole) -> Result<Step, WizardFailure> {
        let id = PromptId::NewCaPassphrase { role };
        let passphrase = value(self.interaction.secret(id, notice::prompt(id), true)?)?;
        let ca = match role {
            CaRole::Control => &mut self.answers.control,
            CaRole::Origin => &mut self.answers.origin,
        };
        let Some(ca) = ca else {
            unreachable!("CA source precedes its export passphrase")
        };
        ca.passphrase = Some(passphrase);
        if role == CaRole::Control && self.generated(CaRole::Origin) {
            Ok(Step::Passphrase(CaRole::Origin))
        } else {
            Ok(Step::Validate)
        }
    }
}
