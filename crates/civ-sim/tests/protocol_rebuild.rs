//! What the client learns of civ.game's systems (docs/REBUILD.md, Engine
//! step 6), and how the game's words read to a player (step 5).

use civ_core::names::NAMES;
use civ_sim::{advance, content, new_game_in, player_view, tables, Settings};

#[test]
fn rule_tables_list_the_faiths_with_their_traits() {
    let r = content::load().unwrap();
    let t = tables(&r);
    let faiths = t
        .faiths
        .as_ref()
        .expect("the shipped rules raise faiths by era");
    assert_eq!(faiths.len(), 11);
    assert_eq!(faiths[0].key, 128);
    assert_eq!(faiths[0].name, "Hinduism");
    assert_eq!(faiths[0].homeland, ["Maurya", "Chola"]);
    for f in faiths {
        let kinds: Vec<_> = f
            .traits
            .iter()
            .map(|id| {
                t.beliefs
                    .iter()
                    .find(|b| b.id == *id)
                    .unwrap()
                    .kind
                    .as_str()
            })
            .collect();
        assert_eq!(kinds, ["Follower", "State"], "{}", f.name);
    }
}

#[test]
fn effects_read_with_civ_games_names() {
    let t = tables(&content::load().unwrap());
    let texts = t
        .buildings
        .iter()
        .flat_map(|b| &b.effects)
        .chain(t.beliefs.iter().flat_map(|b| &b.effects))
        .chain(
            t.nations
                .iter()
                .flat_map(|n| n.leaders.iter().flatten())
                .flat_map(|l| &l.effects),
        );
    for text in texts {
        for (civ_word, _) in NAMES {
            assert!(!text.contains(civ_word), "{text:?} still says {civ_word:?}");
        }
    }
}

#[test]
fn a_base_game_shows_trade_influence_and_luminary_names() {
    let settings = Settings {
        seed: 3,
        majors: 4,
        city_states: 2,
        ..Settings::default()
    };
    let (mut w, r, me) = new_game_in(&settings, &[], content::load().unwrap()).unwrap();
    for _ in 0..40 {
        advance(&mut w, &r);
    }
    let v = player_view(&w, &r, me);
    let trade = v.trade.as_ref().expect("Reserve Currency is a victory");
    assert!(trade.shares.iter().any(|s| s.civ == me.0));
    assert!(!trade.history.is_empty() && trade.history.len() <= 20);
    for rel in v
        .relations
        .iter()
        .filter(|rel| w.civs[usize::from(rel.civ)].is_major())
    {
        let influence = rel.influence.as_ref().expect("Lingua Franca is a victory");
        assert!(influence.rate <= 100);
    }
    // Every Luminary on the map carries a name, and none twice.
    let names: Vec<_> = w.units.iter().filter_map(|(_, u)| u.name.clone()).collect();
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(names.len(), unique.len());
    let json = serde_json::to_value(&v).unwrap();
    assert!(json.get("trade").is_some());
}

/// The shipped rules with the Great Wall standing on land, as Content's
/// phase 2 data will have it.
fn placed_rules() -> civ_core::ruleset::Ruleset {
    let mut r = content::load().unwrap();
    let wall = r.buildings.id("Great Wall").unwrap();
    r.buildings
        .get_mut(wall)
        .unwrap()
        .uniques
        .push(civ_core::Unique::parse("Stands on a [Land] tile"));
    r
}

