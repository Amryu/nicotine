use crate::config::Config;
use crate::cycle_state::CycleState;
use crate::window_manager::WindowManager;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL,
    MOD_SHIFT, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, HHOOK, MSG, MSLLHOOKSTRUCT,
    WH_MOUSE_LL, WM_HOTKEY, WM_MOUSEWHEEL, WM_USER, WM_XBUTTONDOWN,
};

const HOTKEY_FORWARD_ID: i32 = 1001;
const HOTKEY_BACKWARD_ID: i32 = 1002;
/// Toggles preview window visibility (sticky manual override). Distinct
/// ID so the message-loop dispatch can route it to the preview manager
/// rather than the cycle path.
const HOTKEY_PREVIEW_TOGGLE_ID: i32 = 1003;
/// Per-character hotkey IDs are assigned starting here, one per bound
/// character, in the order the config lists them. Separated from the
/// cycle IDs so the message dispatch can tell them apart by ID range.
const HOTKEY_CHARACTER_BASE: i32 = 2000;

/// Lookup from per-character hotkey ID → the character name to
/// activate on WM_HOTKEY. Rebuilt from scratch each time hotkeys are
/// registered so stale entries never leak between config changes.
static CHARACTER_HOTKEY_LOOKUP: OnceLock<Mutex<HashMap<i32, String>>> = OnceLock::new();

fn character_lookup() -> &'static Mutex<HashMap<i32, String>> {
    CHARACTER_HOTKEY_LOOKUP.get_or_init(|| Mutex::new(HashMap::new()))
}

const WM_USER_FORWARD: u32 = WM_USER + 1;
const WM_USER_BACKWARD: u32 = WM_USER + 2;
const WM_USER_PAUSE: u32 = WM_USER + 3;
const WM_USER_RESUME: u32 = WM_USER + 4;

/// Thread ID of the running input listener, exposed so the config
/// panel can PostThreadMessage pause/resume signals when the user is
/// binding keys. Zero means "no listener running."
pub static LISTENER_THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// When true, the listener ignores incoming WM_HOTKEY and posted
/// cycle messages. Used to suppress daemon action while the user is
/// rebinding keys in the config panel.
static LISTENER_PAUSED: AtomicBool = AtomicBool::new(false);

/// Packed `MouseCycleMode` checked per XBUTTON press. Hot-toggleable
/// without reinstalling the hook. 0=Never, 1=Always, 2=OnlyWhenEveFocused
/// — see `MouseCycleMode::to_u8`. We keep the hook installed even in
/// `Never` mode so the config panel can still capture XBUTTON presses
/// for binding.
static MOUSE_CYCLE_MODE: AtomicU8 = AtomicU8::new(0);

/// Hot-reload setter for MOUSE_CYCLE_MODE. Called from the daemon
/// config-watch thread so the user's choice takes effect within ~500ms.
pub fn set_mouse_cycle_mode(mode: crate::config::MouseCycleMode) {
    MOUSE_CYCLE_MODE.store(mode.to_u8(), Ordering::Release);
}

/// Whether holding the configured modifier + scrolling the mouse wheel
/// should cycle clients. Separate atomic from the mouse-cycle mode so
/// users can enable wheel cycling without enabling XBUTTON cycling
/// (or vice-versa).
static WHEEL_CYCLE_ENABLED: AtomicBool = AtomicBool::new(false);
/// Virtual-key code of the modifier required to engage wheel cycling.
/// Default VK_SHIFT (0x10). Stored as u16 in an AtomicU32 because
/// AtomicU16 only became stable in Rust 1.74 and we want flexibility.
static WHEEL_CYCLE_MODIFIER: AtomicU32 = AtomicU32::new(0x10);
/// Throttle the wheel-cycle dispatch to one event per WHEEL_DEBOUNCE_MS.
/// A scroll wheel emits many MOUSEWHEEL messages per detent on some
/// drivers; without this the user can blow through every client in a
/// single flick.
static WHEEL_LAST_FIRE_MS: AtomicU64 = AtomicU64::new(0);
const WHEEL_DEBOUNCE_MS: u64 = 80;

