pub mod contract {
    include!("../../../src/journal/event/contract.rs");
}

pub mod types {
    #[derive(Clone, Copy)]
    pub(crate) enum Category {
        Device,
    }

    #[derive(Clone, Copy)]
    pub(crate) enum Severity {
        Info,
    }
}
