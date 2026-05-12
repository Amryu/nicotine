use crate::config::Config;
use crate::window_manager::{EveWindow, WindowManager};
use anyhow::{Context, Result};
use std::ffi::c_void;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindow, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow, GW_HWNDPREV, HWND_TOP,
    SC_MINIMIZE, SC_RESTORE, SWP_NOSIZE, SWP_NOZORDER, SW_MINIMIZE, SW_RESTORE, WM_SYSCOMMAND,
};

pub struct WindowsManager;

impl WindowsManager {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

pub(crate) fn hwnd_to_id(hwnd: HWND) -> u32 {
    hwnd.0 as usize as u32
}

pub(crate) fn id_to_hwnd(id: u32) -> HWND {
    HWND(id as usize as *mut c_void)
}

/// Is an EVE client window currently in the foreground? Mirrors the
/// filter used by `enum_collect_eve` (title prefix + Launcher exclusion).
/// Called from the LL mouse hook's `OnlyWhenEveFocused` gate, which
/// fires per XBUTTON event — so this needs to be cheap. `GetForegroundWindow`
/// + a title fetch is ~microsecond range, fine at human click rates.
pub fn is_foreground_eve() -> bool {
    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() {
        return false;
    }
    let title = read_window_title(fg);
    title.starts_with("EVE - ") && !title.contains("Launcher")
}

fn read_window_title(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    // +1 for the null terminator that GetWindowTextW writes
    let mut buf: Vec<u16> = vec![0; len as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if copied <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..copied as usize])
}

unsafe extern "system" fn enum_collect_eve(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let windows = &mut *(lparam.0 as *mut Vec<EveWindow>);

    if !IsWindowVisible(hwnd).as_bool() {
        return TRUE;
    }

    let title = read_window_title(hwnd);
    if title.starts_with("EVE - ") && !title.contains("Launcher") {
        windows.push(EveWindow {
            id: hwnd_to_id(hwnd),
            title: title.trim_start_matches("EVE - ").to_string(),
        });
    }

    TRUE
}

/// SetForegroundWindow is restricted on modern Windows — for reliable
/// focus stealing from another process, the standard pattern is to attach
/// our input queue to the target's. But that's slow. The fast path:
/// SetForegroundWindow works directly when our process "received the last
/// input event" — and a `RegisterHotKey` WM_HOTKEY counts. So we try the
/// cheap call first and only fall back to the AttachThreadInput dance if
/// Windows rejects it (typical when activation is triggered from the
/// passive low-level mouse hook, not a hotkey).
fn force_activate(target: HWND) {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground == target {
            // Already focused — skip everything. Common case for repeated
            // hotkey presses against the active window.
            return;
        }

        // SW_RESTORE triggers the show animation, so only fire it when we
        // genuinely need to un-minimize.
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }

        // Fast path: try direct SetForegroundWindow first.
        if SetForegroundWindow(target).as_bool() {
            return;
        }

        // Fallback path: Windows refused the foreground change. Briefly
        // attach our thread's input queue to the target and current
        // foreground's queues so we look like the same input session.
        let target_thread = GetWindowThreadProcessId(target, None);
        let current_thread = GetCurrentThreadId();
        let foreground_thread = if foreground.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, None)
        };

        let attached_target = target_thread != 0
            && target_thread != current_thread
            && AttachThreadInput(current_thread, target_thread, true).as_bool();
        let attached_foreground = foreground_thread != 0
            && foreground_thread != current_thread
            && foreground_thread != target_thread
            && AttachThreadInput(current_thread, foreground_thread, true).as_bool();

        let _ = SetForegroundWindow(target);
        let _ = BringWindowToTop(target);
        let _ = SetFocus(Some(target));

        if attached_target {
            let _ = AttachThreadInput(current_thread, target_thread, false);
        }
        if attached_foreground {
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        }
    }
}

impl WindowManager for WindowsManager {
    fn get_eve_windows(&self) -> Result<Vec<EveWindow>> {
        // Use a Mutex<Vec<...>> to satisfy unwind safety even though the
        // callback is single-threaded — EnumWindows is synchronous.
        let mut windows: Vec<EveWindow> = Vec::new();
        unsafe {
            EnumWindows(
                Some(enum_collect_eve),
                LPARAM(&mut windows as *mut _ as isize),
            )
            .context("EnumWindows failed")?;
        }
        Ok(windows)
    }

