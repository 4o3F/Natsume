//! Packaged service-account lookup without inheriting the administrator's environment.

use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    path::PathBuf,
    process::{Command, ExitStatus},
};

/// Account provisioned by the Server Deb's sysusers configuration.
pub const SERVICE_USER: &str = "natsume-server";
/// Executable search path shared by all setup child processes.
pub const SYSTEM_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

/// Unix identity and home directory used for service-user child processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAccount {
    /// Login name resolved by the system account database.
    pub name: String,
    /// Nonzero numeric user identity used for privilege dropping.
    pub uid: u32,
    /// Primary group identity used for privilege dropping.
    pub gid: u32,
    /// Packaged service home exported to child processes.
    pub home: PathBuf,
}

impl ServiceAccount {
    /// Resolves exactly one passwd entry using a fixed, sanitized getent invocation.
    ///
    /// # Errors
    /// Returns missing-account, process, encoding, or passwd-field errors with their causes.
    pub fn lookup(name: &str) -> Result<Self, AccountError> {
        let output = Command::new("/usr/bin/getent")
            .args(["passwd", name])
            .env_clear()
            .env("PATH", SYSTEM_PATH)
            .output()
            .context(LookupSnafu)?;
        if output.status.code() == Some(2) {
            return MissingSnafu.fail();
        }
        ensure!(
            output.status.success(),
            LookupStatusSnafu {
                status: output.status
            }
        );
        let text = String::from_utf8(output.stdout).context(EncodingSnafu)?;
        let account = parse_passwd_line(text.trim_end_matches(['\r', '\n']))?;
        ensure!(account.name == name, InvalidEntrySnafu);
        Ok(account)
    }
}

fn parse_passwd_line(line: &str) -> Result<ServiceAccount, AccountError> {
    let mut fields = line.split(':');
    let (Some(name), Some(_password), Some(uid), Some(gid), Some(_gecos), Some(home), Some(_shell)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return InvalidEntrySnafu.fail();
    };
    ensure!(
        fields.next().is_none()
            && !name.is_empty()
            && PathBuf::from(home).is_absolute()
            && !line.contains(['\r', '\n']),
        InvalidEntrySnafu
    );
    let uid = uid
        .parse::<u32>()
        .context(NumericIdSnafu { field: "uid" })?;
    let gid = gid
        .parse::<u32>()
        .context(NumericIdSnafu { field: "gid" })?;
    ensure!(uid != 0, RootAccountSnafu);
    Ok(ServiceAccount {
        name: name.to_owned(),
        uid,
        gid,
        home: home.into(),
    })
}

/// Failure to obtain a safe non-root service identity.
#[derive(Debug, Snafu)]
pub enum AccountError {
    /// The Deb's service identity has not been provisioned.
    #[snafu(display("service account missing; install the Server Deb first"))]
    Missing,
    /// The account database command could not be started.
    #[snafu(display("cannot run getent passwd"))]
    Lookup {
        /// Operating system process failure.
        source: std::io::Error,
    },
    /// Getent failed for a reason other than a missing record.
    #[snafu(display("getent passwd failed: {status}"))]
    LookupStatus {
        /// Child exit status.
        status: ExitStatus,
    },
    /// The account database returned malformed UTF-8.
    #[snafu(display("getent passwd returned invalid text"))]
    Encoding {
        /// Original text decoding cause.
        source: std::string::FromUtf8Error,
    },
    /// The passwd record did not have exactly seven valid fields.
    #[snafu(display("invalid service passwd entry"))]
    InvalidEntry,
    /// A numeric identity could not be parsed.
    #[snafu(display("invalid service account {field}"))]
    NumericId {
        /// Passwd field being decoded.
        field: &'static str,
        /// Numeric parser cause.
        source: std::num::ParseIntError,
    },
    /// The supplied service account would run probes as root.
    #[snafu(display("service account must not have uid 0"))]
    RootAccount,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;
    #[test]
    fn passwd_records_are_strictly_validated() -> Result<(), AccountError> {
        let account = parse_passwd_line(
            "natsume-server:x:123:456:Server:/var/lib/natsume-server:/usr/sbin/nologin",
        )?;
        assert_eq!(account.uid, 123);
        assert_eq!(account.gid, 456);
        assert_eq!(account.home, PathBuf::from("/var/lib/natsume-server"));
        assert!(matches!(
            parse_passwd_line("name:x:123"),
            Err(AccountError::InvalidEntry)
        ));
        let Err(error) = parse_passwd_line("name:x:no:456::/home:/bin/false") else {
            panic!("nonnumeric uid accepted");
        };
        assert!(error.source().is_some());
        assert!(matches!(
            parse_passwd_line("name:x:0:0::/root:/bin/false"),
            Err(AccountError::RootAccount)
        ));
        Ok(())
    }
}
