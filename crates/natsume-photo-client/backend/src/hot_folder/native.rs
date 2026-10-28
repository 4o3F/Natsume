//! Safe `WinRT` notification subscription. Notifications only wake reconciliation.
use std::{os::windows::ffi::OsStrExt as _, path::Path};

use tokio::sync::mpsc;
use windows::{
    Foundation::TypedEventHandler,
    Storage::Search::{
        IStorageQueryResultBase, IndexerOption, QueryOptions, StorageFileQueryResult,
    },
    Storage::StorageFolder,
};

pub(super) struct NativeMonitor {
    query: StorageFileQueryResult,
    subscription: i64,
}

impl NativeMonitor {
    pub(super) async fn open(path: &Path, wake: mpsc::Sender<()>) -> windows::core::Result<Self> {
        // WinRT expects a DOS/UNC path, not canonicalize's verbatim prefix.
        // Keep UTF-16 intact so non-Unicode Windows names are not replaced.
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
        let unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
        let native = if wide.starts_with(&unc) {
            [vec![92, 92], wide[unc.len()..].to_vec()].concat()
        } else if wide.starts_with(&prefix) {
            wide[prefix.len()..].to_vec()
        } else {
            wide
        };
        let folder =
            StorageFolder::GetFolderFromPathAsync(&windows::core::HSTRING::from_wide(&native))?
                .await?;
        let options = QueryOptions::new()?;
        options.SetIndexerOption(IndexerOption::DoNotUseIndexer)?;
        let query = folder.CreateFileQueryWithOptions(&options)?;
        let handler =
            TypedEventHandler::<IStorageQueryResultBase, windows::core::IInspectable>::new(
                move |_, _| {
                    let _ = wake.try_send(());
                    Ok(())
                },
            );
        let subscription = query.ContentsChanged(&handler)?;
        let monitor = Self {
            query,
            subscription,
        };
        monitor.refresh().await?;
        Ok(monitor)
    }

    pub(super) async fn refresh(&self) -> windows::core::Result<()> {
        // Reading the result arms the next coalesced ContentsChanged event.
        self.query.GetFilesAsync(0, 1)?.await?;
        Ok(())
    }
}

impl Drop for NativeMonitor {
    fn drop(&mut self) {
        if self.query.RemoveContentsChanged(self.subscription).is_err() {
            tracing::warn!("could not remove the Photo directory notification subscription");
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/native.rs"]
mod tests;