/// Hot-reload setters for the wheel-cycle config.
pub fn set_wheel_cycle_enabled(enabled: bool) {
    WHEEL_CYCLE_ENABLED.store(enabled, Ordering::Release);
}
pub fn set_wheel_cycle_modifier(vk: u16) {
    WHEEL_CYCLE_MODIFIER.store(vk as u32, Ordering::Release);
}

/// Ask the input listener to stop acting on hotkeys. This unregisters
/// its global hotkeys so the keys become available to the focused
/// window (the config panel) for capture. No-op if the listener isn't
/// running yet.
pub fn pause_hotkeys() {
    let tid = LISTENER_THREAD_ID.load(Ordering::Acquire);
    if tid == 0 {
        return;
    }
    unsafe {
        let _ = PostThreadMessageW(tid, WM_USER_PAUSE, WPARAM(0), LPARAM(0));
    }
}

/// Ask the input listener to resume. It will re-read the latest
/// config.toml and re-register hotkeys with whatever the user just
/// bound.
pub fn resume_hotkeys() {
    let tid = LISTENER_THREAD_ID.load(Ordering::Acquire);
    if tid == 0 {
        return;
    }
    unsafe {
        let _ = PostThreadMessageW(tid, WM_USER_RESUME, WPARAM(0), LPARAM(0));
    }
}

/// Static context the low-level mouse hook reads to decide which posted
/// message (if any) to send back to the listener thread on each x-button
/// click. The hook callback is `extern "system" fn` — it can't capture, so
/// state has to live in a global.
struct HookContext {
    forward_button: u16,
    backward_button: u16,
    listener_thread_id: u32,
}