#[test]
fn construction_choices_offer_sites_and_the_view_shows_what_stands_where() {
    let settings = Settings {
        seed: 4,
        majors: 2,
        city_states: 0,
        ..Settings::default()
    };
    let (mut w, r, me) = new_game_in(&settings, &[], placed_rules()).unwrap();
    for _ in 0..3 {
        advance(&mut w, &r);
    }
    let city = w.civs[usize::from(me.0)].cities[0];
    let wall = r.buildings.id("Great Wall").unwrap();
    let centre = w.cities.get(city).unwrap().centre;
    for t in w.grid.within(centre, 2) {
        w.claim_tile(city, t);
    }
    // Everything the Great Wall needs, so it is offered.
    let engineering = r.technologies.id("Engineering").unwrap();
    w.civs[usize::from(me.0)].techs.insert(engineering);
    let options =
        civ_sim::queries::city_options(&w, &r, me, city.index(), city.generation()).unwrap();
    let choice = options
        .choices
        .iter()
        .find(|c| matches!(c.construction, civ_sim::protocol::ConstructionRef::Building { id } if id == wall.index()));
    let sites = choice
        .expect("the Great Wall is offered")
        .sites
        .as_ref()
        .expect("the wall stands on a tile");
    assert!(sites.takes_tile && !sites.tiles.is_empty());
    assert!(sites.tiles.contains(&sites.selected.unwrap()));

    let c = w.cities.get_mut(city).unwrap();
    c.built.insert(wall);
    civ_core::world::placement::settle(&mut w, &r);
    let site = civ_core::world::placement::standing(&w, city, wall).expect("placed");
    civ_core::world::fog::capture_observations(&mut w, &r);
    let v = player_view(&w, &r, me);
    let tile = v.tiles.iter().find(|t| t.id == site.0).unwrap();
    let placed = tile
        .placed
        .as_ref()
        .expect("the renderer is told what stands here");
    assert_eq!(
        (placed.building, placed.wonder, placed.canal),
        (wall.index(), true, false)
    );
    let walls = v
        .walls
        .as_ref()
        .expect("the Great Wall draws along the border");
    assert!(walls
        .iter()
        .any(|x| x.owner == me.0 && x.building == wall.index() && !x.tiles.is_empty()));
}

/// What the city offers is what the queue accepts: the World Charter waits
/// for the Internationalism tree (Content's rule), so it is not offered at
/// the start (a bug seen in the browser).
#[test]
fn construction_choices_agree_with_the_queue_check() {
    let settings = Settings {
        seed: 2,
        majors: 2,
        city_states: 0,
        ..Settings::default()
    };
    let (mut w, r, me) = new_game_in(&settings, &[], content::load().unwrap()).unwrap();
    for _ in 0..3 {
        advance(&mut w, &r);
    }
    let city = w.civs[usize::from(me.0)].cities[0];
    let options =
        civ_sim::queries::city_options(&w, &r, me, city.index(), city.generation()).unwrap();
    let c = w.cities.get(city).unwrap();
    for choice in &options.choices {
        let what = match choice.construction {
            civ_sim::protocol::ConstructionRef::Building { id } => {
                civ_core::world::construction::Construction::Building(
                    r.buildings.ids().nth(id as usize).unwrap(),
                )
            }
            civ_sim::protocol::ConstructionRef::Unit { id } => {
                civ_core::world::construction::Construction::Unit(
                    r.units.ids().nth(id as usize).unwrap(),
                )
            }
        };
        let queue = civ_core::world::construction::can_construct_in(&w, &r, me, c, what);
        let buy = civ_core::world::construction::can_purchase_in(&w, &r, me, c, what);
        assert!(
            queue.is_ok() || buy.is_ok(),
            "{} is offered but refused: {queue:?}",
            choice.name
        );
    }
    if let Some(charter) = r.buildings.id("World Charter") {
        assert!(!options.choices.iter().any(|c| c.construction
            == civ_sim::protocol::ConstructionRef::Building {
                id: charter.index()
            }));
    }
}

/// Every effect the rule tables send reads as prose: no brackets, braces or
/// angle-bracketed conditionals, nothing meant only for the engine.
#[test]
fn every_effect_reads_as_prose() {
    let t = tables(&content::load().unwrap());
    let effects: Vec<&String> = t
        .buildings
        .iter()
        .flat_map(|b| &b.effects)
        .chain(t.beliefs.iter().flat_map(|b| &b.effects))
        .chain(
            t.nations
                .iter()
                .flat_map(|n| n.leaders.iter().flatten())
                .flat_map(|l| &l.effects),
        )
        .collect();
    assert!(
        effects.len() > 200,
        "the base effects are all here: {}",
        effects.len()
    );
    for text in effects {
        assert!(
            !text
                .chars()
                .any(|c| matches!(c, '[' | ']' | '{' | '}' | '<' | '>')),
            "{text:?}"
        );
        assert!(
            !text.contains("Suppress warning") && !text.contains("  ") && text.trim() == text,
            "{text:?}"
        );
        assert!(!text.is_empty());
    }
}
