//! Run in background: notification-area (system tray) icon + single instance.
//!
//! With `misc.run_in_background` on, closing the window only hides it; the
//! player keeps going (eframe keeps calling `App::logic` for hidden windows
//! while a repaint is pending) and the tray icon brings the window back.
//! Tray clicks and menu picks arrive on the UI thread's message loop and are
//! forwarded as [`AppMsg::Tray`] through the bus, which also wakes the app.
//!
//! Single instance: a second launch signals a named event and exits; the
//! running instance shows its window (also when it is hidden in the tray).

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::services::{AppMsg, Bus};
use crate::ui::i18n::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    Toggle,
    Next,
    Prev,
    Quit,
}

const ID_SHOW: &str = "mochy.show";
const ID_TOGGLE: &str = "mochy.toggle";
const ID_NEXT: &str = "mochy.next";
const ID_PREV: &str = "mochy.prev";
const ID_QUIT: &str = "mochy.quit";

struct Live {
    icon: TrayIcon,
    show: MenuItem,
    toggle: MenuItem,
    next: MenuItem,
    prev: MenuItem,
    quit: MenuItem,
}

/// The tray icon (present only while background mode is on).
pub struct Tray {
    live: Option<Live>,
    icon_png: &'static [u8],
    /// Last tooltip / labels pushed (avoid redundant OS calls).
    shown: (String, bool, &'static str),
}

impl Tray {
    /// Installs the global tray/menu event handlers (they forward to `bus`).
    pub fn new(bus: Bus, icon_png: &'static [u8]) -> Self {
        let b = bus.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| match e {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
            | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => b.send(AppMsg::Tray(TrayCmd::Show)),
            _ => {}
        }));
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let cmd = match e.id.as_ref() {
                ID_SHOW => TrayCmd::Show,
                ID_TOGGLE => TrayCmd::Toggle,
                ID_NEXT => TrayCmd::Next,
                ID_PREV => TrayCmd::Prev,
                ID_QUIT => TrayCmd::Quit,
                _ => return,
            };
            bus.send(AppMsg::Tray(cmd));
        }));
        Self { live: None, icon_png, shown: (String::new(), false, "") }
    }

    pub fn is_active(&self) -> bool {
        self.live.is_some()
    }

    /// Create or remove the icon to match the setting; refresh tooltip/labels.
    pub fn sync(&mut self, enabled: bool, now_playing: Option<String>, playing: bool) {
        if !enabled {
            if self.live.take().is_some() {
                tracing::info!("tray icon removed");
            }
            return;
        }
        if self.live.is_none() {
            match self.build() {
                Ok(l) => {
                    tracing::info!("tray icon created");
                    self.live = Some(l);
                    self.shown = (String::new(), false, "");
                }
                Err(e) => {
                    tracing::warn!("tray icon failed: {e}");
                    return;
                }
            }
        }
        let Some(l) = &self.live else { return };
        let tip = match now_playing {
            Some(s) => format!("Mochy Music — {s}"),
            None => "Mochy Music".to_string(),
        };
        // Tooltips are limited to 127 chars on Windows.
        let tip: String = tip.chars().take(120).collect();
        let lang_key = tr("Show Mochy");
        if self.shown != (tip.clone(), playing, lang_key) {
            let _ = l.icon.set_tooltip(Some(&tip));
            l.show.set_text(tr("Show Mochy"));
            l.toggle.set_text(if playing { tr("Pause") } else { tr("Play") });
            l.next.set_text(tr("Next"));
            l.prev.set_text(tr("Previous"));
            l.quit.set_text(tr("Quit Mochy"));
            self.shown = (tip, playing, lang_key);
        }
    }

    fn build(&self) -> Result<Live, String> {
        let img = image::load_from_memory(self.icon_png).map_err(|e| e.to_string())?;
        let img = img.resize(64, 64, image::imageops::FilterType::Lanczos3).to_rgba8();
        let (w, h) = img.dimensions();
        let icon = Icon::from_rgba(img.into_raw(), w, h).map_err(|e| e.to_string())?;
        let show = MenuItem::with_id(ID_SHOW, tr("Show Mochy"), true, None);
        let toggle = MenuItem::with_id(ID_TOGGLE, tr("Play"), true, None);
        let next = MenuItem::with_id(ID_NEXT, tr("Next"), true, None);
        let prev = MenuItem::with_id(ID_PREV, tr("Previous"), true, None);
        let quit = MenuItem::with_id(ID_QUIT, tr("Quit Mochy"), true, None);
        let menu = Menu::with_items(&[
            &show,
            &PredefinedMenuItem::separator(),
            &toggle,
            &next,
            &prev,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Mochy Music")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Live { icon, show, toggle, next, prev, quit })
    }
}

// ---------------------------------------------------------------------------
// Single instance
// ---------------------------------------------------------------------------

#[cfg(windows)]
const SHOW_EVENT: &str = "Local\\MochyMusic.ShowWindow";

/// Handle of the "show window" event owned by the first instance.
pub struct Instance {
    #[cfg(windows)]
    handle: usize,
}

/// `Some` when this is the first instance; `None` when another instance is
/// already running (it has been asked to show its window — just exit).
pub fn claim_single_instance() -> Option<Instance> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};
        let name: Vec<u16> = SHOW_EVENT.encode_utf16().chain(Some(0)).collect();
        // SAFETY: plain Win32 calls with a valid NUL-terminated name.
        unsafe {
            let h = CreateEventW(std::ptr::null(), 0, 0, name.as_ptr());
            if h.is_null() {
                return Some(Instance { handle: 0 });
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                SetEvent(h);
                return None;
            }
            Some(Instance { handle: h as usize })
        }
    }
    #[cfg(not(windows))]
    {
        Some(Instance {})
    }
}

impl Instance {
    /// Forward "show" requests from later launches to the app.
    pub fn listen(&self, bus: Bus) {
        #[cfg(windows)]
        {
            let handle = self.handle;
            if handle == 0 {
                return;
            }
            let _ = std::thread::Builder::new().name("mochy-instance".into()).spawn(move || {
                use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
                loop {
                    // SAFETY: the event handle lives for the whole process.
                    let r = unsafe { WaitForSingleObject(handle as _, INFINITE) };
                    if r != 0 {
                        break;
                    }
                    tracing::info!("another launch asked to show the window");
                    bus.send(AppMsg::Tray(TrayCmd::Show));
                }
            });
        }
        #[cfg(not(windows))]
        let _ = bus;
    }
}
