use super::{CaRole, Host, Interaction, KeyRetention, Step, Wizard, WizardFailure};
use crate::{
    deployment::file::{self, FileSpec},
    interaction::{
        Answer, PromptId,
        notice::{self, Notice},
    },
    pki::generate,
};
use secrecy::SecretString;

impl<I: Interaction, H: Host> Wizard<'_, I, H> {
    pub(super) fn reveal(&mut self) -> Result<Step, WizardFailure> {
        for role in [CaRole::Control, CaRole::Origin] {
            if !self.generated(role) {
                continue;
            }
            self.interaction.show(&Notice::KeyRevealIntro { role })?;
            let ca = match role {
                CaRole::Control => &self.answers.control,
                CaRole::Origin => &self.answers.origin,
            };
            let Some(ca) = ca else {
                unreachable!("generated CA is present during export")
            };
            let Some(passphrase) = &ca.passphrase else {
                unreachable!("export passphrase precedes confirmation")
            };
            let pem = generate::encrypt_private_key_pem(ca.private_key(), passphrase)?;
            self.revealed = true;
            let result = self.retain(role, &pem);
            let clear = self.interaction.clear_scrollback();
            result?;
            clear?;
        }
        if self.generated(CaRole::Control) {
            let certificate = self.answers.control_ca().certificate.to_pem();
            self.save_pki("control-ca.crt", 0o644, certificate.as_bytes())?;
        }
        Ok(Step::Apply)
    }

    fn retain(&mut self, role: CaRole, pem: &str) -> Result<(), WizardFailure> {
        self.interaction
            .reveal_key(role, &SecretString::from(pem))?;
        let id = PromptId::KeyPersistence { role };
        let save = loop {
            if let Answer::Value(save) = self.interaction.confirm(id, notice::prompt(id), false)? {
                break save;
            }
        };
        let retention = if save {
            let name = match role {
                CaRole::Control => "control-ca-key.pem",
                CaRole::Origin => "origin-ca-key.pem",
            };
            KeyRetention::Saved(self.save_pki(name, 0o600, pem.as_bytes())?)
        } else {
            let id = PromptId::KeyRetentionConfirmation { role };
            let phrase = loop {
                if let Answer::Value(phrase) =
                    self.interaction
                        .phrase(id, notice::prompt(id), &["key saved", "abort"])?
                {
                    break phrase;
                }
            };
            if phrase != "key saved" {
                return super::CancelledSnafu.fail();
            }
            KeyRetention::ExternalCopy
        };
        let ca = match role {
            CaRole::Control => &mut self.answers.control,
            CaRole::Origin => &mut self.answers.origin,
        };
        let Some(ca) = ca else {
            unreachable!("exported CA remains selected")
        };
        ca.retention = retention;
        Ok(())
    }

    fn save_pki(
        &mut self,
        name: &str,
        mode: u32,
        contents: &[u8],
    ) -> Result<std::path::PathBuf, WizardFailure> {
        let directory = file::rooted_pki(&self.context.target_root, &self.context.pki_dir)?;
        self.pki_directories.extend(file::ensure_directory(
            &directory,
            self.context.owners.root,
            0o700,
        )?);
        file::check_directory(&directory, self.context.owners.root, 0o700)?;
        let path = directory.join(name);
        self.pki_files.push(file::create_new_file(
            &FileSpec {
                path: path.clone(),
                owner: self.context.owners.root,
                mode,
            },
            contents,
        )?);
        Ok(path)
    }
}
