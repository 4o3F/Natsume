//! Server-owned vertical business components.

// TODO(WP8): Consume Binding operator mutations from the HTTP surface.
#[allow(dead_code)]
pub(crate) mod binding;
pub(crate) mod contest;
// TODO(WP8): Consume Enrollment review and lifecycle mutations from the HTTP surface.
#[allow(dead_code)]
pub(crate) mod device;
pub(crate) mod gateway;
// TODO(WP8): Consume Home operator mutations from the HTTP surface.
#[allow(dead_code)]
pub(crate) mod home;
pub(crate) mod import;
pub(crate) mod operator;
pub(crate) mod provisioning;
// TODO(WP8): Consume Runtime Config operator mutations from the HTTP surface.
#[allow(dead_code)]
pub(crate) mod runtime;
// TODO(WP8): Consume Session Control operator mutations from the HTTP surface.
#[allow(dead_code)]
pub(crate) mod session;