    fn activate_window(&self, window_id: u32) -> Result<()> {
        force_activate(id_to_hwnd(window_id));
        Ok(())
    }

    fn stack_windows(&self, windows: &[EveWindow], config: &Config) -> Result<()> {
        let x = ((config.display_width - config.eve_width) / 2) as i32;
        let y = 0;
        let width = config.eve_width as i32;
        let height = (config.display_height - config.panel_height) as i32;

        for window in windows {
            unsafe {
                SetWindowPos(
                    id_to_hwnd(window.id),
                    Some(HWND_TOP),
                    x,
                    y,
                    width,
                    height,
                    SWP_NOZORDER,
                )
                .ok();
            }
        }
        Ok(())
    }

    fn get_active_window(&self) -> Result<u32> {
        let hwnd = unsafe { GetForegroundWindow() };
        Ok(hwnd_to_id(hwnd))
    }

    fn find_window_by_title(&self, title: &str) -> Result<Option<u32>> {
        struct Search {
            needle: String,
            found: Option<u32>,
        }
        let mut search = Search {
            needle: title.to_string(),
            found: None,
        };

        unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let s = &mut *(lparam.0 as *mut Search);
            if s.found.is_some() {
                return BOOL(0);
            }
            if !IsWindowVisible(hwnd).as_bool() {
                return TRUE;
            }
            if read_window_title(hwnd) == s.needle {
                s.found = Some(hwnd_to_id(hwnd));
                return BOOL(0);
            }
            TRUE
        }

        unsafe {
            // Returning false from the callback to short-circuit surfaces as
            // an Err from EnumWindows; ignore it.
            let _ = EnumWindows(Some(cb), LPARAM(&mut search as *mut _ as isize));
        }
        Ok(search.found)
    }

    fn move_window(&self, window_id: u32, x: i32, y: i32) -> Result<()> {
        unsafe {
            SetWindowPos(
                id_to_hwnd(window_id),
                Some(HWND_TOP),
                x,
                y,
                0,
                0,
                SWP_NOZORDER | SWP_NOSIZE,
            )
            .ok();
        }
        Ok(())
    }

    fn minimize_window(&self, window_id: u32) -> Result<()> {
        let hwnd = id_to_hwnd(window_id);
        unsafe {
            // SC_MINIMIZE via WM_SYSCOMMAND is friendlier to applications
            // than ShowWindow(SW_MINIMIZE) — it goes through the normal
            // window state machine.
            SendMessageW(
                hwnd,
                WM_SYSCOMMAND,
                Some(WPARAM(SC_MINIMIZE as usize)),
                Some(LPARAM(0)),
            );
        }
        let _ = unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
        Ok(())
    }

    fn restore_window(&self, window_id: u32) -> Result<()> {
        let hwnd = id_to_hwnd(window_id);
        unsafe {
            SendMessageW(
                hwnd,
                WM_SYSCOMMAND,
                Some(WPARAM(SC_RESTORE as usize)),
                Some(LPARAM(0)),
            );
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        Ok(())
    }

    fn eve_visibility_ratios(&self) -> Vec<(u32, f32)> {
        let eve_windows = match self.get_eve_windows() {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        // Snapshot our PID once and let visibility_ratio exclude any
        // window owned by it — preview / list / config-panel windows
        // shouldn't count as occluding the EVE clients they're meant
        // to surface.
        let self_pid = unsafe { GetCurrentProcessId() };
        let mut out = Vec::with_capacity(eve_windows.len());
        for w in &eve_windows {
            let hwnd = id_to_hwnd(w.id);
            out.push((w.id, visibility_ratio(hwnd, self_pid)));
        }
        out
    }
}

/// Compute the visible-area ratio for one window. Minimized → 0.0;
/// fully unoccluded inside its monitor → 1.0. Considers every window
/// above this one in the system z-order as a potential occluder,
/// except those owned by `self_pid` — Nicotine's own preview / list /
/// config-panel windows must not count as occlusion (that would cause
/// Smart Hide to fight itself).
fn visibility_ratio(hwnd: HWND, self_pid: u32) -> f32 {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            return 0.0;
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return 0.0;
        }
        let mut window_rect = RECT::default();
        if GetWindowRect(hwnd, &mut window_rect).is_err() {
            return 0.0;
        }

        // Clip to the work area of the containing monitor so windows that
        // partially hang off-screen (or onto a disconnected display) don't
        // inflate their denominator.
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        // Zero-init then set cbSize; GetMonitorInfo refuses to fill if
        // cbSize is wrong. Avoid Default::default() in case the
        // windows-rs version doesn't derive it for this struct.
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let work_area = if GetMonitorInfoW(monitor, &mut mi).as_bool() {
            mi.rcWork
        } else {
            window_rect
        };
        let clipped = intersect_rect(&window_rect, &work_area);
        let total_area = rect_area(&clipped);
        if total_area <= 0 {
            return 0.0;
        }

        // Walk z-order from this HWND toward the foreground via
        // GW_HWNDPREV (Windows treats the topmost as the "previous"
        // sibling). Every non-cloaked, visible, non-iconic window we
        // encounter that overlaps `clipped` is an occluder.
        let mut occluders: Vec<RECT> = Vec::new();
        let mut cur = hwnd;
        loop {
            let prev = match GetWindow(cur, GW_HWNDPREV) {
                Ok(h) if !h.0.is_null() => h,
                _ => break,
            };
            cur = prev;

            if !IsWindowVisible(prev).as_bool() || IsIconic(prev).as_bool() {
                continue;
            }
            // Skip windows owned by us (preview thumbnails, list window,
            // config panel). Without this filter, opening the config
            // panel over the EVE foreground would knock its visibility
            // ratio below 90% and Smart Hide would hide all previews —
            // exactly the user gesture that proves they want previews
            // visible.
            let mut other_pid: u32 = 0;
            let _ = GetWindowThreadProcessId(prev, Some(&mut other_pid));
            if other_pid == self_pid {
                continue;
            }
            // Skip DWM-cloaked windows (UWP/explorer.exe ghosts that
            // never actually paint).
            let mut cloaked: u32 = 0;
            let _ = DwmGetWindowAttribute(
                prev,
                DWMWA_CLOAKED,
                &mut cloaked as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );
            if cloaked != 0 {
                continue;
            }
            let mut r = RECT::default();
            if GetWindowRect(prev, &mut r).is_err() {
                continue;
            }
            let overlap = intersect_rect(&r, &clipped);
            if rect_area(&overlap) > 0 {
                occluders.push(overlap);
            }
        }

        // Rectangle-subtraction sweep. After each occluder, fragments
        // remain disjoint, so the running sum of fragment area gives us
        // the actual unoccluded surface.
        let mut fragments = vec![clipped];
        for occ in &occluders {
            let mut next = Vec::with_capacity(fragments.len() * 2);
            for f in &fragments {
                subtract_rect(f, occ, &mut next);
            }
            fragments = next;
        }
        let visible: i64 = fragments.iter().map(|r| rect_area(r) as i64).sum();
        (visible as f32) / (total_area as f32)
    }
}

