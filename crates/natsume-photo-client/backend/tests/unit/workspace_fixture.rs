use crate::{
    roster::Roster,
    workspace::{Result, WorkerSnafu, Workspace},
};
use snafu::ResultExt as _;
use std::path::Path;

pub(crate) async fn create(root: &Path, roster: &Roster) -> Result<Workspace> {
    let root = root.to_owned();
    let teams = roster.teams().to_vec();
    tokio::task::spawn_blocking(move || {
        let source = root
            .parent()
            .unwrap_or_else(|| panic!("fixture target has a parent"))
            .to_owned();
        Workspace::create_with_logos(root, teams, &source, Vec::new())
    })
    .await
    .context(WorkerSnafu)?
}
pub(crate) async fn create_with_resources(
    root: &Path,
    roster: &Roster,
    manifest: &Path,
) -> Result<Workspace> {
    let input: serde_json::Value = serde_json::from_slice(
        &std::fs::read(manifest).unwrap_or_else(|error| panic!("read resource fixture: {error:?}")),
    )
    .unwrap_or_else(|error| panic!("parse resource fixture: {error:?}"));
    let logos = input["school_logos"]
        .as_array()
        .unwrap_or_else(|| panic!("fixture logo array"))
        .iter()
        .map(|logo| crate::workspace::resources::SchoolLogo {
            organization_id: logo["organization_id"]
                .as_i64()
                .unwrap_or_else(|| panic!("fixture organization")),
            path: logo["path"]
                .as_str()
                .unwrap_or_else(|| panic!("fixture logo path"))
                .into(),
        })
        .collect();
    let root = root.to_owned();
    let teams = roster.teams().to_vec();
    let source = std::fs::canonicalize(
        manifest
            .parent()
            .unwrap_or_else(|| panic!("fixture manifest parent")),
    )
    .unwrap_or_else(|error| panic!("canonical fixture source: {error:?}"));
    tokio::task::spawn_blocking(move || Workspace::create_with_logos(root, teams, &source, logos))
        .await
        .context(WorkerSnafu)?
}
