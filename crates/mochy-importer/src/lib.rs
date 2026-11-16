//! mochy-importer — bring playlists from other services into the local
//! library (spec §6), ported from the Android `importer/*` sources.
//!
//! - [`link`]: public Spotify / Apple Music / Deezer / YouTube links
//! - [`csv`]: TuneMyMusic, Exportify and Mochy CSVs, plus plain text lists
//! - [`account`]: a signed-in Spotify / Apple Music / Tidal library from pasted tokens
//! - [`matcher`]: TrackMatcher (ported exactly) against YouTube Music search
//! - [`runner`] + [`store`]: the import itself, re-sync and undo bookkeeping

pub mod account;
pub mod csv;
pub mod json;
pub mod link;
pub mod matcher;
pub mod matches;
pub mod models;
pub mod runner;
pub mod store;

pub use models::*;
