//! User-scope auto-start at login. Windows uses
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`; Linux writes
//! a `~/.config/autostart/nicotine.desktop`. Both are no-admin,
//! per-user installs.
//!
//! Invoked only from the Windows config panel today; Linux users edit
//! the desktop file by hand or use their DE's autostart UI.
#![cfg_attr(unix, allow(dead_code))]

use anyhow::{Context, Result};

const APP_NAME: &str = "Nicotine";

pub fn set_enabled(enabled: bool) -> Result<()> {
    if enabled {
        let exe = std::env::current_exe().context("locate current exe")?;
        write(&exe)
    } else {
        clear()
    }
}

#[cfg(windows)]
fn write(exe: &std::path::Path) -> Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let value = format!("\"{}\" start --autostart", exe.display());
    let value_w: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let subkey: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Run"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let name: Vec<u16> = APP_NAME.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut hkey = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()
        .context("open Run key")?;
        let bytes: &[u8] = std::slice::from_raw_parts(
            value_w.as_ptr() as *const u8,
            value_w.len() * std::mem::size_of::<u16>(),
        );
        let rc = RegSetValueExW(hkey, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);
        rc.ok().context("write Run value")?;
    }
    Ok(())
}

#[cfg(windows)]
fn clear() -> Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    };

    let subkey: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Run"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let name: Vec<u16> = APP_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_WRITE,
            &mut hkey,
        )
        .is_err()
        {
            return Ok(());
        }
        let _ = RegDeleteValueW(hkey, PCWSTR(name.as_ptr()));
        let _ = RegCloseKey(hkey);
    }
    Ok(())
}

#[cfg(unix)]
fn write(exe: &std::path::Path) -> Result<()> {
    let path = desktop_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create autostart dir")?;
    }
    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Nicotine\n\
         Exec={} start --autostart\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))
}

#[cfg(unix)]
fn clear() -> Result<()> {
    let path = desktop_path()?;
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[cfg(unix)]
fn desktop_path() -> Result<std::path::PathBuf> {
    let mut p = dirs::config_dir().context("locate config dir")?;
    p.push("autostart");
    p.push("nicotine.desktop");
    Ok(p)
}
