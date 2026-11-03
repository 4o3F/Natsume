use std::{io, path::Path, sync::Arc};

use super::{
    ContestComponent, ExportError, OrganizationDetails,
    logo_cache::LogoValidationCache,
    logo_http_cache::LogoResponse,
    logo_io::{LogoDirectory, LogoError, LogoMatch},
};

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
    pub(crate) failure: Option<Arc<LogoError>>,
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
            // Capacity exhaustion is a domain rejection; semaphore detail does not change the decision.
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
        .map_err(|source| ExportError::Worker {
            source: source.into(),
        })?
    }

    // Callers resolve a current or pending organization before reaching file IO.
    pub(crate) async fn organization_logo(
        &self,
        organization: OrganizationDetails,
        conditions: Vec<String>,
    ) -> Result<LogoResponse, ExportError> {
        self.logo_http
            .response(
                self.logo_directory.clone(),
                organization,
                conditions,
                Arc::clone(&self.image_work),
            )
            .await
    }
}

fn observe(
    index: Option<&LogoDirectory>,
    organization: OrganizationDetails,
    validation: &LogoValidationCache,
) -> LogoObservation {
    let (status, paths, failure) = match resolve(index, &organization) {
        LogoMatch::Missing => (LogoStatus::Missing, vec![], None),
        LogoMatch::Ambiguous(paths) => (LogoStatus::Ambiguous, paths, None),
        LogoMatch::Unique(path) => match validation.validate(&path) {
            Ok(()) => (LogoStatus::Available, vec![path], None),
            Err(error) => (LogoStatus::Invalid, vec![path], Some(error)),
        },
    };
    LogoObservation {
        organization,
        status,
        files: paths.iter().map(|p| filename(p)).collect(),
        failure,
    }
}

pub(super) fn open_directory(path: &Path) -> Result<Option<LogoDirectory>, ExportError> {
    match LogoDirectory::open(path) {
        Ok(index) => Ok(Some(index)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ExportError::LogoDirectory { source: error }),
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
    error: LogoError,
) -> ExportError {
    ExportError::Logo {
        organization: format!("INST-{:03} {}", school.organization_id, school.key()),
        file: filename(path),
        reason: error.to_string(),
        source: Box::new(error),
    }
}
