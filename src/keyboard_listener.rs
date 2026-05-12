use crate::config::Config;
use crate::cycle_state::CycleState;
use crate::window_manager::WindowManager;
use anyhow::{Context, Result};
use evdev::{Device, InputEventKind, Key};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub struct KeyboardListener {
    config: Config,
}

impl KeyboardListener {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Find keyboard device by looking for devices with standard keyboard keys
    fn find_keyboard_device(configured_path: Option<&str>) -> Result<Device> {
        if let Some(path_str) = configured_path {
            let path = Path::new(path_str);
            match Device::open(path) {
                Ok(device) => {
                    println!(
                        "Using configured keyboard device {} ({})",
                        device.name().unwrap_or("Unknown"),
                        path.display()
                    );
                    return Ok(device);
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to open configured keyboard device '{}': {}",
                        path_str, e
                    );
                    eprintln!("Falling back to automatic device detection...");
                }
            }
        }

        let devices_path = Path::new("/dev/input");
        for entry in std::fs::read_dir(devices_path)? {
            let entry = entry?;
            let path = entry.path();

            if let Some(filename) = path.file_name() {
                if let Some(name) = filename.to_str() {
                    if name.starts_with("event") {
                        if let Ok(device) = Device::open(&path) {
                            if device.supported_keys().is_some_and(|keys| {
                                keys.contains(Key::KEY_TAB)
                                    || keys.contains(Key::KEY_LEFTSHIFT)
                                    || keys.contains(Key::KEY_Z)
                            }) {
                                println!(
                                    "Found keyboard device: {} ({})",
                                    device.name().unwrap_or("Unknown"),
                                    path.display()
                                );
                                return Ok(device);
                            }
                        }
                    }
                }
            }
        }

        anyhow::bail!("No keyboard device found in /dev/input")
    }

    pub fn spawn(
        &self,
        wm: Arc<dyn WindowManager>,
        state: Arc<Mutex<CycleState>>,
    ) -> Result<std::thread::JoinHandle<()>> {
        if !self.config.enable_keyboard_buttons
            && !self.config.enable_wheel_cycle
            && self.config.preview_toggle_key == 0
        {
            anyhow::bail!("Keyboard listener not needed");
        }

        let dispatch_cycles = self.config.enable_keyboard_buttons;
        let forward_key = self.config.forward_key;
        let backward_key = self.config.backward_key;
        let modifier_key = self.config.modifier_key;
        let toggle_key = self.config.preview_toggle_key;
        let toggle_modifier = self.config.preview_toggle_modifier;
        let keyboard_device_path = self.config.keyboard_device_path.clone();
        let minimize_inactive = self.config.minimize_inactive;

        let handle = std::thread::spawn(move || {
            match Self::run_listener(
                wm,
                state,
                dispatch_cycles,
                forward_key,
                backward_key,
                modifier_key,
                toggle_key,
                toggle_modifier,
                keyboard_device_path,
                minimize_inactive,
            ) {
                Ok(_) => println!("Keyboard listener stopped"),
                Err(e) => println!("Keyboard listener error: {}", e),
            }
        });

        Ok(handle)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_listener(
        wm: Arc<dyn WindowManager>,
        state: Arc<Mutex<CycleState>>,
        dispatch_cycles: bool,
        forward_key: u16,
        backward_key: u16,
        modifier_key: Option<u16>,
        toggle_key: u16,
        toggle_modifier: Option<u16>,
        keyboard_device_path: Option<String>,
        minimize_inactive: bool,
    ) -> Result<()> {
        let mut device = Self::find_keyboard_device(keyboard_device_path.as_deref()).context(
            "Failed to find keyboard device. Make sure you have permission to read /dev/input/event*",
        )?;

        // DON'T grab the device - we only want to passively listen to events
        // Grabbing would prevent normal keyboard usage!

        println!(
            "Listening for keyboard keys: forward={} backward={} (dispatch_cycles={})",
            forward_key, backward_key, dispatch_cycles
        );
        let mut modifier_pressed = false;

        loop {
            for event in device.fetch_events()? {
                if let InputEventKind::Key(key) = event.kind() {
                    let code = key.code();
                    let value = event.value();

                    crate::linux_input_state::set_modifier_state(code, value != 0);

                    if let Some(mod_key) = modifier_key {
                        if code == mod_key {
                            modifier_pressed = value != 0;
                        }
                    }

                    if value == 0 {
                        continue;
                    }

                    if toggle_key != 0 && code == toggle_key {
                        let mod_ok = match toggle_modifier {
                            Some(vk) => crate::linux_input_state::modifier_held(vk),
                            None => true,
                        };
                        if mod_ok {
                            crate::toggle_state::PREVIEW_TOGGLE_COUNTER
                                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                            continue;
                        }
                    }

                    if !dispatch_cycles {
                        continue;
                    }

                    // Check modifier+backward first or forward_key == backward_key swallows it.
                    if code == backward_key && modifier_pressed {
                        println!("Backward + Modifier button pressed");
                        if let Err(e) = Self::cycle_backward(&wm, &state, minimize_inactive) {
                            eprintln!("Failed to cycle backward: {}", e);
                        }
                    } else if code == forward_key {
                        println!("Forward button pressed");
                        if let Err(e) = Self::cycle_forward(&wm, &state, minimize_inactive) {
                            eprintln!("Failed to cycle forward: {}", e);
                        }
                    } else if code == backward_key {
                        println!("Backward button pressed");
                        if let Err(e) = Self::cycle_backward(&wm, &state, minimize_inactive) {
                            eprintln!("Failed to cycle backward: {}", e);
                        }
                    }
                }
            }
        }
    }

    fn cycle_forward(
        wm: &Arc<dyn WindowManager>,
        state: &Arc<Mutex<CycleState>>,
        minimize_inactive: bool,
    ) -> Result<()> {
        let mut state = state.lock().unwrap();

        // Sync with active window first
        if let Ok(active) = wm.get_active_window() {
            state.sync_with_active(active);
        }

        state.cycle_forward(&**wm, minimize_inactive)?;
        Ok(())
    }

    fn cycle_backward(
        wm: &Arc<dyn WindowManager>,
        state: &Arc<Mutex<CycleState>>,
        minimize_inactive: bool,
    ) -> Result<()> {
        let mut state = state.lock().unwrap();

        // Sync with active window first
        if let Ok(active) = wm.get_active_window() {
            state.sync_with_active(active);
        }

        state.cycle_backward(&**wm, minimize_inactive)?;
        Ok(())
    }
}
