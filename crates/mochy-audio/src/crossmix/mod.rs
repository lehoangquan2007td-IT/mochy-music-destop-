//! Crossmix: beat/energy-aware DJ transitions (port of Android
//! `crossmix/*` + `CrossmixDirector`). Analysis → planning → per-lane
//! rendering; the decode thread executes plans (see `crate::mix`).

pub mod analysis;
pub(crate) mod bars;
pub mod fx;
pub mod planner;
pub mod styles;
pub mod worker;

pub use analysis::{StreamingTrackAnalyzer, TrackAnalysis};
pub use planner::{Plan, TrackInput, plan};
pub use styles::{FxParams, Role, Style};