fn intersect_rect(a: &RECT, b: &RECT) -> RECT {
    RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    }
}

fn rect_area(r: &RECT) -> i32 {
    let w = (r.right - r.left).max(0);
    let h = (r.bottom - r.top).max(0);
    w * h
}

/// Push the up-to-four rectangles covering `a - b` into `out`. Empty
/// pieces (zero or negative dimension) are skipped.
fn subtract_rect(a: &RECT, b: &RECT, out: &mut Vec<RECT>) {
    let inter = intersect_rect(a, b);
    if rect_area(&inter) == 0 {
        // No overlap → `a` survives whole.
        out.push(*a);
        return;
    }
    // Top strip above the intersection.
    if a.top < inter.top {
        out.push(RECT {
            left: a.left,
            top: a.top,
            right: a.right,
            bottom: inter.top,
        });
    }
    // Bottom strip below the intersection.
    if inter.bottom < a.bottom {
        out.push(RECT {
            left: a.left,
            top: inter.bottom,
            right: a.right,
            bottom: a.bottom,
        });
    }
    // Left strip, within the intersection's vertical band.
    if a.left < inter.left {
        out.push(RECT {
            left: a.left,
            top: inter.top,
            right: inter.left,
            bottom: inter.bottom,
        });
    }
    // Right strip, within the intersection's vertical band.
    if inter.right < a.right {
        out.push(RECT {
            left: inter.right,
            top: inter.top,
            right: a.right,
            bottom: inter.bottom,
        });
    }
}

// SAFETY: WindowsManager has no state and Win32 window APIs are thread-safe
// for the operations we perform.
unsafe impl Send for WindowsManager {}
unsafe impl Sync for WindowsManager {}
