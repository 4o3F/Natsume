use super::{
    CaMaterial, CaSource, Host, Interaction, KeyRetention, Step, TlsMaterial, Wizard,
    WizardFailure, value,
};
use crate::{
    interaction::{PromptId, notice},
    pki::{
        Certificate, KeyAlgorithm, PrivateKey, generate,
        import::{self, ImportError},
        rules::{self, ServerTlsOption},
    },
};
use std::path::Path;

impl<I: Interaction, H: Host> Wizard<'_, I, H> {
    pub(super) fn control(&mut self) -> Result<Step, WizardFailure> {
        let id = PromptId::ControlCaSource;
        let choice = value(self.interaction.select(
            id,
            notice::prompt(id),
            &notice::control_choices(),
            0,
        )?)?;
        let (certificate, key) = match choice {
            CaSource::Generated => {
                let (certificate, key) = generate::generate_ca(
                    notice::CONTROL_NAME,
                    notice::ORGANIZATION,
                    self.host.now(),
                )?;
                (certificate, Some(key))
            }
            CaSource::ImportedWithKey | CaSource::CertificateOnly => {
                let certificate = self.certificate(PromptId::ControlCaCertificatePath)?;
                if !ca_usable(&certificate) {
                    self.rejected(notice::NOT_CA)?;
                    return Ok(Step::ControlCa);
                }
                if certificate.not_before() > self.host.now()
                    || certificate.not_after()
                        < self.answers.parameters().gateway_not_after_timestamp()
                {
                    self.rejected(notice::CA_VALIDITY)?;
                    return Ok(Step::ControlCa);
                }
                let key = if choice == CaSource::ImportedWithKey {
                    Some(self.private_key(
                        PromptId::ControlCaKeyPath,
                        PromptId::ControlCaKeyPassphrase,
                    )?)
                } else {
                    None
                };
                if key.as_ref().is_some_and(|key| !key.matches(&certificate)) {
                    self.rejected(notice::KEY_MISMATCH)?;
                    return Ok(Step::ControlCa);
                }
                (certificate, key)
            }
        };
        self.answers.control = Some(CaMaterial {
            certificate,
            key,
            source: choice,
            passphrase: None,
            retention: match choice {
                CaSource::Generated => KeyRetention::ExternalCopy,
                CaSource::ImportedWithKey => KeyRetention::Imported,
                CaSource::CertificateOnly => KeyRetention::Offline,
            },
        });
        Ok(Step::ServerTls)
    }

    pub(super) fn server_tls(&mut self) -> Result<Step, WizardFailure> {
        let control = self.answers.control_ca();
        let control_choice = match control.source {
            CaSource::Generated => rules::ControlCaChoice::Generated,
            CaSource::ImportedWithKey => rules::ControlCaChoice::ImportedWithKey,
            CaSource::CertificateOnly => rules::ControlCaChoice::CertificateOnly,
        };
        let options: Vec<_> = rules::server_tls_options(control_choice)
            .into_iter()
            .map(|(option, available)| notice::tls_choice(option, available))
            .collect();
        let Some(default) = options
            .iter()
            .position(|choice| choice.value == rules::default_server_tls_option(control_choice))
        else {
            unreachable!("default Server TLS option is always listed")
        };
        let id = PromptId::ServerTlsSource;
        let choice = value(
            self.interaction
                .select(id, notice::prompt(id), &options, default)?,
        )?;
        self.answers.tls_source = Some(choice);
        self.answers.tls = Some(match choice {
            ServerTlsOption::Issue => TlsMaterial::Issue,
            ServerTlsOption::Import => {
                let certificate = self.certificate(PromptId::ServerTlsCertificatePath)?;
                let key =
                    self.private_key(PromptId::ServerTlsKeyPath, PromptId::ServerTlsKeyPassphrase)?;
                if !key.matches(&certificate) {
                    self.rejected(notice::KEY_MISMATCH)?;
                    return Ok(Step::ServerTls);
                }
                TlsMaterial::Identity { certificate, key }
            }
            ServerTlsOption::SigningRequest => {
                let params = self.answers.parameters();
                let (key, csr) = generate::create_csr(params.server_ips())?;
                TlsMaterial::Request { key, csr }
            }
        });
        if choice == ServerTlsOption::SigningRequest {
            Ok(Step::PkiDirectory)
        } else {
            Ok(Step::OriginCa)
        }
    }

    pub(super) fn origin(&mut self) -> Result<Step, WizardFailure> {
        let choices = notice::origin_choices();
        let id = PromptId::OriginCaSource;
        let choice = value(
            self.interaction
                .select(id, notice::prompt(id), &choices, 0)?,
        )?;
        let (certificate, key) = if choice == CaSource::Generated {
            generate::generate_ca(notice::ORIGIN_NAME, notice::ORGANIZATION, self.host.now())?
        } else {
            let certificate = self.certificate(PromptId::OriginCaCertificatePath)?;
            let key =
                self.private_key(PromptId::OriginCaKeyPath, PromptId::OriginCaKeyPassphrase)?;
            if !ca_usable(&certificate) {
                self.rejected(notice::NOT_CA)?;
                return Ok(Step::OriginCa);
            }
            if !key.matches(&certificate) {
                self.rejected(notice::KEY_MISMATCH)?;
                return Ok(Step::OriginCa);
            }
            if key.algorithm() != KeyAlgorithm::P256 {
                self.rejected(notice::ORIGIN_P256)?;
                return Ok(Step::OriginCa);
            }
            (certificate, key)
        };
        self.answers.origin = Some(CaMaterial {
            certificate,
            key: Some(key),
            source: choice,
            passphrase: None,
            retention: if choice == CaSource::Generated {
                KeyRetention::ExternalCopy
            } else {
                KeyRetention::Imported
            },
        });
        Ok(Step::PkiDirectory)
    }

    pub(super) fn certificate(&mut self, id: PromptId) -> Result<Certificate, WizardFailure> {
        loop {
            let path = value(self.interaction.text(id, notice::prompt(id), None, None)?)?;
            let result = self
                .host
                .read_input(Path::new(&path))
                .map_err(WizardFailure::from)
                .and_then(|bytes| import::import_certificate(&bytes).map_err(WizardFailure::from));
            match result {
                Ok(certificate) => return Ok(certificate),
                Err(error) => self.rejected(&error.to_string())?,
            }
        }
    }

    fn private_key(
        &mut self,
        path_id: PromptId,
        passphrase_id: PromptId,
    ) -> Result<PrivateKey, WizardFailure> {
        loop {
            let path =
                value(
                    self.interaction
                        .text(path_id, notice::prompt(path_id), None, None)?,
                )?;
            let bytes = match self.host.read_input(Path::new(&path)) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.rejected(&error.to_string())?;
                    continue;
                }
            };
            match import::import_private_key(&bytes, None) {
                Ok(key) => return Ok(key),
                Err(ImportError::PassphraseRequired) => {}
                Err(error) => {
                    self.rejected(&error.to_string())?;
                    continue;
                }
            }
            for attempt in 0..3 {
                let passphrase = value(self.interaction.secret(
                    passphrase_id,
                    notice::prompt(passphrase_id),
                    false,
                )?)?;
                match import::import_private_key(&bytes, Some(&passphrase)) {
                    Ok(key) => return Ok(key),
                    Err(error @ ImportError::WrongPassphrase { .. }) => {
                        self.rejected(&error.to_string())?;
                        if attempt == 2 {
                            return Err(error.into());
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
}

fn ca_usable(certificate: &Certificate) -> bool {
    if !certificate.is_ca() {
        return false;
    }
    let Ok((_, parsed)) = x509_parser::parse_x509_certificate(certificate.der()) else {
        return false;
    };
    parsed
        .key_usage()
        .is_ok_and(|usage| usage.is_some_and(|usage| usage.value.key_cert_sign()))
}
