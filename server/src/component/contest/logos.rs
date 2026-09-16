use std::{io, path::Path};

use natsume_roster::{LogoDirectory, LogoImage, LogoMatch, read_logo};

use super::{ContestComponent, ExportError, OrganizationDetails, logo_cache::LogoValidationCache};

#[derive(Clone, Copy)]
pub(crate) enum LogoStatus {
    Available,
    Missing,
    Ambiguous,
    Invalid,
}

pub(crate) struct LogoObservation {
    pub(crate) organization: OrganizationDetails,
    pub(crate) status: LogoStatus,
    pub(crate) files: Vec<String>,
    pub(crate) detail: Option<String>,
}

impl ContestComponent {
    pub(crate) async fn observe_logos(
        &self,
        organizations: Vec<OrganizationDetails>,
    ) -> Result<Vec<LogoObservation>, ExportError> {
        let permit = self
            .image_work
            .clone()
            .try_acquire_owned()
            .map_err(|_| ExportError::Busy)?;
        let directory = self.logo_directory.clone();
        let validation = self.logo_validation.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let index = open_directory(&directory)?;
            Ok(organizations
                .into_iter()
                .map(|organization| observe(index.as_ref(), organization, &validation))
                .collect())
        })
        .await
        .map_err(|_| ExportError::Worker)?
    }

    // Callers resolve a current or pending organization before reaching file IO.
    pub(crate) async fn organization_logo(
        &self,
        organization: OrganizationDetails,
    ) -> Result<LogoImage, ExportError> {
        let permit = self
            .image_work
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ExportError::Worker)?;
        let directory = self.logo_directory.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let index = open_directory(&directory)?;
            match resolve(index.as_ref(), &organization) {
                LogoMatch::Missing | LogoMatch::Ambiguous(_) => Err(ExportError::LogoUnavailable),
                LogoMatch::Unique(path) => read_logo(&path)
                    .and_then(natsume_roster::DecodedLogo::into_http)
                    .map_err(|error| logo_error(&organization, &path, &error)),
            }
        })
        .await
        .map_err(|_| ExportError::Worker)?
    }
}

fn observe(
    index: Option<&LogoDirectory>,
    organization: OrganizationDetails,
    validation: &LogoValidationCache,
) -> LogoObservation {
    let (status, paths, detail) = match resolve(index, &organization) {
        LogoMatch::Missing => (LogoStatus::Missing, vec![], None),
        LogoMatch::Ambiguous(paths) => (LogoStatus::Ambiguous, paths, None),
        LogoMatch::Unique(path) => match validation.validate(&path) {
            Ok(()) => (LogoStatus::Available, vec![path], None),
            Err(error) => {
                tracing::warn!(organization_id = organization.organization_id, path = %path.display(), %error, "School logo could not be read");
                (LogoStatus::Invalid, vec![path], Some(error))
            }
        },
    };
    LogoObservation {
        organization,
        status,
        files: paths.iter().map(|p| filename(p)).collect(),
        detail,
    }
}

pub(super) fn open_directory(path: &Path) -> Result<Option<LogoDirectory>, ExportError> {
    match LogoDirectory::open(path) {
        Ok(index) => Ok(Some(index)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            tracing::error!(path = %path.display(), %error, "Cannot read school logo directory");
            Err(ExportError::LogoDirectory)
        }
    }
}

pub(super) fn resolve(index: Option<&LogoDirectory>, school: &OrganizationDetails) -> LogoMatch {
    index.map_or(LogoMatch::Missing, |index| {
        index.resolve(&school.name_zh, &school.name_en)
    })
}

pub(super) fn filename(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub(super) fn logo_error(
    school: &OrganizationDetails,
    path: &Path,
    error: &natsume_roster::LogoError,
) -> ExportError {
    tracing::error!(organization_id = school.organization_id, path = %path.display(), %error, "School logo processing failed");
    ExportError::Logo {
        organization: format!("INST-{:03} {}", school.organization_id, school.key()),
        file: filename(path),
        reason: error.to_string(),
    }
}
