//! Current Contest identities, indexed once for a Photo use case.
use natsume_photo_protocol::PhotoTeam;
use std::collections::HashMap;

pub(crate) struct PhotoRoster(HashMap<String, String>);

impl From<Vec<PhotoTeam>> for PhotoRoster {
    fn from(teams: Vec<PhotoTeam>) -> Self {
        Self(
            teams
                .into_iter()
                .map(|team| (team.account_id, team.account))
                .collect(),
        )
    }
}

impl PhotoRoster {
    pub(super) fn account(&self, id: &str) -> Option<&str> {
        self.0.get(id).map(String::as_str)
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }

    pub(super) fn ids(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}
