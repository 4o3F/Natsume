slint::slint! {
    import { PrintWindow, Screen } from "gui/window.slint";
    export { PrintWindow, Screen }
}

mod code;
mod controller;
#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod tray;
mod view;

/// Opens the Windows print client window and runs its application controls.
///
/// # Errors
/// Returns an error if the local application directory is unavailable, the window
/// or tray cannot be initialized, or an application worker cannot be joined.
#[cfg(windows)]
pub fn run(path: Option<std::path::PathBuf>) -> super::error::Result<()> {
    use snafu::ResultExt as _;
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            super::error::AttentionSnafu {
                reason: "LOCALAPPDATA is unavailable",
            }
            .build()
        })?
        .join("NatsumePrint");
    controller::run(
        path,
        &base,
        |path, controls, snapshots| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context(super::error::StorageSnafu {
                    operation: "start application runtime",
                })?;
            runtime.block_on(super::application::run(
                &path,
                &super::windows::VaultCredentials,
                super::windows::NativePrinter,
                controls,
                snapshots,
            ))
        },
        crate::backend::list_queues,
    )
}
