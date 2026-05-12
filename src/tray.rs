//! Windows system tray. Owns its own worker thread that handles both
//! menu actions and left-click directly via Win32 — eframe pauses its
//! event loop while the viewport is hidden, so anything routed through
//! `update()` would queue forever.

use anyhow::{Context, Result};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE,
};

const WINDOW_TITLE: &str = "Nicotine";

#[allow(dead_code)]
pub struct Tray {
    icon: TrayIcon,
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
            // Defaults to true on Windows — that captures left-clicks
            // into the context menu before our handler sees them.
            .with_menu_on_left_click(false)
            .build()
            .context("build tray icon")?;

        let show_id = show.id().clone();
        let exit_id = exit.id().clone();

        std::thread::spawn(move || loop {
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
                if restore {
                    show_window();
                }
            }
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                if ev.id == show_id {
                    show_window();
                } else if ev.id == exit_id {
                    std::process::exit(0);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        });

        Ok(Self { icon: tray })
    }

    pub fn hide_window(&self) {
        hide_window();
    }
}

fn find_window() -> Option<windows::Win32::Foundation::HWND> {
    let title: Vec<u16> = WINDOW_TITLE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        match FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) {
            Ok(h) if !h.0.is_null() => Some(h),
            _ => None,
        }
    }
}

fn show_window() {
    if let Some(hwnd) = find_window() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

fn hide_window() {
    if let Some(hwnd) = find_window() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}
