use serde::{Deserialize, Serialize};

/// Non-secret school facts; the HTTP adapter formats the persistent sequence as INST-xxx.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OrganizationDetails {
    pub(crate) organization_id: i64,
    pub(crate) name_zh: String,
    pub(crate) name_en: String,
    pub(crate) country: String,
}

impl OrganizationDetails {
    pub(crate) fn key(&self) -> &str {
        if self.name_zh.is_empty() {
            &self.name_en
        } else {
            &self.name_zh
        }
    }
}

// Passwords remain encrypted until the database read transaction is closed.
pub(super) struct ExportTeam {
    pub(super) account: String,
    pub(super) seat: String,
    pub(super) organization_id: i64,
    pub(super) name_zh: String,
    pub(super) name_en: String,
    pub(super) category: String,
    pub(super) nonce: Vec<u8>,
    pub(super) ciphertext: Vec<u8>,
}

pub(super) struct ExportRoster {
    pub(super) organizations: Vec<OrganizationDetails>,
    pub(super) teams: Vec<ExportTeam>,
}
