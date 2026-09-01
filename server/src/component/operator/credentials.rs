use super::{
    OperatorError,
    password::{self, OperatorPassword},
};

pub(crate) struct OperatorCredentials {
    login_name: String,
    password: OperatorPassword,
}

impl OperatorCredentials {
    /// Validates non-interactive operator credential input.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`OperatorError`] when the login name is empty or the
    /// two passwords differ.
    pub(crate) fn new(
        login_name: String,
        password: String,
        password_confirmation: String,
    ) -> Result<Self, OperatorError> {
        let password = OperatorPassword::new(password);
        let password_confirmation = OperatorPassword::new(password_confirmation);
        if login_name.is_empty() {
            return Err(OperatorError::EmptyLoginName);
        }
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
}
