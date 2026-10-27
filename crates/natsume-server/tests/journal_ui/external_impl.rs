#[allow(dead_code)]
mod journal {
    pub mod event {
        include!("support/contract.rs");
    }
}

use journal::event::{
    contract::{DetailsWriter, JournalEvent, JournalField},
    types::{Category, Severity},
};

#[derive(serde::Serialize)]
struct ExternalSecret;
impl JournalField for ExternalSecret {}
impl JournalEvent for ExternalSecret {
    const KIND: &'static str = "external_secret";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

fn main() {}