static HOOK_CTX: OnceLock<HookContext> = OnceLock::new();

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let event = wparam.0 as u32;

    if event == WM_XBUTTONDOWN {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        // High word of mouseData identifies the X button: 1 = XBUTTON1 (back),
        // 2 = XBUTTON2 (forward).
        let xbutton = ((info.mouseData >> 16) & 0xFFFF) as u16;

        // Gate on the configured MouseCycleMode. Never -> pass through.
        // OnlyWhenEveFocused -> only dispatch when an EVE client owns
        // the foreground. Always -> proceed.
        let mode = crate::config::MouseCycleMode::from_u8(MOUSE_CYCLE_MODE.load(Ordering::Acquire));
        match mode {
            crate::config::MouseCycleMode::Never => {
                return CallNextHookEx(None, code, wparam, lparam);
            }
            crate::config::MouseCycleMode::OnlyWhenEveFocused => {
                if !crate::windows_manager::is_foreground_eve() {
                    return CallNextHookEx(None, code, wparam, lparam);
                }
            }
            crate::config::MouseCycleMode::Always => {}
        }

        if let Some(ctx) = HOOK_CTX.get() {
            let post = if xbutton == ctx.forward_button {
                Some(WM_USER_FORWARD)
            } else if xbutton == ctx.backward_button {
                Some(WM_USER_BACKWARD)
            } else {
                None
            };

            if let Some(msg) = post {
                // PostThreadMessageW returns false if the thread queue is
                // unavailable (e.g. listener has exited). Best-effort.
                let _ = PostThreadMessageW(ctx.listener_thread_id, msg, WPARAM(0), LPARAM(0));
            }
        }
    } else if event == WM_MOUSEWHEEL && WHEEL_CYCLE_ENABLED.load(Ordering::Acquire) {
        // Mirror the XBUTTON gating so a single mode dropdown governs
        // both side-button and wheel cycling.
        let mode = crate::config::MouseCycleMode::from_u8(MOUSE_CYCLE_MODE.load(Ordering::Acquire));
        let mode_ok = match mode {
            crate::config::MouseCycleMode::Never => false,
            crate::config::MouseCycleMode::OnlyWhenEveFocused => {
                crate::windows_manager::is_foreground_eve()
            }
            crate::config::MouseCycleMode::Always => true,
        };
        if !mode_ok {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        // Require the configured modifier to be held. GetAsyncKeyState's
        // high bit means "currently down" — this works inside the LL
        // hook callback regardless of which thread input was attached.
        let modifier_vk = WHEEL_CYCLE_MODIFIER.load(Ordering::Acquire) as i32;
        let down = GetAsyncKeyState(modifier_vk) as u16 & 0x8000 != 0;
        if !down {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        // Debounce — drivers can emit multiple wheel events per detent.
        // Compare using millis-since-arbitrary-epoch from SystemTime.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = WHEEL_LAST_FIRE_MS.load(Ordering::Acquire);
        if now_ms.saturating_sub(last) < WHEEL_DEBOUNCE_MS {
            // Still swallow the event so the focused app doesn't scroll
            // during the debounce window — otherwise a hard scroll
            // would cycle once and ALSO scroll the page underneath
            // until the cooldown elapses.
            return LRESULT(1);
        }
        WHEEL_LAST_FIRE_MS.store(now_ms, Ordering::Release);

        // High word of mouseData is the wheel delta (signed). Positive
        // = wheel up = forward (matches the EVE-O Preview convention).
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let delta = ((info.mouseData >> 16) & 0xFFFF) as i16;
        let msg = if delta > 0 {
            WM_USER_FORWARD
        } else {
            WM_USER_BACKWARD
        };
        if let Some(ctx) = HOOK_CTX.get() {
            let _ = PostThreadMessageW(ctx.listener_thread_id, msg, WPARAM(0), LPARAM(0));
        }
        // Return non-zero to swallow the wheel event so the focused
        // app's scroll doesn't also fire. Without this, holding Shift
        // and scrolling a browser would both cycle Nicotine AND scroll
        // the page horizontally (Chrome remaps Shift+Wheel that way).
        return LRESULT(1);
    }

    CallNextHookEx(None, code, wparam, lparam)
}

fn vk_to_modifier(vk: u16) -> HOT_KEY_MODIFIERS {
    match vk {
        v if v == VK_SHIFT.0 || v == VK_LSHIFT.0 || v == VK_RSHIFT.0 => MOD_SHIFT,
        v if v == VK_CONTROL.0 || v == VK_LCONTROL.0 || v == VK_RCONTROL.0 => MOD_CONTROL,
        v if v == VK_MENU.0 || v == VK_LMENU.0 || v == VK_RMENU.0 => MOD_ALT,
        _ => HOT_KEY_MODIFIERS(0),
    }
}

/// Spawn the Windows input listener thread. The thread installs a low-level
/// mouse hook for the configured side buttons and (optionally) registers
/// keyboard hotkeys, then runs a message pump that triggers cycle actions.
pub fn spawn(
    config: Config,
    wm: Arc<dyn WindowManager>,
    state: Arc<Mutex<CycleState>>,
) -> Result<JoinHandle<()>> {
    let handle = std::thread::spawn(move || {
        if let Err(e) = run_listener(config, wm, state) {
            eprintln!("Windows input listener exited with error: {}", e);
        }
    });
    Ok(handle)
}

fn run_listener(
    config: Config,
    wm: Arc<dyn WindowManager>,
    state: Arc<Mutex<CycleState>>,
) -> Result<()> {
    let listener_thread_id = unsafe { GetCurrentThreadId() };
    LISTENER_THREAD_ID.store(listener_thread_id, Ordering::Release);

    // Install the low-level mouse hook unconditionally — we need it
    // running even when mouse cycling is disabled so that the config
    // panel can still capture x-button presses for binding. The hook
    // gates actual cycle dispatch on MOUSE_CYCLE_MODE (below), so a
    // `Never`-configured user won't trigger cycling.
    let _ = HOOK_CTX.set(HookContext {
        forward_button: config.forward_button,
        backward_button: config.backward_button,
        listener_thread_id,
    });
    MOUSE_CYCLE_MODE.store(config.mouse_cycle_mode.to_u8(), Ordering::Release);
    WHEEL_CYCLE_ENABLED.store(config.enable_wheel_cycle, Ordering::Release);
    WHEEL_CYCLE_MODIFIER.store(config.wheel_cycle_modifier as u32, Ordering::Release);

    let module = unsafe { GetModuleHandleW(None) }.context("GetModuleHandleW failed")?;
    let _hook: HHOOK = unsafe {
        SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(mouse_hook_proc),
            Some(HINSTANCE(module.0)),
            0,
        )
    }
    .context("SetWindowsHookExW failed — check that the daemon process has UI access")?;
    println!("Mouse side-button hook installed");

    // Register keyboard hotkeys if enabled.
    register_hotkeys(&config);

    let mut msg = MSG::default();
    loop {
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if !got.as_bool() {
            // WM_QUIT or error — both terminate the loop.
            break;
        }

        // Config-panel binding mode: temporarily stop consuming hotkeys
        // so egui sees the user's next key press.
        if msg.message == WM_USER_PAUSE {
            LISTENER_PAUSED.store(true, Ordering::Release);
            unregister_hotkeys();
            continue;
        }
        if msg.message == WM_USER_RESUME {
            LISTENER_PAUSED.store(false, Ordering::Release);
            // Unregister first so this path is safe to call as a
            // generic "rebind" even when we weren't paused. Re-read
            // config.toml so the hotkeys the user just bound take
            // effect immediately.
            unregister_hotkeys();
            if let Ok(fresh) = Config::load() {
                register_hotkeys(&fresh);
            }
            continue;
        }
        // While paused, drop any cycle-triggering events — the mouse
        // hook can still fire XBUTTON posts, but we don't want the
        // daemon to act on them mid-capture.
        if LISTENER_PAUSED.load(Ordering::Acquire) {
            continue;
        }

        // Read config.minimize_inactive fresh each action so user
        // toggles apply without restart. One small file read, cheap.
        let minimize_inactive_lookup =
            || Config::load().map(|c| c.minimize_inactive).unwrap_or(false);

        // Preview-toggle hotkey lands here. Bump the global counter so
        // the preview-manager thread sees a fresh request on its next
        // reconcile tick. Done before the cycle match so we don't fall
        // through into the cycle paths for the same WM_HOTKEY message.
        if msg.message == WM_HOTKEY && msg.wParam.0 as i32 == HOTKEY_PREVIEW_TOGGLE_ID {
            crate::toggle_state::PREVIEW_TOGGLE_COUNTER.fetch_add(1, Ordering::AcqRel);
            continue;
        }

        // Cycle action?
        let cycle: Option<CycleDirection> = match msg.message {
            WM_USER_FORWARD => Some(CycleDirection::Forward),
            WM_USER_BACKWARD => Some(CycleDirection::Backward),
            WM_HOTKEY => match msg.wParam.0 as i32 {
                HOTKEY_FORWARD_ID => Some(CycleDirection::Forward),
                HOTKEY_BACKWARD_ID => Some(CycleDirection::Backward),
                _ => None,
            },
            _ => None,
        };
        if let Some(direction) = cycle {
            let minimize_inactive = minimize_inactive_lookup();
            if let Err(e) = perform_cycle(&wm, &state, direction, minimize_inactive) {
                eprintln!("Cycle action failed: {}", e);
            }
            continue;
        }

        // Per-character jump hotkey?
        if msg.message == WM_HOTKEY {
            let id = msg.wParam.0 as i32;
            if id >= HOTKEY_CHARACTER_BASE {
                let name = character_lookup().lock().unwrap().get(&id).cloned();
                if let Some(name) = name {
                    let minimize_inactive = minimize_inactive_lookup();
                    let new_active_id = {
                        let mut state_guard = state.lock().unwrap();
                        if let Ok(active) = wm.get_active_window() {
                            state_guard.sync_with_active(active);
                        }
                        if let Err(e) =
                            state_guard.switch_to_character(&name, &*wm, minimize_inactive)
                        {
                            eprintln!("Character switch failed: {}", e);
                        }
                        let windows = state_guard.get_windows();
                        let idx = state_guard.get_current_index();
                        windows.get(idx).map(|w| w.id)
                    };
                    if let Some(id) = new_active_id {
                        crate::preview_windows::notify_active_change(id);
                    }
                }
            }
        }
    }
    LISTENER_THREAD_ID.store(0, Ordering::Release);
    Ok(())
}

