// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The ruleset: what a game is played with, as opposed to what is happening in it.
//!
//! Immutable once loaded, shared by every system, and identical on server and
//! client. [`generated`] holds the data types, derived from the json the game
//! actually ships rather than hand-written from the Kotlin — the schema is the
//! data's, so inferring it is more faithful than transcribing it.

pub mod civgame;
pub mod generated;
pub mod load;
pub mod queries;

pub use generated::*;
pub use load::{DanglingRef, LoadError, Ruleset};
pub use queries::SortGroup;

/// A json value this crate does not model yet.
///
/// A handful of ruleset fields hold free-form objects (difficulty's starting
/// unit tables, event payloads). They are named rather than silently dropped,
/// so the gap is visible in the type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unstructured;
