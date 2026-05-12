//! System-tray icon shared by the Windows config panel and the Linux
//! overlay. Lives for the process lifetime; `try_recv_event` is polled
//! from the eframe update loop.

use anyhow::{Context, Result};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum TrayEvent {
    Show,
    Exit,
}

#[allow(dead_code)]
pub struct Tray {
    icon: TrayIcon,
    show_id: MenuId,
    exit_id: MenuId,
}

impl Tray {
    pub fn new() -> Result<Self> {
        let icon_data = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
            .context("decode tray icon")?;
        let icon = tray_icon::Icon::from_rgba(icon_data.rgba, icon_data.width, icon_data.height)
            .context("build tray icon")?;

        let menu = Menu::new();
        let show = MenuItem::new("Show", true, None);
        let exit = MenuItem::new("Exit", true, None);
        menu.append(&show).context("tray menu: Show")?;
        menu.append(&exit).context("tray menu: Exit")?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("Nicotine")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()
            .context("build tray icon")?;

        Ok(Self {
            icon: tray,
            show_id: show.id().clone(),
            exit_id: exit.id().clone(),
        })
    }

    pub fn try_recv_event(&self) -> Option<TrayEvent> {
        // Left-click → Show. Right-click is reserved for the context
        // menu (handled internally by tray-icon).
        if let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            let show = matches!(
                ev,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if show {
                return Some(TrayEvent::Show);
            }
        }
        if let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.show_id {
                return Some(TrayEvent::Show);
            }
            if ev.id == self.exit_id {
                return Some(TrayEvent::Exit);
            }
        }
        None
    }
}
