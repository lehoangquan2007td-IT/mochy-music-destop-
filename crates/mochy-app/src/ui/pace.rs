//! Frame pacing: how often running animations (lyrics scroll, spinning
//! records, visualisers…) may repaint, by what the user is doing.
//!
//! * focused window → at most 60 fps (not the monitor's 144/240 Hz);
//! * window in the background → 30 fps;
//! * another app fullscreen in front (a game) → 6 fps, so Mochy leaves the
//!   GPU and CPU to it.
//!
//! Animations call [`animate`] / [`animate_after`] instead of
//! `request_repaint()`; one-off repaints (a message arrived, a click) stay
//! immediate.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    Focused,
    Background,
    /// Another program covers a whole monitor in front of everything.
    Game,
}

static PACE: AtomicU8 = AtomicU8::new(0);
static GAME_CHECK: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
const GAME_CHECK_EVERY: Duration = Duration::from_secs(1);

/// Call once per frame (before drawing).
pub fn update(ctx: &egui::Context) {
    let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
    let pace = if focused {
        Pace::Focused
    } else if fullscreen_app_in_front() {
        Pace::Game
    } else {
        Pace::Background
    };
    PACE.store(pace as u8, Ordering::Relaxed);
}

pub fn current() -> Pace {
    match PACE.load(Ordering::Relaxed) {
        1 => Pace::Background,
        2 => Pace::Game,
        _ => Pace::Focused,
    }
}

/// The shortest gap between two animation frames right now.
pub fn frame_interval() -> Duration {
    match current() {
        Pace::Focused => Duration::from_millis(16),
        Pace::Background => Duration::from_millis(33),
        Pace::Game => Duration::from_millis(160),
    }
}

/// Ask for the next animation frame, no sooner than the pace allows.
pub fn animate(ctx: &egui::Context) {
    ctx.request_repaint_after(frame_interval());
}

/// Like [`animate`] for an animation that is fine with `after` (never faster
/// than the pace).
pub fn animate_after(ctx: &egui::Context, after: Duration) {
    ctx.request_repaint_after(after.max(frame_interval()));
}

/// Cached (≤ once a second): is a window of another process covering a
/// whole monitor in the foreground?
fn fullscreen_app_in_front() -> bool {
    let mut g = GAME_CHECK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *g
        && at.elapsed() < GAME_CHECK_EVERY
    {
        return v;
    }
    let v = sys::foreground_is_fullscreen_other_app();
    *g = Some((Instant::now(), v));
    v
}

#[cfg(windows)]
mod sys {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromWindow};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowRect, GetWindowThreadProcessId,
    };

    pub fn foreground_is_fullscreen_other_app() -> bool {
        // SAFETY: plain Win32 queries on a window handle; every out-pointer
        // is a live local of the right type and size.
        unsafe {
            let fg: HWND = GetForegroundWindow();
            if fg.is_null() || fg == GetShellWindow() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, &mut pid);
            if pid == GetCurrentProcessId() {
                return false;
            }
            // The desktop itself (after "show desktop") is fullscreen too.
            let mut class = [0u16; 32];
            let n = GetClassNameW(fg, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            if class == "WorkerW" || class == "Progman" {
                return false;
            }
            let mut r: RECT = std::mem::zeroed();
            if GetWindowRect(fg, &mut r) == 0 {
                return false;
            }
            let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONULL);
            if mon.is_null() {
                return false;
            }
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut mi) == 0 {
                return false;
            }
            let m = mi.rcMonitor;
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
        }
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn foreground_is_fullscreen_other_app() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slower_pace_never_speeds_animations_up() {
        PACE.store(Pace::Game as u8, Ordering::Relaxed);
        assert!(frame_interval() >= Duration::from_millis(100));
        PACE.store(Pace::Focused as u8, Ordering::Relaxed);
        assert_eq!(frame_interval(), Duration::from_millis(16));
    }
}
