#[allow(dead_code)]
mod journal {
    pub mod event {
        include!("support/contract.rs");
        use contract::{DetailsWriter, JournalEvent, sealed};
        use types::{Category, Severity};

        #[derive(serde::Deserialize)]
        struct UnsafePayload {
            value: String,
        }

        impl sealed::Sealed for UnsafePayload {}
        impl JournalEvent for UnsafePayload {
            const KIND: &'static str = "unsafe_payload";
            const CATEGORY: Category = Category::Device;
            const SEVERITY: Severity = Severity::Info;

            fn write(&self, w: &mut DetailsWriter) {
                w.field("value", &self.value);
            }
        }
    }
}

fn main() {}
