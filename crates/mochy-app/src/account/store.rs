//! On-disk state of the YouTube account.
//!
//! - The session (cookie, data sync id, visitor data) lives encrypted in
//!   `yt_session.bin`, handled by [`mochy_storage::yt_session`] (shared with
//!   the `mochy-signin` window).
//! - `yt_account.json`: what is safe to keep in the clear — display name,
//!   email, avatar, last sync and which library entries the sync added (so a
//!   later sync can remove exactly those when they leave the YouTube library).

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use mochy_storage::yt_session::{Session, delete_session, load_session, save_session, session_mtime, webview_dir};

pub fn to_auth(s: &Session) -> mochy_innertube::YtAuth {
    mochy_innertube::YtAuth::new(s.cookie.clone(), s.data_sync_id.clone())
}

/// Non-secret account state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountFile {
    pub name: String,
    pub email: Option<String>,
    pub channel_handle: Option<String>,
    pub thumbnail_url: Option<String>,
    /// Epoch millis of the last successful sync.
    pub last_sync_ms: Option<i64>,
    pub last_report: Option<SyncCounts>,
    /// Library entries that exist because a sync added them.
    pub synced_playlists: BTreeSet<String>,
    pub synced_albums: BTreeSet<String>,
    pub synced_artists: BTreeSet<String>,
}

/// What the last sync saw / changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncCounts {
    pub liked_total: usize,
    pub liked_new: usize,
    pub playlists: usize,
    pub albums: usize,
    pub artists: usize,
    pub history: usize,
    pub removed: usize,
    /// Lists that failed to load.
    pub errors: Vec<String>,
}

fn account_path() -> PathBuf {
    mochy_storage::data_dir().join("yt_account.json")
}

pub fn load_account() -> AccountFile {
    std::fs::read(account_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_account(a: &AccountFile) {
    let path = account_path();
    match serde_json::to_vec_pretty(a) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!("saving {} failed: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("serialising account file failed: {e}"),
    }
}

pub fn delete_account() {
    let _ = std::fs::remove_file(account_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_file_defaults() {
        let a: AccountFile = serde_json::from_str(r#"{"name":"Q"}"#).unwrap();
        assert_eq!(a.name, "Q");
        assert!(a.synced_playlists.is_empty());
        assert!(a.last_sync_ms.is_none());
    }
}
