use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A single per-character hotkey binding. `vk` is a Win32 Virtual-Key
/// code (or evdev code on Linux); `modifier` is an optional second VK
/// that must be held down (typically Shift/Ctrl/Alt).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct CharacterHotkey {
    pub vk: u16,
    #[serde(default)]
    pub modifier: Option<u16>,
}

/// Zero width or height is treated as "no override" by `preview_size_for`.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub struct PreviewSize {
    pub width: u32,
    pub height: u32,
}

/// `Never` keeps the LL hook installed so the config panel can still
/// capture XBUTTON for binding; it just suppresses dispatch.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum MouseCycleMode {
    Always,
    Never,
    OnlyWhenEveFocused,
}

impl MouseCycleMode {
    #[cfg_attr(unix, allow(dead_code))]
    pub fn to_u8(self) -> u8 {
        match self {
            MouseCycleMode::Never => 0,
            MouseCycleMode::Always => 1,
            MouseCycleMode::OnlyWhenEveFocused => 2,
        }
    }

    #[cfg_attr(unix, allow(dead_code))]
    pub fn from_u8(v: u8) -> MouseCycleMode {
        match v {
            0 => MouseCycleMode::Never,
            2 => MouseCycleMode::OnlyWhenEveFocused,
            _ => MouseCycleMode::Always,
        }
    }
}

/// How the visible-at-a-glance view of clients is rendered.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// One DWM thumbnail window per EVE client (default).
    Previews,
    /// A single always-on-top window listing each character name. Active
    /// character shown in Nicotine red with a 🚬 marker.
    List,
}

/// Settings that components watch for *live* changes — e.g. the preview
/// manager resizes windows as soon as these change, without waiting for a
/// save-to-disk + hot-reload cycle. Shared via Arc<Mutex<>> between the
/// config panel (writer) and the preview manager (reader).
// LiveSettings fields are read by the Windows preview manager only.
// On Linux they're allocated and written by `from_config` but never
// read, so suppress the unused-field lint there.
#[derive(Debug, Clone)]
#[cfg_attr(unix, allow(dead_code))]
pub struct LiveSettings {
    pub preview_width: u32,
    pub preview_height: u32,
    pub display_mode: DisplayMode,
    /// When true, both preview windows and the client-list window
    /// ignore mouse drags so they can't accidentally be knocked out of
    /// position mid-game. Click-to-activate still works on previews.
    pub positions_locked: bool,
    /// Per-character preview-size overrides. Keyed by EVE character
    /// name to survive list reorders and renames, mirroring the
    /// `character_hotkeys` map.
    pub preview_size_overrides: HashMap<String, PreviewSize>,
    pub smart_hide_enabled: bool,
    pub preview_opacity: u8,
    pub preview_hover_opacity: u8,
    pub previews_interactive: bool,
}

