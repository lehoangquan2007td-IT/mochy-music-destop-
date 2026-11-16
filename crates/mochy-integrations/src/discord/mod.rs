//! Discord Rich Presence through the local Discord client's IPC pipe.
//!
//! Upstream (Android) signs in with OAuth and talks to Discord's gateway; on
//! desktop the Discord client itself is the transport, so only a Discord
//! application id is needed (no login, no token).

pub mod activity;
pub mod ipc;
pub mod presence;

pub use activity::{NowPlaying, PresenceActivity};
pub use presence::{PresenceConnection, PresenceHandle};
