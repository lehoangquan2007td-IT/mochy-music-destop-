//! OS media keys + now-playing info via `souvlaki`
//! (Windows SMTC needs the window HWND; Linux uses MPRIS over D-Bus).

use std::time::Duration;

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

use crate::services::{AppMsg, Bus};

#[derive(Debug, Clone, PartialEq)]
pub enum MediaKey {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    SeekBy(f64),
    SetPosition(f64),
    Raise,
}

pub struct MediaKeys {
    controls: MediaControls,
    last_meta: Option<(String, String)>,
    last_playback: Option<(u8, i64)>,
}

impl MediaKeys {
    /// `hwnd`: the native window handle (required on Windows).
    pub fn new(bus: Bus, hwnd: Option<*mut std::ffi::c_void>) -> Result<Self, String> {
        #[cfg(target_os = "windows")]
        if hwnd.is_none() {
            return Err("no HWND available".into());
        }
        let config = PlatformConfig { dbus_name: "mochy_music", display_name: "Mochy Music", hwnd };
        let mut controls = MediaControls::new(config).map_err(|e| format!("{e:?}"))?;
        controls
            .attach(move |ev: MediaControlEvent| {
                let key = match ev {
                    MediaControlEvent::Play => MediaKey::Play,
                    MediaControlEvent::Pause => MediaKey::Pause,
                    MediaControlEvent::Toggle => MediaKey::Toggle,
                    MediaControlEvent::Next => MediaKey::Next,
                    MediaControlEvent::Previous => MediaKey::Previous,
                    MediaControlEvent::Stop => MediaKey::Stop,
                    MediaControlEvent::Seek(SeekDirection::Forward) => MediaKey::SeekBy(10.0),
                    MediaControlEvent::Seek(SeekDirection::Backward) => MediaKey::SeekBy(-10.0),
                    MediaControlEvent::SeekBy(dir, d) => {
                        let s = d.as_secs_f64();
                        MediaKey::SeekBy(if dir == SeekDirection::Forward { s } else { -s })
                    }
                    MediaControlEvent::SetPosition(MediaPosition(p)) => MediaKey::SetPosition(p.as_secs_f64()),
                    MediaControlEvent::Raise => MediaKey::Raise,
                    _ => return,
                };
                bus.send(AppMsg::MediaKey(key));
            })
            .map_err(|e| format!("{e:?}"))?;
        Ok(Self { controls, last_meta: None, last_playback: None })
    }

    /// Update metadata (only when the track changed).
    pub fn set_track(&mut self, id: &str, title: &str, artist: &str, album: Option<&str>, cover: Option<&str>, duration: Option<f64>) {
        let key = (id.to_string(), cover.unwrap_or_default().to_string());
        if self.last_meta.as_ref() == Some(&key) {
            return;
        }
        self.last_meta = Some(key);
        let meta = MediaMetadata {
            title: Some(title),
            artist: Some(artist),
            album,
            cover_url: cover,
            duration: duration.map(Duration::from_secs_f64),
        };
        if let Err(e) = self.controls.set_metadata(meta) {
            tracing::debug!("media metadata update failed: {e:?}");
        }
    }

    /// Update playback state; position is only re-sent when it jumps.
    pub fn set_playback(&mut self, playing: Option<bool>, pos: f64) {
        let state = match playing {
            None => 0u8,
            Some(false) => 1,
            Some(true) => 2,
        };
        let secs = pos as i64;
        if let Some((s, p)) = self.last_playback
            && s == state
            && (state == 2 && (secs - p).abs() < 5 || state != 2 && secs == p)
        {
            return;
        }
        self.last_playback = Some((state, secs));
        let progress = Some(MediaPosition(Duration::from_secs_f64(pos.max(0.0))));
        let pb = match playing {
            None => MediaPlayback::Stopped,
            Some(false) => MediaPlayback::Paused { progress },
            Some(true) => MediaPlayback::Playing { progress },
        };
        if let Err(e) = self.controls.set_playback(pb) {
            tracing::debug!("media playback update failed: {e:?}");
        }
    }
}
