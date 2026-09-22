//! Mutable game state: the map, who owns what, and who can see it.
//!
//! Split from [`ruleset`](crate::ruleset), which is what a game is played
//! *with* and never changes once loaded. Everything here changes every turn.

pub mod ai;
pub mod assignment;
pub mod barbarians;
pub mod city;
pub mod city_states;
pub mod combat;
pub mod conditionals;
pub mod connections;
pub mod conquest;
pub mod construction;
pub mod diplomacy;
pub mod economy;
pub mod entities;
pub mod espionage;
pub mod expansion;
pub mod filters;
pub mod fog;
pub mod great_people;
pub mod grid;
pub mod happiness;
pub mod improvements;
pub mod mapgen;
pub mod movement;
pub mod policies;
pub mod purchase;
pub mod quests;
pub mod religion;
pub mod research;
pub mod resources;
pub mod settling;
pub mod specialists;
pub mod turn;
pub mod unique_index;
pub mod victory;
pub mod world;
pub mod yields;

pub use assignment::{assign_idle_citizens, best_tile_for_new_citizen, CityFocus, Ranking};
pub use city::{apply_food, City, GrowthEvent, GrowthModifiers};
pub use entities::{Civilization, Unit};
pub use fog::{TileMemory, Visibility};
pub use grid::{CivId, Direction, TileGrid, TileId};
pub use turn::{advance_turn, TurnEvent, TurnModifiers, TurnPlan};
pub use world::World;
pub use yields::{tile_yield, tile_yield_breakdown};
