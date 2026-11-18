//! The WebView2 sign-in itself — the desktop counterpart of Android's
//! `YouTubeLoginScreen`.
//!
//! A child WebView covers the window below a small egui bar. It opens
//! Google's sign-in page with `continue=music.youtube.com`; once the page is
//! on music.youtube.com and the profile holds a `SAPISID` cookie, the cookies
//! (HttpOnly ones included, read from the WebView2 cookie manager) and the
//! page's `VISITOR_DATA` / `DATASYNC_ID` are captured.
//!
//! The WebView2 profile lives in its own folder (given by mochy-app), so
//! Google keeps the user signed in there for the next sign-in; signing out of
//! the app wipes it (`--fresh`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a finished sign-in hands back to mochy-app.
#[derive(Debug, Clone, Default)]
pub struct Session {
    pub cookie: String,
    pub data_sync_id: Option<String>,
    pub visitor_data: Option<String>,
}

pub const LOGIN_URL: &str = "https://accounts.google.com/ServiceLogin?ltmpl=music&service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";

/// Reads the session ids YouTube Music keeps in its page config.
const READ_CONFIG_JS: &str = r#"
(function() {
  try {
    var get = function(k) {
      if (window.ytcfg && ytcfg.get) { var v = ytcfg.get(k); if (v) return v; }
      if (window.yt && yt.config_ && yt.config_[k]) return yt.config_[k];
      return null;
    };
    return { visitorData: get('VISITOR_DATA'), dataSyncId: get('DATASYNC_ID') };
  } catch (e) { return null; }
})();
"#;

const POLL_EVERY: Duration = Duration::from_millis(900);
/// How long to wait for the page config after the cookies are there.
const CONFIG_WAIT: Duration = Duration::from_secs(4);

pub enum Outcome {
    Pending,
    Done(Session),
    Failed(String),
}

struct Captured {
    cookie: String,
    at: Instant,
    asked_config: bool,
}

pub struct LoginWindow {
    context: wry::WebContext,
    webview: Option<wry::WebView>,
    clear_first: bool,
    failed: bool,
    last_poll: Instant,
    captured: Option<Captured>,
    config: Arc<Mutex<Option<String>>>,
}

impl LoginWindow {
    /// `clear_first`: wipe the profile (cookies, storage) before showing the
    /// page, e.g. after signing out or to use another account.
    pub fn new(profile_dir: PathBuf, clear_first: bool) -> Self {
        let dir = profile_dir;
        let _ = std::fs::create_dir_all(&dir);
        Self {
            context: wry::WebContext::new(Some(dir)),
            webview: None,
            clear_first,
            failed: false,
            last_poll: Instant::now(),
            captured: None,
            config: Arc::new(Mutex::new(None)),
        }
    }

    /// Page the sign-in window is on (diagnostics).
    pub fn current_url(&self) -> Option<String> {
        self.webview.as_ref().and_then(|w| w.url().ok())
    }

    /// Drive the sign-in once per frame. `rect` (egui points) is where the
    /// WebView goes; `ppp` is egui's pixels-per-point.
    pub fn frame(&mut self, frame: &eframe::Frame, rect: egui::Rect, ppp: f32) -> Outcome {
        if self.failed {
            return Outcome::Pending;
        }
        let bounds = wry::Rect {
            position: wry::dpi::PhysicalPosition::new((rect.min.x * ppp).round() as i32, (rect.min.y * ppp).round() as i32).into(),
            size: wry::dpi::PhysicalSize::new((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32).into(),
        };
        let Some(webview) = &self.webview else {
            let built = wry::WebViewBuilder::new_with_web_context(&mut self.context)
                .with_url(LOGIN_URL)
                .with_bounds(bounds)
                .with_focused(true)
                .build_as_child(frame);
            match built {
                Ok(w) => {
                    if self.clear_first {
                        if let Err(e) = w.clear_all_browsing_data() {
                            eprintln!("clearing the sign-in profile failed: {e}");
                        }
                        // Reload so the cleared profile shows the sign-in form.
                        let _ = w.load_url(LOGIN_URL);
                    }
                    self.webview = Some(w);
                    return Outcome::Pending;
                }
                Err(e) => {
                    self.failed = true;
                    return Outcome::Failed(format!("WebView2: {e}"));
                }
            }
        };
        let _ = webview.set_bounds(bounds);

        if self.last_poll.elapsed() < POLL_EVERY {
            return Outcome::Pending;
        }
        self.last_poll = Instant::now();

        if let Some(cap) = &mut self.captured {
            let config = self.config.lock().ok().and_then(|g| g.clone());
            if !cap.asked_config {
                cap.asked_config = true;
                let slot = self.config.clone();
                let _ = webview.evaluate_script_with_callback(READ_CONFIG_JS, move |json| {
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(json);
                    }
                });
            }
            if config.is_some() || cap.at.elapsed() >= CONFIG_WAIT {
                let (visitor_data, data_sync_id) = config.as_deref().map(parse_config).unwrap_or_default();
                return Outcome::Done(Session { cookie: cap.cookie.clone(), data_sync_id, visitor_data });
            }
            return Outcome::Pending;
        }

        let on_music = webview.url().map(|u| u.starts_with("https://music.youtube.com")).unwrap_or(false);
        if !on_music {
            return Outcome::Pending;
        }
        match webview.cookies_for_url("https://music.youtube.com") {
            Ok(cookies) => {
                let cookie = cookies
                    .iter()
                    .filter(|c| !c.value().is_empty())
                    .map(|c| format!("{}={}", c.name(), c.value()))
                    .collect::<Vec<_>>()
                    .join("; ");
                if mochy_innertube::YtAuth::new(cookie.clone(), None).is_signed_in() {
                    self.captured = Some(Captured { cookie, at: Instant::now(), asked_config: false });
                }
            }
            Err(e) => eprintln!("reading sign-in cookies failed: {e}"),
        }
        Outcome::Pending
    }
}

/// `{"visitorData": "...", "dataSyncId": "..."}` (or `null`).
fn parse_config(json: &str) -> (Option<String>, Option<String>) {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let get = |k: &str| v[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    (get("visitorData"), get("dataSyncId"))
}

#[cfg(test)]
mod tests {
    use super::parse_config;

    #[test]
    fn config_json() {
        assert_eq!(
            parse_config(r#"{"visitorData":"CgtABC","dataSyncId":"123||"}"#),
            (Some("CgtABC".into()), Some("123||".into()))
        );
        assert_eq!(parse_config("null"), (None, None));
        assert_eq!(parse_config(r#"{"visitorData":null,"dataSyncId":""}"#), (None, None));
    }
}
