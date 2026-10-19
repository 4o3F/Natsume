use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
};

use crate::component::operator::OperatorCredentials;

use super::CommandError;

const LOGIN_NAME_PROMPT: &[u8] = b"Login name: ";
const PASSWORD_PROMPT: &str = "Password: ";
const PASSWORD_CONFIRMATION_PROMPT: &str = "Confirm password: ";

pub(super) fn read_from_tty(
    credential_error: CommandError,
) -> Result<OperatorCredentials, CommandError> {
    let mut terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| credential_error)?;
    terminal
        .write_all(LOGIN_NAME_PROMPT)
        .and_then(|()| terminal.flush())
        .map_err(|_| credential_error)?;
    let mut login_name = String::new();
    BufReader::new(terminal)
        .read_line(&mut login_name)
        .map_err(|_| credential_error)?;
    while login_name.ends_with(['\r', '\n']) {
        login_name.pop();
    }

    let password = rpassword::prompt_password(PASSWORD_PROMPT).map_err(|_| credential_error)?;
    let password_confirmation =
        rpassword::prompt_password(PASSWORD_CONFIRMATION_PROMPT).map_err(|_| credential_error)?;
    OperatorCredentials::new(login_name, password, password_confirmation)
        .map_err(|_| credential_error)
}
