#[allow(dead_code)]
mod component {
    pub mod device {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, specta::Type)]
        pub(crate) struct DeviceId;
        impl DeviceId {
            pub(crate) fn parse(_: &str) -> Option<Self> {
                Some(Self)
            }
            pub(crate) fn as_text(self) -> String {
                uuid::Uuid::nil().to_string()
            }
        }
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) struct EnrollmentReviewId;
        impl EnrollmentReviewId {
            pub(crate) fn parse(_: &str) -> Option<Self> {
                Some(Self)
            }
            pub(crate) fn as_text(self) -> String {
                uuid::Uuid::nil().to_string()
            }
        }
    }
}

#[allow(dead_code)]
mod journal {
    pub mod event {
        pub mod contract {
            include!("../../src/journal/event/contract.rs");
        }
        pub mod types {
            include!("../../src/journal/event/types.rs");
        }
    }
}

use journal::event::{contract::DetailsWriter, types::ReviewId};

fn accepts_review_id(w: &mut DetailsWriter, review: component::device::EnrollmentReviewId) {
    w.field("review_id", &ReviewId::from_enrollment_review(review));
}

fn main() {
    let _ = accepts_review_id;
}