/// Register the configured forward / backward cycle hotkeys AND every
/// per-character hotkey, all on the current thread. Silently ignores
/// failures (another app may own the key) — the listener still runs,
/// it just won't fire for the contested key.
unsafe fn do_register_hotkeys(config: &Config) {
    // Cycle hotkeys — gated by enable_keyboard_buttons so users can
    // disable cycle hotkeys while still using per-character ones.
    if config.enable_keyboard_buttons {
        let _ = RegisterHotKey(
            None,
            HOTKEY_FORWARD_ID,
            HOT_KEY_MODIFIERS(0),
            config.forward_key as u32,
        );
        let modifier = config.modifier_key.map(vk_to_modifier);
        let backward_mod = if config.forward_key == config.backward_key {
            modifier.unwrap_or(HOT_KEY_MODIFIERS(0))
        } else {
            HOT_KEY_MODIFIERS(0)
        };
        if config.forward_key != config.backward_key || backward_mod.0 != 0 {
            let _ = RegisterHotKey(
                None,
                HOTKEY_BACKWARD_ID,
                backward_mod,
                config.backward_key as u32,
            );
        }
    }

    // Preview-toggle hotkey — independent of enable_keyboard_buttons so
    // a user who's mapped cycling to mouse buttons can still bind a
    // show/hide hotkey here.
    if config.preview_toggle_key != 0 {
        let modifier = config
            .preview_toggle_modifier
            .map(vk_to_modifier)
            .unwrap_or(HOT_KEY_MODIFIERS(0));
        if RegisterHotKey(
            None,
            HOTKEY_PREVIEW_TOGGLE_ID,
            modifier,
            config.preview_toggle_key as u32,
        )
        .is_err()
        {
            eprintln!("Failed to register preview-toggle hotkey (another app may own it)");
        }
    }

    // Per-character hotkeys — iterate the characters list in order so
    // hotkey IDs are stable for the same config, and populate the
    // ID → name lookup.
    let mut lookup = character_lookup().lock().unwrap();
    lookup.clear();
    let mut next_id = HOTKEY_CHARACTER_BASE;
    for name in &config.characters {
        let Some(hk) = config.character_hotkeys.get(name) else {
            continue;
        };
        // vk == 0 is a placeholder entry — the user picked a modifier
        // but hasn't captured a key yet. Skip registration; the entry
        // becomes active only once a real VK is bound.
        if hk.vk == 0 {
            continue;
        }
        let modifier = hk
            .modifier
            .map(vk_to_modifier)
            .unwrap_or(HOT_KEY_MODIFIERS(0));
        if RegisterHotKey(None, next_id, modifier, hk.vk as u32).is_ok() {
            lookup.insert(next_id, name.clone());
        } else {
            eprintln!(
                "Failed to register per-character hotkey for '{}' (another app may own it)",
                name
            );
        }
        next_id += 1;
    }
}

