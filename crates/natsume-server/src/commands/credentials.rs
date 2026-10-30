use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
};

use crate::component::operator::{OperatorCredentials, OperatorError, PASSWORD_POLICY};

use super::error::{CommandFailure as CommandError, WorkError};

const LOGIN_NAME_PROMPT: &str = "Login name: ";
const PASSWORD_PROMPT: &str = "Password: ";
const PASSWORD_CONFIRMATION_PROMPT: &str = "Confirm password: ";

/// Operator account change that the TTY credentials will be used for.
#[derive(Debug, Clone, Copy)]
pub(super) enum CredentialPurpose {
    /// Creating the first administrator during bootstrap.
    FirstAdministrator,
    /// Replacing the password of an existing operator.
    PasswordReset,
}

impl CredentialPurpose {
    const fn introduction(self) -> &'static str {
        match self {
            Self::FirstAdministrator => {
                "Create the first Natsume Panel administrator. Use this login name and password to sign in to the Web Panel after installation."
            }
            Self::PasswordReset => {
                "Reset an existing Natsume Panel operator's password. Enter that operator's login name and a new password."
            }
        }
    }
}

/// Reads credentials from the controlling terminal, explaining the rules up front and
/// asking again after any input the policy rejects.
pub(super) fn read_from_tty(
    purpose: CredentialPurpose,
    credential_error: impl Fn(WorkError) -> CommandError,
) -> Result<OperatorCredentials, CommandError> {
    let io_error = |source| credential_error(WorkError::Io { source });
    let mut terminal = BufReader::new(
        OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(io_error)?,
    );
    write_terminal(
        &mut terminal,
        &format!(
            "\n{}\nPassword rules: {PASSWORD_POLICY}.\n\n",
            purpose.introduction()
        ),
    )
    .map_err(io_error)?;
    loop {
        write_terminal(&mut terminal, LOGIN_NAME_PROMPT).map_err(io_error)?;
        let mut login_name = String::new();
        if terminal.read_line(&mut login_name).map_err(io_error)? == 0 {
            return Err(io_error(io::ErrorKind::UnexpectedEof.into()));
        }
        while login_name.ends_with(['\r', '\n']) {
            login_name.pop();
        }

        let password = rpassword::prompt_password(PASSWORD_PROMPT).map_err(io_error)?;
        let password_confirmation =
            rpassword::prompt_password(PASSWORD_CONFIRMATION_PROMPT).map_err(io_error)?;
        match OperatorCredentials::new(login_name, password, password_confirmation) {
            Ok(credentials) => return Ok(credentials),
            Err(
                rejection @ (OperatorError::EmptyLoginName
                | OperatorError::CredentialsTooLong
                | OperatorError::InvalidNewPassword
                | OperatorError::PasswordMismatch),
            ) => write_terminal(
                &mut terminal,
                &format!("Rejected: {rejection}. Enter the credentials again.\n\n"),
            )
            .map_err(io_error)?,
            Err(source) => return Err(credential_error(WorkError::Operator { source })),
        }
    }
}

fn write_terminal(terminal: &mut BufReader<File>, text: &str) -> io::Result<()> {
    let terminal = terminal.get_mut();
    terminal.write_all(text.as_bytes())?;
    terminal.flush()
}
