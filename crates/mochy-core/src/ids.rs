//! Id generation matching the Android scheme (spec §5.1):
//! artists `LA` + 8 random letters, playlists `LP` + 8 letters;
//! local songs get a stable id derived from their path.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn next_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    // splitmix64 step so successive calls in the same nanosecond still vary.
    nanos.wrapping_add(0x9E3779B97F4A7C15)
}

fn random_letters(count: usize) -> String {
    let mut state = next_seed();
    let mut out = String::with_capacity(count);
    for _ in 0..count {
        state = state
            .wrapping_mul(0x2545F4914F6CDD1D)
            .wrapping_add(0x9E3779B97F4A7C15);
        let idx = ((state >> 33) % LETTERS.len() as u64) as usize;
        out.push(LETTERS[idx] as char);
    }
    out
}

/// `LA` + 8 letters.
pub fn gen_artist_id() -> String {
    format!("LA{}", random_letters(8))
}

/// `LP` + 8 letters.
pub fn gen_playlist_id() -> String {
    format!("LP{}", random_letters(8))
}

/// Stable id for a local file: `LOC_` + 16 hex digits from an FNV-1a hash of
/// the canonical path string (case-folded on Windows).
pub fn song_id_for_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = if cfg!(windows) { s.to_lowercase() } else { s.to_string() };
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("LOC_{:016x}", hash)
}

/// Song id: video id when remote, path-derived when local.
pub fn song_id(video_id: Option<&str>, local_path: Option<&Path>) -> String {
    if let Some(v) = video_id {
        v.to_string()
    } else if let Some(p) = local_path {
        song_id_for_path(p)
    } else {
        gen_playlist_id().replace("LP", "SNG_")
    }
}