fn register_hotkeys(config: &Config) {
    unsafe { do_register_hotkeys(config) }
}

fn unregister_hotkeys() {
    unsafe {
        let _ = UnregisterHotKey(None, HOTKEY_FORWARD_ID);
        let _ = UnregisterHotKey(None, HOTKEY_BACKWARD_ID);
        let _ = UnregisterHotKey(None, HOTKEY_PREVIEW_TOGGLE_ID);
        let mut lookup = character_lookup().lock().unwrap();
        for id in lookup.keys() {
            let _ = UnregisterHotKey(None, *id);
        }
        lookup.clear();
    }
}

#[derive(Copy, Clone)]
enum CycleDirection {
    Forward,
    Backward,
}

fn perform_cycle(
    wm: &Arc<dyn WindowManager>,
    state: &Arc<Mutex<CycleState>>,
    direction: CycleDirection,
    minimize_inactive: bool,
) -> Result<()> {
    let new_active_id = {
        let mut state = state.lock().unwrap();
        if let Ok(active) = wm.get_active_window() {
            state.sync_with_active(active);
        }
        match direction {
            CycleDirection::Forward => state.cycle_forward(&**wm, minimize_inactive)?,
            CycleDirection::Backward => state.cycle_backward(&**wm, minimize_inactive)?,
        }
        // Compute the now-active id while we still hold the state lock,
        // so the notification carries the freshly-cycled window rather
        // than racing get_active_window against EVE's slow focus path.
        let windows = state.get_windows();
        let idx = state.get_current_index();
        windows.get(idx).map(|w| w.id)
    };
    // Push the active-id straight to the preview manager so its red
    // border updates the same frame as the cycle, without waiting for
    // EVENT_SYSTEM_FOREGROUND to fire. No-op when previews are off.
    if let Some(id) = new_active_id {
        crate::preview_windows::notify_active_change(id);
    }
    Ok(())
}
