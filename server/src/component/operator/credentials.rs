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

impl OperatorCredentials {
    /// Validates non-interactive operator credential input.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`OperatorError`] for invalid lengths or mismatched passwords.
    pub(crate) fn new(
        login_name: String,
        password: String,
        password_confirmation: String,
    ) -> Result<Self, OperatorError> {
        let password = OperatorPassword::new(password);
        let password_confirmation = OperatorPassword::new(password_confirmation);
        validate_input(&login_name, password.expose())?;
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
        let password = "密".repeat(341) + "x";
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
}
