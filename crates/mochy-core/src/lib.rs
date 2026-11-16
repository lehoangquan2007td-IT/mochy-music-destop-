//! mochy-core — shared models, settings, EQ definitions and errors for the
//! Mochy Music desktop port. Contract crate: all other crates depend on these
//! types; do not rename public items without coordinating.
//!
//! Spec of record: `docs/PORT_SPEC.md` (extracted from the Android sources).

pub mod eq;
pub mod error;
pub mod ids;
pub mod integrations;
pub mod models;
pub mod settings;

pub use error::{CoreError, Result};
