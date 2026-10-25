// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The static rule tables, asked of the host once per match (docs/CLIENT-PROTOCOL.md: rules travel once with a
//! content hash), and the lookups the map draws from: what each terrain id is, resource kinds, names, nation
//! colours.

use crate::client::QueryAnswered;
use crate::link::{Link, Up};
use crate::view_state::ViewState;
use bevy::prelude::*;
use civ_sim::protocol::{ClientMessage, Query, QueryResult};
use civ_sim::RuleTables;
use std::collections::HashMap;

pub const RULES_REQUEST: &str = "rules";

/// How a terrain id draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ground {
    Ocean,
    Coast,
    Lake,
    Grassland,
    Plains,
    Desert,
    Tundra,
    Snow,
    Mountain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Hill,
    Forest,
    Jungle,
    Marsh,
    Fallout,
    Oasis,
    FloodPlains,
    Ice,
    Atoll,
    /// A natural wonder, drawn as a marker until it gets a model.
    Wonder,
    /// Anything the client does not draw yet.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Bonus,
    Luxury,
    Strategic,
}

#[derive(Resource)]
pub struct Rules {
    pub content_hash: String,
    pub tables: RuleTables,
    grounds: HashMap<u32, Ground>,
    features: HashMap<u32, Feature>,
    terrain_names: HashMap<u32, String>,
    resources: HashMap<u32, (String, ResourceKind)>,
    improvements: HashMap<u32, String>,
    nations: HashMap<u32, (String, Color, Color)>,
}

impl Rules {
    pub fn new(content_hash: String, tables: RuleTables) -> Rules {
        let mut grounds = HashMap::new();
        let mut features = HashMap::new();
        let mut terrain_names = HashMap::new();
        for t in &tables.terrains {
            terrain_names.insert(t.id, t.name.clone());
            let ground = match t.name.as_str() {
                "Ocean" => Some(Ground::Ocean),
                "Coast" => Some(Ground::Coast),
                "Lakes" => Some(Ground::Lake),
                "Grassland" => Some(Ground::Grassland),
                "Plains" => Some(Ground::Plains),
                "Desert" => Some(Ground::Desert),
                "Tundra" => Some(Ground::Tundra),
                "Snow" => Some(Ground::Snow),
                "Mountain" => Some(Ground::Mountain),
                _ => None,
            };
            if let Some(g) = ground {
                grounds.insert(t.id, g);
                continue;
            }
            let feature = match (t.name.as_str(), t.kind.as_str()) {
                (_, "NaturalWonder") => Feature::Wonder,
                ("Hill", _) => Feature::Hill,
                ("Forest", _) => Feature::Forest,
                ("Jungle", _) => Feature::Jungle,
                ("Marsh", _) => Feature::Marsh,
                ("Fallout", _) => Feature::Fallout,
                ("Oasis", _) => Feature::Oasis,
                ("Flood plains", _) => Feature::FloodPlains,
                ("Ice", _) => Feature::Ice,
                ("Atoll", _) => Feature::Atoll,
                _ => Feature::Other,
            };
            features.insert(t.id, feature);
        }
        let resources = tables
            .resources
            .iter()
            .map(|r| {
                let kind = match r.kind.as_str() {
                    "Luxury" => ResourceKind::Luxury,
                    "Strategic" => ResourceKind::Strategic,
                    _ => ResourceKind::Bonus,
                };
                (r.id, (r.name.clone(), kind))
            })
            .collect();
        let improvements = tables.improvements.iter().map(|i| (i.id, i.name.clone())).collect();
        let colour = |c: &[i32]| match c {
            [r, g, b, ..] => Color::srgb_u8(*r as u8, *g as u8, *b as u8),
            _ => Color::srgb_u8(0x80, 0x80, 0x80),
        };
        let nations = tables
            .nations
            .iter()
            .map(|n| (n.id, (n.name.clone(), colour(&n.outer_color), colour(&n.inner_color))))
            .collect();
        Rules { content_hash, tables, grounds, features, terrain_names, resources, improvements, nations }
    }

    /// Unknown ids draw as grassland rather than failing the map.
    pub fn ground(&self, terrain: u32) -> Ground {
        self.grounds.get(&terrain).copied().unwrap_or(Ground::Grassland)
    }

    pub fn feature(&self, terrain: u32) -> Feature {
        self.features.get(&terrain).copied().unwrap_or(Feature::Other)
    }

    pub fn terrain_name(&self, terrain: u32) -> &str {
        self.terrain_names.get(&terrain).map_or("?", String::as_str)
    }

    pub fn resource(&self, id: u32) -> Option<(&str, ResourceKind)> {
        self.resources.get(&id).map(|(n, k)| (n.as_str(), *k))
    }

    pub fn improvement(&self, id: u32) -> &str {
        self.improvements.get(&id).map_or("?", String::as_str)
    }

    /// Name, outer colour and inner colour of a nation.
    pub fn nation(&self, id: u32) -> Option<(&str, Color, Color)> {
        self.nations.get(&id).map(|(n, o, i)| (n.as_str(), *o, *i))
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(PreUpdate, (ask, receive).after(crate::client::receive).in_set(crate::app::InMatch));
}

/// Ask when the rules held are not the host's, once per connection (an answer lost with a socket is asked
/// for again after the next welcome).
fn ask(link: Res<Link>, view: Res<ViewState>, rules: Option<Res<Rules>>, mut asked: Local<Option<(String, u32)>>) {
    let Some(hash) = view.content_hash() else { return };
    let held = rules.is_some_and(|r| r.content_hash == hash);
    let this = (hash.to_string(), view.welcomes());
    if !held && asked.as_ref() != Some(&this) && view.view().is_some() {
        *asked = Some(this);
        link.send(Up::Client(ClientMessage::Query { request_id: RULES_REQUEST.into(), query: Query::Rules }));
    }
}

fn receive(mut commands: Commands, mut answers: MessageReader<QueryAnswered>) {
    for answer in answers.read() {
        if answer.request_id != RULES_REQUEST {
            continue;
        }
        if let QueryResult::Rules { content_hash, tables } = &answer.reply.result {
            commands.insert_resource(Rules::new(content_hash.clone(), tables.clone()));
        }
    }
}
