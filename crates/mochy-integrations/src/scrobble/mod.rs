//! Last.fm / Libre.fm scrobbling (Android `scrobble/*` + `providers/lastfm`).

pub mod audioscrobbler;
pub mod queue;
pub mod scrobbler;
pub mod timer;

pub use audioscrobbler::{Audioscrobbler, ScrobbleError, ScrobbleTrack, Session};
pub use queue::{PendingScrobble, ScrobbleQueue, ScrobbleSite};
pub use scrobbler::{Scrobbler, ScrobblerEvent};
pub use timer::{ScrobbleRules, ScrobbleTimer, scrobble_track};
