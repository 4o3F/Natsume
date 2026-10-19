use super::{
    OperatorError,
    password::{self, OperatorPassword},
};

pub(crate) struct OperatorCredentials {
    login_name: String,
    password: OperatorPassword,
}

pub(super) fn validate_input(login_name: &str, password: &str) -> Result<(), OperatorError> {
    if login_name.is_empty() {
        return Err(OperatorError::EmptyLoginName);
    }
    if login_name.len() > 128 || password.len() > 1024 {
        return Err(OperatorError::CredentialsTooLong);
    }
    Ok(())
}

pub(super) fn validate_new_login_name(login_name: &str) -> Result<(), OperatorError> {
    validate_input(login_name, "")?;
    if login_name.trim() != login_name {
        return Err(OperatorError::InvalidNewLoginName);
    }
    Ok(())
}

pub(super) fn validate_new_password(password: &str) -> Result<(), OperatorError> {
    const SPECIAL_CHARACTERS: &[u8] = b"!@#$%^&*()-_=+[]{};:,.?/~";
    if password.len() > 1024 {
        return Err(OperatorError::CredentialsTooLong);
    }
    if password.len() < 16
        || !password
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || SPECIAL_CHARACTERS.contains(&byte))
        || !password.bytes().any(|byte| byte.is_ascii_digit())
        || !password
            .bytes()
            .any(|byte| SPECIAL_CHARACTERS.contains(&byte))
    {
        return Err(OperatorError::InvalidNewPassword);
    }
    Ok(())
}

impl OperatorCredentials {
    /// Validates non-interactive operator credential input.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`OperatorError`] for invalid lengths, a new password
    /// that violates the policy, or mismatched passwords. Existing login names
    /// are not normalized so TTY recovery can still address legacy accounts.
    pub(crate) fn new(
        login_name: String,
        password: String,
        password_confirmation: String,
    ) -> Result<Self, OperatorError> {
        let password = OperatorPassword::new(password);
        let password_confirmation = OperatorPassword::new(password_confirmation);
        validate_input(&login_name, password.expose())?;
        validate_new_password(password.expose())?;
        if password.expose() != password_confirmation.expose() {
            return Err(OperatorError::PasswordMismatch);
        }
        Ok(Self {
            login_name,
            password,
        })
    }

    pub(crate) fn login_name(&self) -> &str {
        &self.login_name
    }

    pub(crate) fn hash_password(&self) -> Result<String, OperatorError> {
        password::hash_password(&self.password)
    }
}

#[cfg(test)]
mod tests {
    use super::{OperatorCredentials, OperatorPassword};

    impl OperatorCredentials {
        pub(in crate::component::operator) fn password(&self) -> &OperatorPassword {
            &self.password
        }
    }

    #[test]
    fn credential_limits_count_bytes_and_preserve_accepted_values() {
        let login = "é".repeat(64);
        let password = "a".repeat(1022) + "1!";
        let credentials =
            OperatorCredentials::new(login.clone(), password.clone(), password.clone())
                .unwrap_or_else(|error| panic!("boundary credentials rejected: {error}"));
        assert_eq!(credentials.login_name(), login);
        assert_eq!(credentials.password.expose(), password);
        for (login, password) in [
            (login + "x", password.clone()),
            ("admin".to_owned(), password + "x"),
        ] {
            assert_eq!(
                super::validate_input(&login, &password),
                Err(super::OperatorError::CredentialsTooLong)
            );
            assert_eq!(
                OperatorCredentials::new(login, password.clone(), password).err(),
                Some(super::OperatorError::CredentialsTooLong)
            );
        }
    }

    #[test]
    fn registration_names_preserve_case_and_use_utf8_byte_limits() {
        for name in ["Alice", "alice", "A B", "用户", &"é".repeat(64)] {
            assert_eq!(super::validate_new_login_name(name), Ok(()));
        }
        assert_eq!(
            super::validate_new_login_name(""),
            Err(super::OperatorError::EmptyLoginName)
        );
        assert_eq!(
            super::validate_new_login_name(&("é".repeat(64) + "x")),
            Err(super::OperatorError::CredentialsTooLong)
        );
        for name in [" Alice", "Alice ", "\tAlice", "Alice\n", "\u{3000}Alice"] {
            assert_eq!(
                super::validate_new_login_name(name),
                Err(super::OperatorError::InvalidNewLoginName)
            );
            assert_eq!(super::validate_input(name, "old password"), Ok(()));
        }
    }

    #[test]
    fn new_password_policy_enforces_length_and_both_required_character_classes() {
        for password in [
            "",
            "Password123456!",
            "abcdefghijklmnop!",
            "abcdefghijklmnop1",
            "中文密码123456789012345!",
        ] {
            assert_eq!(
                super::validate_new_password(password),
                Err(super::OperatorError::InvalidNewPassword)
            );
        }
        for password in ["Password1234567!", "123456789012345!"] {
            assert_eq!(super::validate_new_password(password), Ok(()));
        }
        assert_eq!(
            super::validate_new_password(&("a".repeat(1022) + "1!")),
            Ok(())
        );
        assert_eq!(
            super::validate_new_password(&("a".repeat(1023) + "1!")),
            Err(super::OperatorError::CredentialsTooLong)
        );
    }

    #[test]
    fn new_passwords_accept_exactly_the_confirmed_ascii_set() {
        for byte in 0..=127_u8 {
            let password = format!("Password1234567!{}", char::from(byte));
            let allowed =
                byte.is_ascii_alphanumeric() || b"!@#$%^&*()-_=+[]{};:,.?/~".contains(&byte);
            assert_eq!(super::validate_new_password(&password).is_ok(), allowed);
        }
        for byte in b"!@#$%^&*()-_=+[]{};:,.?/~" {
            assert_eq!(
                super::validate_new_password(&format!("Password1234567{}", char::from(*byte))),
                Ok(())
            );
        }
    }

    #[test]
    fn tty_credentials_enforce_new_password_policy_and_exact_confirmation() {
        let password = "Password1234567!";
        assert_eq!(
            OperatorCredentials::new("admin".into(), "old password".into(), "old password".into())
                .err(),
            Some(super::OperatorError::InvalidNewPassword)
        );
        assert_eq!(
            OperatorCredentials::new("admin".into(), password.into(), "different".into()).err(),
            Some(super::OperatorError::PasswordMismatch)
        );
        let credentials =
            OperatorCredentials::new(" admin ".into(), password.into(), password.into())
                .unwrap_or_else(|error| panic!("legacy recovery login rejected: {error}"));
        assert_eq!(credentials.login_name(), " admin ");
        assert_eq!(credentials.password.expose(), password);
    }
}
