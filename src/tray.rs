//! System-tray icon shared by the Windows config panel and the Linux
//! overlay. Lives for the process lifetime; `try_recv_event` is polled
//! from the eframe update loop.

use anyhow::{Context, Result};
use std::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum TrayEvent {
    Show,
    Exit,
}

#[allow(dead_code)]
pub struct Tray {
    icon: TrayIcon,
    rx: mpsc::Receiver<TrayEvent>,
}

impl Tray {
    pub fn new(ctx: egui::Context) -> Result<Self> {
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

        let show_id = show.id().clone();
        let exit_id = exit.id().clone();
        let (tx, rx) = mpsc::channel();

        // The tray crate publishes events on global crossbeam channels.
        // eframe stops calling update() when the window is hidden, so
        // polling from the update loop would miss every event between
        // hide and the next show. This worker thread forwards events
        // to our mpsc and wakes the egui context so update() runs.
        std::thread::spawn(move || loop {
            let mut woke = false;
            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                let restore = matches!(
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
                if restore && tx.send(TrayEvent::Show).is_ok() {
                    woke = true;
                }
            }
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                let mapped = if ev.id == show_id {
                    Some(TrayEvent::Show)
                } else if ev.id == exit_id {
                    Some(TrayEvent::Exit)
                } else {
                    None
                };
                if let Some(e) = mapped {
                    if tx.send(e).is_ok() {
                        woke = true;
                    }
                }
            }
            if woke {
                ctx.request_repaint();
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        });

        Ok(Self { icon: tray, rx })
    }

    pub fn try_recv_event(&self) -> Option<TrayEvent> {
        self.rx.try_recv().ok()
    }
}
