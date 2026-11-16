//! What the output device was opened with, for the app's stream info panel
//! ("output" card of Android `StreamStats`). Published by `open_output`.

use std::sync::Mutex;

/// The open output stream's format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFormat {
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Device sample format, e.g. "f32", "i16".
    pub sample_format: String,
    /// The host API (e.g. "WASAPI").
    pub host: String,
}

static CURRENT: Mutex<Option<OutputFormat>> = Mutex::new(None);

pub(crate) fn publish(format: OutputFormat) {
    if let Ok(mut c) = CURRENT.lock() {
        *c = Some(format);
    }
}

/// The format of the output stream the engine has open, if any. Cheap.
pub fn current_output() -> Option<OutputFormat> {
    CURRENT.lock().ok().and_then(|c| c.clone())
}
