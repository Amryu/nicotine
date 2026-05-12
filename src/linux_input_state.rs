//! Shared modifier state between the keyboard and mouse evdev
//! listeners. evdev splits devices per /dev/input/eventN, so the mouse
//! handler can't see keyboard events on its own.

use std::sync::atomic::{AtomicBool, Ordering};

pub static SHIFT_HELD: AtomicBool = AtomicBool::new(false);
pub static CTRL_HELD: AtomicBool = AtomicBool::new(false);
pub static ALT_HELD: AtomicBool = AtomicBool::new(false);

pub fn modifier_held(vk: u16) -> bool {
    match vk {
        0x10 => SHIFT_HELD.load(Ordering::Acquire),
        0x11 => CTRL_HELD.load(Ordering::Acquire),
        0x12 => ALT_HELD.load(Ordering::Acquire),
        _ => false,
    }
}

fn modifier_vk_for_evdev(code: u16) -> Option<u16> {
    match code {
        42 | 54 => Some(0x10),
        29 | 97 => Some(0x11),
        56 | 100 => Some(0x12),
        _ => None,
    }
}

pub fn set_modifier_state(code: u16, pressed: bool) {
    match modifier_vk_for_evdev(code) {
        Some(0x10) => SHIFT_HELD.store(pressed, Ordering::Release),
        Some(0x11) => CTRL_HELD.store(pressed, Ordering::Release),
        Some(0x12) => ALT_HELD.store(pressed, Ordering::Release),
        _ => {}
    }
}
