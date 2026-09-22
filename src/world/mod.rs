//! Mutable game state: the map, who owns what, and who can see it.
//!
//! Split from [`ruleset`](crate::ruleset), which is what a game is played
//! *with* and never changes once loaded. Everything here changes every turn.

pub mod ai;
pub mod assignment;
pub mod city;
pub mod combat;
pub mod conditionals;
pub mod construction;
pub mod filters;
pub mod fog;
pub mod happiness;
pub mod improvements;
pub mod grid;
pub mod diplomacy;
pub mod entities;
pub mod movement;
pub mod policies;
pub mod research;
pub mod resources;
pub mod settling;
pub mod turn;
pub mod unique_index;
pub mod victory;
pub mod world;
pub mod yields;

pub use assignment::{assign_idle_citizens, best_tile_for_new_citizen, CityFocus, Ranking};
pub use city::{apply_food, City, GrowthEvent, GrowthModifiers};
pub use fog::{TileMemory, Visibility};
pub use grid::{CivId, Direction, TileGrid, TileId};
pub use entities::{Civilization, Unit};
pub use turn::{advance_turn, TurnEvent, TurnPlan};
pub use world::World;
pub use yields::{tile_yield, tile_yield_breakdown};