impl LiveSettings {
    pub fn from_config(config: &Config) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            preview_width: config.preview_width,
            preview_height: config.preview_height,
            display_mode: config.display_mode,
            positions_locked: config.positions_locked,
            preview_size_overrides: config.preview_size_overrides.clone(),
            smart_hide_enabled: config.smart_hide_enabled,
            preview_opacity: config.preview_opacity,
            preview_hover_opacity: config.preview_hover_opacity,
            previews_interactive: config.previews_interactive,
        }))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub display_width: u32,
    pub display_height: u32,
    pub panel_height: u32,
    pub eve_width: u32,
    pub eve_height: u32,
    pub overlay_x: f32,
    pub overlay_y: f32,
    #[serde(default = "default_mouse_cycle_mode")]
    pub mouse_cycle_mode: MouseCycleMode,
    #[serde(default = "default_forward_button")]
    pub forward_button: u16, // BTN_SIDE (mouse button 9)
    #[serde(default = "default_backward_button")]
    pub backward_button: u16, // BTN_EXTRA (mouse button 8)
    #[serde(default = "default_enable_keyboard")]
    pub enable_keyboard_buttons: bool,
    #[serde(default = "default_forward_key")]
    pub forward_key: u16, // KEY_TAB (15) - Tab for forward, Shift+Tab for backward
    #[serde(default = "default_backward_key")]
    pub backward_key: u16, // KEY_TAB (15) - Track SHIFT modifier internally
    #[serde(default = "default_show_overlay")]
    pub show_overlay: bool,
    #[serde(default = "default_mouse_device_name")]
    pub mouse_device_name: Option<String>,
    #[serde(default = "default_mouse_device_path")]
    pub mouse_device_path: Option<String>,
    #[serde(default = "default_minimize_inactive")]
    pub minimize_inactive: bool,
    #[serde(default = "default_keyboard_device_path")]
    pub keyboard_device_path: Option<String>,
    #[serde(default = "default_modifier_key")]
    pub modifier_key: Option<u16>,
    #[serde(default = "default_preview_width")]
    pub preview_width: u32,
    #[serde(default = "default_preview_height")]
    pub preview_height: u32,
    #[serde(default = "default_show_previews")]
    pub show_previews: bool,
    /// `switch N` maps target N to entry N-1. Empty list = detection order.
    #[serde(default)]
    pub characters: Vec<String>,
    #[serde(default = "default_display_mode")]
    pub display_mode: DisplayMode,
    #[serde(default)]
    pub positions_locked: bool,
    /// Keyed by character name so bindings survive list reorders.
    #[serde(default)]
    pub character_hotkeys: HashMap<String, CharacterHotkey>,
    #[serde(default)]
    pub preview_size_overrides: HashMap<String, PreviewSize>,
    /// 0 = unbound.
    #[serde(default)]
    pub preview_toggle_key: u16,
    #[serde(default)]
    pub preview_toggle_modifier: Option<u16>,
    #[serde(default)]
    pub smart_hide_enabled: bool,
    /// 0..=255; on Windows applied via WS_EX_LAYERED.
    #[serde(default = "default_preview_opacity")]
    pub preview_opacity: u8,
    #[serde(default = "default_preview_hover_opacity")]
    pub preview_hover_opacity: u8,
    /// false makes previews click-through (Windows: WS_EX_TRANSPARENT;
    /// Linux: overlay passthrough via winit set_cursor_hittest).
    #[serde(default = "default_previews_interactive")]
    pub previews_interactive: bool,
    #[serde(default)]
    pub enable_wheel_cycle: bool,
    #[serde(default = "default_wheel_modifier")]
    pub wheel_cycle_modifier: u16,
    #[serde(default = "default_config_panel_width")]
    pub config_panel_width: u32,
    #[serde(default = "default_config_panel_height")]
    pub config_panel_height: u32,
}

#[cfg(unix)]
fn default_mouse_cycle_mode() -> MouseCycleMode {
    MouseCycleMode::Always
}

// Windows: most users remap their side buttons via Logi Options+ etc.,
// so we'd hijack the buttons under their nose if we defaulted to Always.
#[cfg(windows)]
fn default_mouse_cycle_mode() -> MouseCycleMode {
    MouseCycleMode::Never
}

#[cfg(unix)]
fn default_forward_button() -> u16 {
    276 // BTN_SIDE (forward button, mouse button 9) — evdev code
}

#[cfg(windows)]
fn default_forward_button() -> u16 {
    2 // XBUTTON2 (forward side button)
}

#[cfg(unix)]
fn default_backward_button() -> u16 {
    275 // BTN_EXTRA (backward button, mouse button 8) — evdev code
}

#[cfg(windows)]
fn default_backward_button() -> u16 {
    1 // XBUTTON1 (backward side button)
}

#[cfg(unix)]
fn default_enable_keyboard() -> bool {
    false // Disabled by default to avoid conflicts with games that use Tab
}

#[cfg(windows)]
fn default_enable_keyboard() -> bool {
    true // F10/F11 are uncommon enough to enable by default for cycling
}

#[cfg(unix)]
fn default_forward_key() -> u16 {
    15 // KEY_TAB — evdev code
}

#[cfg(windows)]
fn default_forward_key() -> u16 {
    0x7A // VK_F11
}

#[cfg(unix)]
fn default_backward_key() -> u16 {
    15 // KEY_TAB (Modifier applied if set) — evdev code
}

#[cfg(windows)]
fn default_backward_key() -> u16 {
    0x79 // VK_F10
}

fn default_show_overlay() -> bool {
    true
}

fn default_mouse_device_name() -> Option<String> {
    None
}

fn default_mouse_device_path() -> Option<String> {
    None
}

fn default_minimize_inactive() -> bool {
    false
}

fn default_keyboard_device_path() -> Option<String> {
    None
}

fn default_modifier_key() -> Option<u16> {
    None // No modifier for backward shifting by default
}

fn default_preview_width() -> u32 {
    320
}

fn default_preview_height() -> u32 {
    180
}

fn default_show_previews() -> bool {
    true
}

fn default_display_mode() -> DisplayMode {
    DisplayMode::Previews
}

fn default_preview_opacity() -> u8 {
    230
}

