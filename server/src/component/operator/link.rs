use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy)]
pub(super) enum LinkKind {
    Invitation,
    PasswordReset,
}

impl LinkKind {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Invitation => "invite_",
            Self::PasswordReset => "reset_",
        }
    }
}

// SecretString redacts Debug output and zeroizes the wire value on drop.
#[derive(Debug)]
pub(crate) struct LinkToken(SecretString);

impl LinkToken {
    pub(super) fn generate(kind: LinkKind) -> Result<Self, getrandom::Error> {
        let mut bytes = Zeroizing::new([0_u8; 32]);
        getrandom::fill(&mut *bytes)?;
        let encoded = Zeroizing::new(hex::encode(bytes.as_slice()));
        Ok(Self(
            format!("{}{}", kind.prefix(), encoded.as_str()).into(),
        ))
    }

    pub(super) fn from_wire(kind: LinkKind, value: String) -> Option<Self> {
        let value: SecretString = value.into();
        let hex = value.expose_secret().strip_prefix(kind.prefix())?;
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        Some(Self(value))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    pub(super) fn sha256(&self) -> [u8; 32] {
        // Hash the purpose prefix as well so equal entropy has distinct grants.
        Sha256::digest(self.expose().as_bytes()).into()
    }
}

#[cfg(test)]
mod tests {
    use super::{LinkKind, LinkToken};

    #[test]
    fn generated_tokens_round_trip_without_revealing_their_secret() {
        for kind in [LinkKind::Invitation, LinkKind::PasswordReset] {
            let token = LinkToken::generate(kind)
                .unwrap_or_else(|error| panic!("token generation failed: {error}"));
            assert_eq!(token.expose().len(), kind.prefix().len() + 64);
            let parsed = LinkToken::from_wire(kind, token.expose().to_owned())
                .unwrap_or_else(|| panic!("generated token rejected"));
            assert_eq!(parsed.sha256(), token.sha256());
            let debug = format!("{token:?}");
            assert!(debug.contains("REDACTED"));
            assert!(!debug.contains(token.expose()));
            assert!(!debug.contains(&token.expose()[kind.prefix().len()..]));
        }
    }

    #[test]
    fn purposes_cannot_be_interchanged_even_with_the_same_entropy() {
        let entropy = "0123456789abcdef".repeat(4);
        let invitation = LinkToken::from_wire(LinkKind::Invitation, format!("invite_{entropy}"))
            .unwrap_or_else(|| panic!("invitation fixture rejected"));
        let reset = LinkToken::from_wire(LinkKind::PasswordReset, format!("reset_{entropy}"))
            .unwrap_or_else(|| panic!("reset fixture rejected"));
        assert_ne!(invitation.sha256(), reset.sha256());
        assert!(
            LinkToken::from_wire(LinkKind::PasswordReset, invitation.expose().into()).is_none()
        );
        assert!(LinkToken::from_wire(LinkKind::Invitation, reset.expose().into()).is_none());
    }

    #[test]
    fn malformed_tokens_are_rejected_without_normalization() {
        let entropy = "abcdef0123456789".repeat(4);
        for kind in [LinkKind::Invitation, LinkKind::PasswordReset] {
            let valid = format!("{}{entropy}", kind.prefix());
            for invalid in [
                String::new(),
                entropy.clone(),
                valid[..valid.len() - 1].to_owned(),
                format!("{valid}0"),
                format!(" {valid}"),
                format!("{valid}\n"),
                valid.to_uppercase(),
                format!("{}{}", kind.prefix(), entropy.to_uppercase()),
                format!("{}{}", kind.prefix(), "g".repeat(64)),
                format!("{}{}", kind.prefix(), "é".repeat(32)),
            ] {
                assert!(LinkToken::from_wire(kind, invalid).is_none());
            }
        }
    }
}
