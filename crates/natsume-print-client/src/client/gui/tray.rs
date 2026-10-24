use super::PrintWindow;
use crate::client::error::{GuiSnafu, IconSnafu, MenuSnafu, Result, TraySnafu};
use slint::ComponentHandle as _;
use snafu::ResultExt as _;
use std::{cell::RefCell, rc::Rc, time::Duration};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

/// Owned and polled only by the window event-loop thread.
pub(super) struct Tray {
    icon: Rc<RefCell<Option<TrayIcon>>>,
    open: MenuItem,
    pause: MenuItem,
    resume: MenuItem,
    exit: MenuItem,
    displayed: RefCell<Option<(u8, String)>>,
}

fn image(state: u8) -> Result<Icon> {
    let color = match state {
        1 => [17, 17, 17, 255],
        2 => [212, 20, 28, 255],
        _ => [119, 119, 119, 255],
    };
    let mut pixels = [255, 255, 255, 255].repeat(16 * 16);
    for y in 2..14 {
        for x in 2..14 {
            let offset = (y * 16 + x) * 4;
            pixels[offset..offset + 4].copy_from_slice(&color);
        }
    }
    Icon::from_rgba(pixels, 16, 16).context(IconSnafu)
}

impl Tray {
    pub fn new(ui: &PrintWindow) -> Result<Self> {
        let menu = Menu::new();
        let open = MenuItem::new("打开主窗口", true, None);
        let pause = MenuItem::new("暂停领取", true, None);
        let resume = MenuItem::new("恢复领取", true, None);
        let exit = MenuItem::new("正常退出…", true, None);
        menu.append_items(&[&open, &pause, &resume, &exit])
            .context(MenuSnafu)?;
        let image = image(0)?;
        let icon = Rc::new(RefCell::new(None));
        let owner = icon.clone();
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::ZERO, move || {
            match TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_icon(image)
                .with_tooltip("Natsume 打印客户端")
                .build()
                .context(TraySnafu)
            {
                Ok(tray) => {
                    *owner.borrow_mut() = Some(tray);
                }
                Err(error) => {
                    tracing::error!(%error, "tray unavailable; keep the client window open");
                    if let Some(ui) = weak.upgrade() {
                        super::view::operation_error(&ui, &error.to_string());
                    }
                }
            }
        });
        Ok(Self {
            icon,
            open,
            pause,
            resume,
            exit,
            displayed: RefCell::new(None),
        })
    }

    pub fn available(&self) -> bool {
        self.icon.borrow().is_some()
    }

    fn update(&self, ui: &PrintWindow) -> Result<()> {
        let state = if ui.get_attention() || ui.get_notice_tone() > 0 {
            2
        } else {
            u8::from(ui.get_can_control() && !ui.get_paused() && !ui.get_closing())
        };
        let tooltip = format!(
            "Natsume · {}{}",
            ui.get_claim_title(),
            if ui.get_task_present() {
                format!(" · 任务 #{}", ui.get_task_sequence())
            } else {
                String::new()
            }
        );
        if self
            .displayed
            .borrow()
            .as_ref()
            .is_some_and(|(saved, text)| *saved == state && text == &tooltip)
        {
            return Ok(());
        }
        if let Some(icon) = self.icon.borrow().as_ref() {
            icon.set_icon(Some(image(state)?)).context(TraySnafu)?;
            icon.set_tooltip(Some(&tooltip)).context(TraySnafu)?;
            *self.displayed.borrow_mut() = Some((state, tooltip));
        }
        Ok(())
    }

    pub fn poll(&self, ui: &PrintWindow) {
        self.pause
            .set_enabled(ui.get_can_control() && !ui.get_paused() && !ui.get_closing());
        self.resume
            .set_enabled(ui.get_can_control() && ui.get_paused() && !ui.get_closing());
        self.exit.set_enabled(!ui.get_closing());
        if let Err(error) = self.update(ui) {
            tracing::warn!(%error, "could not update tray status");
        }
        for _ in 0..32 {
            let Ok(event) = MenuEvent::receiver().try_recv() else {
                break;
            };
            if event.id == *self.open.id() {
                if let Err(error) = ui.show().context(GuiSnafu) {
                    tracing::warn!(%error, "could not show client window");
                }
            } else if event.id == *self.pause.id() {
                ui.invoke_pause_client();
            } else if event.id == *self.resume.id() {
                ui.invoke_resume_client();
            } else if event.id == *self.exit.id() {
                ui.invoke_request_exit();
            }
        }
    }
}
