//! Single worker thread that owns the database (`mochy_storage::Db`).
//!
//! The Db is opened *inside* the worker thread, so it doesn't need to be
//! `Send` or `Sync`. Jobs are closures; a panicking job is caught and logged
//! so the worker keeps running.

use std::panic::AssertUnwindSafe;
use std::sync::mpsc;

use mochy_core::models::Song;
use mochy_storage::Db;

type Job = Box<dyn FnOnce(&Db) + Send + 'static>;

#[derive(Clone)]
pub struct DbWorker {
    tx: mpsc::Sender<Job>,
}

impl DbWorker {
    /// Spawn the worker. `on_open` receives the open result (`Err` = the
    /// on-disk DB failed and an in-memory fallback is used).
    pub fn spawn(on_open: impl FnOnce(Result<(), String>) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("mochy-db".into())
            .spawn(move || {
                let db = match Db::open_default() {
                    Ok(db) => {
                        tracing::info!(path = %mochy_storage::db_path().display(), "database opened");
                        on_open(Ok(()));
                        db
                    }
                    Err(e) => {
                        tracing::error!("database open failed: {e}; using in-memory fallback");
                        on_open(Err(e.to_string()));
                        match Db::open(std::path::Path::new(":memory:")) {
                            Ok(db) => db,
                            Err(e) => {
                                tracing::error!("in-memory database failed too: {e}");
                                return;
                            }
                        }
                    }
                };
                while let Ok(job) = rx.recv() {
                    if std::panic::catch_unwind(AssertUnwindSafe(|| job(&db))).is_err() {
                        tracing::error!("database job panicked");
                    }
                }
            })
            .expect("spawn db thread");
        Self { tx }
    }

    /// Fire-and-forget job.
    pub fn run(&self, f: impl FnOnce(&Db) + Send + 'static) {
        let _ = self.tx.send(Box::new(f));
    }

    /// Job with a result, awaitable from async code (or `blocking_recv`).
    pub fn query<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Db) -> R + Send + 'static,
    ) -> tokio::sync::oneshot::Receiver<R> {
        let (otx, orx) = tokio::sync::oneshot::channel();
        self.run(move |db| {
            let _ = otx.send(f(db));
        });
        orx
    }
}

/// Insert or merge a song row. `Db::upsert_song` merges (it never wipes likes,
/// library state, play time or known metadata), so it is safe to call for
/// songs that already exist.
pub fn ensure_song(db: &Db, song: &Song) {
    if let Err(e) = db.upsert_song(song) {
        tracing::warn!(id = %song.id, "upsert_song failed: {e}");
    }
}

/// Log-and-drop helper for storage results.
pub fn log_err<T, E: std::fmt::Display>(what: &str, r: Result<T, E>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("{what}: {e}");
            None
        }
    }
}
