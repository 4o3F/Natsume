use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
};

use crate::component::operator::OperatorCredentials;

use super::error::{CommandFailure as CommandError, WorkError};

const LOGIN_NAME_PROMPT: &[u8] = b"Login name: ";
const PASSWORD_PROMPT: &str = "Password: ";
const PASSWORD_CONFIRMATION_PROMPT: &str = "Confirm password: ";

pub(super) fn read_from_tty(
    credential_error: impl Fn(WorkError) -> CommandError,
) -> Result<OperatorCredentials, CommandError> {
    let mut terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|source| credential_error(WorkError::Io { source }))?;
    terminal
        .write_all(LOGIN_NAME_PROMPT)
        .and_then(|()| terminal.flush())
        .map_err(|source| credential_error(WorkError::Io { source }))?;
    let mut login_name = String::new();
    BufReader::new(terminal)
        .read_line(&mut login_name)
        .map_err(|source| credential_error(WorkError::Io { source }))?;
    while login_name.ends_with(['\r', '\n']) {
        login_name.pop();
    }

    let password = rpassword::prompt_password(PASSWORD_PROMPT)
        .map_err(|source| credential_error(WorkError::Io { source }))?;
    let password_confirmation = rpassword::prompt_password(PASSWORD_CONFIRMATION_PROMPT)
        .map_err(|source| credential_error(WorkError::Io { source }))?;
    OperatorCredentials::new(login_name, password, password_confirmation)
        .map_err(|source| credential_error(WorkError::Operator { source }))
}
