//! tracing → stderr + `<data>/logs/mochy.log` (previous run kept as
//! `mochy.prev.log`). Panics are logged before the default hook runs.

use std::path::PathBuf;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub fn log_dir() -> PathBuf {
    mochy_storage::data_dir().join("logs")
}

pub fn log_file() -> PathBuf {
    log_dir().join("mochy.log")
}

/// Install the global subscriber. Returns the log file path when it could be opened.
pub fn init() -> Option<PathBuf> {
    let filter = EnvFilter::try_from_env("MOCHY_LOG").unwrap_or_else(|_| {
        EnvFilter::new("info,mochy_app=debug,eframe=warn,egui_glow=warn,egui_winit=warn,winit=warn,reqwest=warn,hyper=warn,symphonia=warn")
    });
    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_target(true);

    let path = log_file();
    let file = (|| {
        std::fs::create_dir_all(log_dir()).ok()?;
        if path.exists() {
            let _ = std::fs::rename(&path, log_dir().join("mochy.prev.log"));
        }
        std::fs::File::create(&path).ok()
    })();
    let opened = file.is_some();
    let file_layer = file.map(|f| {
        tracing_subscriber::fmt::layer()
            .with_writer(Mutex::new(f))
            .with_ansi(false)
            .with_target(true)
    });
    let _ = tracing_subscriber::registry().with(filter).with(stderr_layer).with(file_layer).try_init();

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!(thread = thread.name().unwrap_or("?"), "PANIC: {info}");
        default_hook(info);
    }));

    opened.then_some(path)
}
