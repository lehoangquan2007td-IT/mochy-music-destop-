#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Mochy Music desktop — app shell.
//!
//! CLI:
//!   mochy-app                      start the GUI
//!   mochy-app --smoke [QUERY] [--secs N]
//!                                  headless integration run: DB, YTM home,
//!                                  search QUERY, resolve + play the first
//!                                  song for N seconds, lyrics; logs events.

mod actions;
mod app;
mod controller;
mod db;
mod images;
mod local_scan;
mod logging;
mod lyrics;
mod lyrics_extras;
mod media_keys;
mod nav;
mod pages;
mod queue;
mod services;
mod smoke;
mod ui;
mod widgets;

fn main() {
    let log_path = logging::init();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), log = ?log_path, "Mochy Music starting");

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("mochy-rt")
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("failed to start tokio runtime: {e}");
            std::process::exit(1);
        }
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--smoke") {
        let mut query = "daft punk get lucky".to_string();
        let mut secs = 15u64;
        let mut it = args[i + 1..].iter();
        while let Some(a) = it.next() {
            if a == "--secs" {
                secs = it.next().and_then(|s| s.parse().ok()).unwrap_or(secs);
            } else if !a.starts_with("--") {
                query = a.clone();
            }
        }
        let code = smoke::run(rt.handle().clone(), &query, secs);
        rt.shutdown_timeout(std::time::Duration::from_secs(2));
        std::process::exit(code);
    }

    if let Err(e) = app::run(rt.handle().clone()) {
        tracing::error!("eframe error: {e}");
    }
    tracing::info!("Mochy Music exiting");
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
}
