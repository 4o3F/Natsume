use super::PrintWindow;
use crate::client::error::{ClipboardSnafu, InvalidSnafu, Result, StorageSnafu};
use slint::ComponentHandle as _;
use snafu::ResultExt as _;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn display_path(path: &Path) -> Result<slint::SharedString> {
    path.to_str().map(Into::into).ok_or_else(|| {
        InvalidSnafu {
            field: "selected path Unicode",
        }
        .build()
    })
}

fn report(ui: &PrintWindow, result: Result<()>) {
    if let Err(error) = result {
        tracing::warn!(%error, "desktop action failed");
        super::view::operation_error(ui, &error.to_string());
    }
}

fn windows_directory() -> Result<PathBuf> {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            InvalidSnafu {
                field: "Windows system directory",
            }
            .build()
        })
}

fn open_directory(path: &Path) -> Result<()> {
    let path = std::path::absolute(path).context(StorageSnafu {
        operation: "resolve configuration directory",
    })?;
    let parent = path
        .parent()
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            InvalidSnafu {
                field: "configuration directory",
            }
            .build()
        })?;
    Command::new(windows_directory()?.join("explorer.exe"))
        .arg(parent)
        .spawn()
        .context(StorageSnafu {
            operation: "open configuration directory",
        })?;
    Ok(())
}

fn open_queue(queue: &str) -> Result<()> {
    if queue.is_empty()
        || queue.starts_with(['/', '-'])
        || queue.chars().any(|ch| ch.is_control() || ch == '"')
    {
        return InvalidSnafu {
            field: "queue name for desktop shortcut",
        }
        .fail();
    }
    let system = windows_directory()?.join("System32");
    let mut entry = system.join("printui.dll").into_os_string();
    entry.push(",PrintUIEntry");
    Command::new(system.join("rundll32.exe"))
        .arg(entry)
        .args(["/o", "/n", queue])
        .spawn()
        .context(StorageSnafu {
            operation: "open Windows print queue",
        })?;
    Ok(())
}

fn pick_config(ui: &PrintWindow, save: bool) {
    let dialog = rfd::FileDialog::new()
        .set_title(if save {
            "选择新配置文件位置（已有文件不会被覆盖）"
        } else {
            "加载已有客户端配置"
        })
        .add_filter("TOML 配置", &["toml"]);
    let selected = if save {
        dialog.set_file_name("client.toml").save_file()
    } else {
        dialog.pick_file()
    };
    if let Some(path) = selected {
        report(ui, display_path(&path).map(|path| ui.set_config_path(path)));
    }
}

fn pick_state(ui: &PrintWindow) {
    if let Some(path) = rfd::FileDialog::new()
        .set_title("选择状态目录：不要使用新目录恢复旧任务")
        .pick_folder()
    {
        report(
            ui,
            display_path(&path).map(|path| ui.set_state_directory(path)),
        );
    }
}

fn pick_ca(ui: &PrintWindow) {
    if let Some(path) = rfd::FileDialog::new()
        .set_title("选择 PEM CA 证书")
        .add_filter("PEM 证书", &["pem", "crt", "cer"])
        .pick_file()
    {
        report(ui, display_path(&path).map(|path| ui.set_ca_path(path)));
    }
}

pub(super) fn bind(ui: &PrintWindow) {
    let weak = ui.as_weak();
    ui.on_browse_config(move |save| {
        if let Some(ui) = weak.upgrade() {
            pick_config(&ui, save);
        }
    });
    let weak = ui.as_weak();
    ui.on_browse_state(move || {
        if let Some(ui) = weak.upgrade() {
            pick_state(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_browse_ca(move || {
        if let Some(ui) = weak.upgrade() {
            pick_ca(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_open_config_directory(move || {
        if let Some(ui) = weak.upgrade() {
            report(
                &ui,
                open_directory(Path::new(ui.get_config_path().as_str())),
            );
        }
    });
    let weak = ui.as_weak();
    ui.on_open_queue(move || {
        if let Some(ui) = weak.upgrade() {
            report(&ui, open_queue(ui.get_queue_name().as_str()));
        }
    });
    let weak = ui.as_weak();
    ui.on_copy_details(move || {
        if let Some(ui) = weak.upgrade() {
            let result =
                arboard::Clipboard::new()
                    .context(ClipboardSnafu)
                    .and_then(|mut clipboard| {
                        clipboard
                            .set_text(ui.get_diagnostic().to_string())
                            .context(ClipboardSnafu)
                    });
            match result {
                Ok(()) => super::view::toast(&ui, "诊断信息已复制，不含配对码、令牌或文档内容。"),
                Err(error) => report(&ui, Err(error)),
            }
        }
    });
}
