//! Client-local UUID roles, using the shared validated representation.
use natsume_photo_protocol::Identity;
/// Local Client identity role, never part of the HTTP contract.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClientRole {}
/// Canonical `UUIDv7` for the local Client.
pub type ClientId = Identity<ClientRole>;
/// Local Prepared identity role, never part of the HTTP contract.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PreparedRole {}
/// Canonical `UUIDv7` for the local Prepared.
pub type PreparedId = Identity<PreparedRole>;
