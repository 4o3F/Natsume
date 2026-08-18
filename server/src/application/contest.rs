use snafu::Snafu;

mod account;
mod binding;
mod seat;

pub(crate) use self::{
    account::{AccountFacts, list_accounts},
    binding::{BindingFacts, list_bindings},
    seat::{SeatFacts, list_seats},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum ContestError {
    #[snafu(display("contest current facts could not be read"))]
    PersistenceFailed,
}
