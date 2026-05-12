use crate::config::Config;
use crate::cycle_state::CycleState;
use crate::window_manager::WindowManager;
use anyhow::{Context, Result};
use evdev::{Device, InputEventKind, Key};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub struct MouseListener {
    config: Config,
}

impl MouseListener {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Find mouse device by looking for devices with BTN_SIDE or BTN_EXTRA capabilities
    /// Priority order: configured device name -> configured device path -> auto-detect
    fn find_mouse_device(
        configured_name: Option<&str>,
        configured_path: Option<&str>,
    ) -> Result<Device> {
        let devices_path = Path::new("/dev/input");

        // 1. Try configured device name first (highest priority)
        if let Some(device_name) = configured_name {
            println!("Searching for device by name: {}", device_name);

            for entry in std::fs::read_dir(devices_path)? {
                let entry = entry?;
                let path = entry.path();

                if let Some(filename) = path.file_name() {
                    if let Some(name) = filename.to_str() {
                        if name.starts_with("event") {
                            if let Ok(device) = Device::open(&path) {
                                if let Some(dev_name) = device.name() {
                                    if dev_name == device_name {
                                        println!(
                                            "Using configured mouse device by name: {} ({})",
                                            dev_name,
                                            path.display()
                                        );
                                        return Ok(device);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            eprintln!(
                "Warning: Failed to find device with name '{}'. Trying other methods...",
                device_name
            );
        }

        // 2. Try configured path second
        if let Some(path_str) = configured_path {
            let path = Path::new(path_str);
            match Device::open(path) {
                Ok(device) => {
                    println!(
                        "Using configured mouse device by path: {} ({})",
                        device.name().unwrap_or("Unknown"),
                        path.display()
                    );
                    return Ok(device);
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to open configured mouse device '{}': {}",
                        path_str, e
                    );
                    eprintln!("Falling back to automatic device detection...");
                }
            }
        }

        // 3. Fall back to automatic detection (lowest priority)
        for entry in std::fs::read_dir(devices_path)? {
            let entry = entry?;
            let path = entry.path();

            if let Some(filename) = path.file_name() {
                if let Some(name) = filename.to_str() {
                    if name.starts_with("event") {
                        if let Ok(device) = Device::open(&path) {
                            // Check if device has mouse side buttons
                            if device.supported_keys().is_some_and(|keys| {
                                keys.contains(Key::BTN_SIDE) || keys.contains(Key::BTN_EXTRA)
                            }) {
                                println!(
                                    "Found mouse device: {} ({})",
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

        anyhow::bail!("No mouse device with side buttons found in /dev/input")
    }

    /// Run the mouse event listener in a background thread
    pub fn spawn(
        &self,
        wm: Arc<dyn WindowManager>,
        state: Arc<Mutex<CycleState>>,
    ) -> Result<std::thread::JoinHandle<()>> {
        // `Never` mode bails; `Always` and `OnlyWhenEveFocused` both
        // run on Linux (the focus check isn't yet portable across
        // Wayland compositors).
        if self.config.mouse_cycle_mode == crate::config::MouseCycleMode::Never {
            anyhow::bail!("Mouse cycle mode is set to Never");
        }

        let forward_button = self.config.forward_button;
        let backward_button = self.config.backward_button;
        let mouse_device_name = self.config.mouse_device_name.clone();
        let mouse_device_path = self.config.mouse_device_path.clone();
        let minimize_inactive = self.config.minimize_inactive;

        let handle = std::thread::spawn(move || {
            match Self::run_listener(
                wm,
                state,
                forward_button,
                backward_button,
                mouse_device_name,
                mouse_device_path,
                minimize_inactive,
            ) {
                Ok(_) => println!("Mouse listener stopped"),
                Err(e) => eprintln!("Mouse listener error: {}", e),
            }
        });

        Ok(handle)
    }

    fn run_listener(
        wm: Arc<dyn WindowManager>,
        state: Arc<Mutex<CycleState>>,
        forward_button: u16,
        backward_button: u16,
        mouse_device_name: Option<String>,
        mouse_device_path: Option<String>,
        minimize_inactive: bool,
    ) -> Result<()> {
        let mut device = Self::find_mouse_device(
            mouse_device_name.as_deref(),
            mouse_device_path.as_deref(),
        )
        .context(
            "Failed to find mouse device. Make sure you have permission to read /dev/input/event*",
        )?;

        // DON'T grab the device - we only want to passively listen to events
        // Grabbing would prevent normal mouse usage!

        println!(
            "Listening for mouse buttons: forward={}, backward={}",
            forward_button, backward_button
        );

        loop {
            for event in device.fetch_events()? {
                if let InputEventKind::Key(key) = event.kind() {
                    let code = key.code();

                    // Only handle button press (value 1), ignore release (value 0)
                    if event.value() == 1 {
                        if code == forward_button {
                            println!("Forward button pressed");
                            if let Err(e) = Self::cycle_forward(&wm, &state, minimize_inactive) {
                                eprintln!("Failed to cycle forward: {}", e);
                            }
                        } else if code == backward_button {
                            println!("Backward button pressed");
                            if let Err(e) = Self::cycle_backward(&wm, &state, minimize_inactive) {
                                eprintln!("Failed to cycle backward: {}", e);
                            }
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
