//! Server-owned vertical business components.

// TODO(WP7): Consume Binding from the production DeviceActor.
#[allow(dead_code)]
pub(crate) mod binding;
pub(crate) mod contest;
// TODO(WP7): Consume Device from production Device Control.
#[allow(dead_code)]
pub(crate) mod device;
// TODO(WP7): Consume Gateway from the production DeviceActor.
#[allow(dead_code)]
pub(crate) mod gateway;
pub(crate) mod import;
pub(crate) mod operator;
pub(crate) mod provisioning;
