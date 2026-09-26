//! Deterministic rules core.
//!
//! Compiles to native (authoritative server) and `wasm32` (browser client)
//! from one source, so the two cannot disagree. Nothing here touches I/O,
//! the clock, or floating point.

pub mod arena;
pub mod fingerprint;
pub mod generated;
pub mod icons;
pub mod json;
pub mod modifiers;
pub mod names;
pub mod rng;
pub mod ruleset;
#[cfg(feature = "save")]
pub mod save;
#[cfg(feature = "serde")]
pub mod serialization;
pub mod shared;
pub mod stats;
mod unique;
mod unique_type;
pub mod world;

pub use unique::{Unique, UniqueKind};
pub use unique_type::{UniqueTarget, UniqueType};