fn default_preview_hover_opacity() -> u8 {
    255
}

fn default_previews_interactive() -> bool {
    true
}

fn default_wheel_modifier() -> u16 {
    0x10 // VK_SHIFT
}

fn default_config_panel_width() -> u32 {
    700
}

fn default_config_panel_height() -> u32 {
    620
}

impl Config {
    fn config_dir() -> PathBuf {
        let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("nicotine");
        path
    }

    fn config_path() -> PathBuf {
        let mut path = Self::config_dir();
        path.push("config.toml");
        path
    }

    /// Persist the current Config back to disk. Used by the config panel
    /// to commit user edits. Only called from the Windows config panel,
    /// hence the dead-code allow on Linux.
    #[cfg_attr(unix, allow(dead_code))]
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path();
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&config_path, contents).context("Failed to write config.toml")?;
        Ok(())
    }

    #[cfg(unix)]
    fn detect_display_size() -> (u32, u32) {
        if let Ok(output) = std::process::Command::new("xrandr")
            .args(["--current"])
            .output()
        {
            if let Ok(stdout) = String::from_utf8(output.stdout) {
                for line in stdout.lines() {
                    if line.contains("*") && line.contains("x") {
                        // Parse line like: "7680x2160     60.00*+"
                        if let Some(resolution) = line.split_whitespace().next() {
                            if let Some((w, h)) = resolution.split_once('x') {
                                if let (Ok(width), Ok(height)) = (w.parse(), h.parse()) {
                                    return (width, height);
                                }
                            }
                        }
                    }
                }
            }
        }
        (1920, 1080)
    }

    #[cfg(windows)]
    fn detect_display_size() -> (u32, u32) {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if w > 0 && h > 0 {
            (w as u32, h as u32)
        } else {
            (1920, 1080)
        }
    }

    fn build_default(display_width: u32, display_height: u32) -> Self {
        Self {
            display_width,
            display_height,
            panel_height: 0,
            eve_width: (display_width as f32 * 0.54) as u32, // ~54% of width
            eve_height: display_height,
            overlay_x: 10.0,
            overlay_y: 10.0,
            mouse_cycle_mode: default_mouse_cycle_mode(),
            forward_button: default_forward_button(),
            backward_button: default_backward_button(),
            enable_keyboard_buttons: default_enable_keyboard(),
            forward_key: default_forward_key(),
            backward_key: default_backward_key(),
            show_overlay: default_show_overlay(),
            mouse_device_name: default_mouse_device_name(),
            mouse_device_path: default_mouse_device_path(),
            minimize_inactive: default_minimize_inactive(),
            keyboard_device_path: default_keyboard_device_path(),
            modifier_key: default_modifier_key(),
            preview_width: default_preview_width(),
            preview_height: default_preview_height(),
            show_previews: default_show_previews(),
            characters: Vec::new(),
            display_mode: default_display_mode(),
            positions_locked: false,
            character_hotkeys: HashMap::new(),
            preview_size_overrides: HashMap::new(),
            preview_toggle_key: 0,
            preview_toggle_modifier: None,
            smart_hide_enabled: false,
            preview_opacity: default_preview_opacity(),
            preview_hover_opacity: default_preview_hover_opacity(),
            previews_interactive: default_previews_interactive(),
            enable_wheel_cycle: false,
            wheel_cycle_modifier: default_wheel_modifier(),
            config_panel_width: default_config_panel_width(),
            config_panel_height: default_config_panel_height(),
        }
    }

    pub fn load() -> Result<Self> {
        let config_path = Self::config_path();

        if let Ok(contents) = fs::read_to_string(&config_path) {
            return toml::from_str(&contents).context("Failed to parse config.toml");
        }

        println!("Generating config based on your display...");
        let (display_width, display_height) = Self::detect_display_size();
        println!("Detected display: {}x{}", display_width, display_height);

        let config = Self::build_default(display_width, display_height);

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(&config)?;
        fs::write(&config_path, contents)?;
        println!("Created config: {}", config_path.display());
        println!("Edit it to customize window sizes and positions");

        Ok(config)
    }

    pub fn save_default() -> Result<()> {
        let config_path = Self::config_path();
        let (display_width, display_height) = Self::detect_display_size();

        let config = Self::build_default(display_width, display_height);

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(&config)?;
        fs::write(&config_path, contents)?;
        println!("Created config: {}", config_path.display());
        Ok(())
    }

    pub fn eve_height_adjusted(&self) -> u32 {
        self.display_height - self.panel_height
    }

    #[cfg_attr(unix, allow(dead_code))]
    pub fn preview_size_for(&self, character: &str) -> (u32, u32) {
        if let Some(ovr) = self.preview_size_overrides.get(character) {
            if ovr.width != 0 && ovr.height != 0 {
                return (ovr.width, ovr.height);
            }
        }
        (self.preview_width, self.preview_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eve_height_adjusted_with_panel() {
        let config = Config {
            display_width: 1920,
            display_height: 1080,
            panel_height: 40,
            eve_width: 1000,
            eve_height: 1080,
            overlay_x: 10.0,
            overlay_y: 10.0,
            mouse_cycle_mode: MouseCycleMode::Always,
            forward_button: 276,
            backward_button: 275,
            enable_keyboard_buttons: false,
            forward_key: 15,
            backward_key: 15,
            show_overlay: true,
            mouse_device_name: None,
            mouse_device_path: None,
            minimize_inactive: false,
            keyboard_device_path: None,
            modifier_key: None,
            preview_width: 320,
            preview_height: 180,
            show_previews: true,
            characters: Vec::new(),
            display_mode: DisplayMode::Previews,
            positions_locked: false,
            character_hotkeys: HashMap::new(),
            preview_size_overrides: HashMap::new(),
            preview_toggle_key: 0,
            preview_toggle_modifier: None,
            smart_hide_enabled: false,
            preview_opacity: 230,
            preview_hover_opacity: 255,
            previews_interactive: true,
            enable_wheel_cycle: false,
            wheel_cycle_modifier: 0x10,
            config_panel_width: 700,
            config_panel_height: 620,
        };

        // Height should be: 1080 - 40 = 1040
        assert_eq!(config.eve_height_adjusted(), 1040);
    }

    #[test]
    fn test_eve_height_adjusted_without_panel() {
        let config = Config {
            display_width: 1920,
            display_height: 1080,
            panel_height: 0,
            eve_width: 1000,
            eve_height: 1080,
            overlay_x: 10.0,
            overlay_y: 10.0,
            mouse_cycle_mode: MouseCycleMode::Always,
            forward_button: 276,
            backward_button: 275,
            enable_keyboard_buttons: false,
            forward_key: 15,
            backward_key: 15,
            show_overlay: true,
            mouse_device_name: None,
            mouse_device_path: None,
            minimize_inactive: false,
            keyboard_device_path: None,
            modifier_key: None,
            preview_width: 320,
            preview_height: 180,
            show_previews: true,
            characters: Vec::new(),
            display_mode: DisplayMode::Previews,
            positions_locked: false,
            character_hotkeys: HashMap::new(),
            preview_size_overrides: HashMap::new(),
            preview_toggle_key: 0,
            preview_toggle_modifier: None,
            smart_hide_enabled: false,
            preview_opacity: 230,
            preview_hover_opacity: 255,
            previews_interactive: true,
            enable_wheel_cycle: false,
            wheel_cycle_modifier: 0x10,
            config_panel_width: 700,
            config_panel_height: 620,
        };

        assert_eq!(config.eve_height_adjusted(), 1080);
    }

    #[test]
    fn test_config_serialization() {
        let config = Config {
            display_width: 7680,
            display_height: 2160,
            panel_height: 0,
            eve_width: 4147,
            eve_height: 2160,
            overlay_x: 10.0,
            overlay_y: 10.0,
            mouse_cycle_mode: MouseCycleMode::Always,
            forward_button: 276,
            backward_button: 275,
            enable_keyboard_buttons: false,
            forward_key: 15,
            backward_key: 15,
            show_overlay: true,
            mouse_device_name: None,
            mouse_device_path: None,
            minimize_inactive: false,
            keyboard_device_path: None,
            modifier_key: None,
            preview_width: 320,
            preview_height: 180,
            show_previews: true,
            characters: Vec::new(),
            display_mode: DisplayMode::Previews,
            positions_locked: false,
            character_hotkeys: HashMap::new(),
            preview_size_overrides: HashMap::new(),
            preview_toggle_key: 0,
            preview_toggle_modifier: None,
            smart_hide_enabled: false,
            preview_opacity: 230,
            preview_hover_opacity: 255,
            previews_interactive: true,
            enable_wheel_cycle: false,
            wheel_cycle_modifier: 0x10,
            config_panel_width: 700,
            config_panel_height: 620,
        };

        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&toml_str).unwrap();

        assert_eq!(deserialized.display_width, 7680);
        assert_eq!(deserialized.display_height, 2160);
        assert_eq!(deserialized.eve_width, 4147);
    }
}
