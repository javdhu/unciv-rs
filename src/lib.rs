//! Deterministic rules core.
//!
//! Compiles to native (authoritative server) and `wasm32` (browser client)
//! from one source, so the two cannot disagree. Nothing here touches I/O,
//! the clock, or floating point.

pub mod arena;
pub mod generated;
pub mod json;
pub mod modifiers;
pub mod rng;
pub mod ruleset;
pub mod stats;
pub mod world;
pub mod fingerprint;
pub mod icons;
mod unique_type;
mod unique;

pub use unique_type::{UniqueTarget, UniqueType};
pub use unique::{Unique, UniqueKind};
