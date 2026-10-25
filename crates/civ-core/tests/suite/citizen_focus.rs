// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A city's focus and pinned tiles (Unciv's `CityFocus` and `lockedTiles`),
//! and the choice a human makes about a city just taken.
use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::stats::Stat;
use civ_core::world::commands::{execute, Command, CommandRejection};
use civ_core::world::conquest::CaptureChoice;
use civ_core::world::turn::{finish_round, update_visibility, TurnModifiers};
use civ_core::world::{City, CityFocus, CivId, Civilization, TileGrid, TileId, World};

/// Rome (civ 0) at (4, 8) with its first ring, all grassland but one plains
/// tile east of it; Antium (civ 1) far off. Both civs are human.
fn fixture() -> (Ruleset, World, Id<City>, TileId) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let nation = r.nations.id("Rome").unwrap();
    let civs = vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)];
    let mut w = World::new(TileGrid::new(24, 18, r.terrain("Grassland").unwrap()), civs);
    let rome = w.found_city_in(&r, City::found("Rome", CivId(0), w.grid.id_at(4, 8).unwrap()));
    let antium = w.found_city_in(&r, City::found("Antium", CivId(1), w.grid.id_at(18, 8).unwrap()));
    for city in [rome, antium] {
        let centre = w.cities.get(city).unwrap().centre;
        for tile in w.grid.within(centre, 1) {
            w.claim_tile(city, tile);
        }
    }
    let plains = w.grid.id_at(5, 8).unwrap();
    w.grid.terrain[plains.0 as usize] = r.terrain("Plains").unwrap();
    update_visibility(&mut w);
    civ_core::world::fog::capture_observations(&mut w, &r);
    civ_core::world::assignment::place_idle_citizens(&mut w, &r, rome);
    (r, w, rome, plains)
}

fn worked(w: &World, city: Id<City>) -> Vec<TileId> {
    w.cities.get(city).unwrap().worked.clone()
}

fn round(w: &mut World, r: &Ruleset) {
    finish_round(w, r, CityFocus::None, TurnModifiers::for_world(w, r));
}

#[test]
fn a_focus_places_citizens_at_once_and_again_every_turn() {
    let (r, mut w, rome, plains) = fixture();
    // A small city wants to grow: the grassland beats the plains.
    let grown = worked(&w, rome);
    assert_eq!(grown.len(), 1);
    assert_ne!(grown[0], plains);

    let production = CityFocus::Single(Stat::Production);
    execute(&mut w, &r, CivId(0), Command::SetCityFocus { city: rome, focus: production }).unwrap();
    assert_eq!(w.cities.get(rome).unwrap().focus, production);
    assert_eq!(worked(&w, rome), vec![plains], "production counts three times over");

    // A direct pick in a focused city lasts only until the turn places everyone again.
    execute(
        &mut w,
        &r,
        CivId(0),
        Command::AssignCitizens { city: rome, worked: grown.clone(), specialists: Default::default() },
    )
    .unwrap();
    assert_eq!(worked(&w, rome), grown);
    round(&mut w, &r);
    assert_eq!(worked(&w, rome), vec![plains]);

    // Back to balanced with no pins: the player's own picks stay where they are, as before.
    execute(&mut w, &r, CivId(0), Command::SetCityFocus { city: rome, focus: CityFocus::None }).unwrap();
    assert!(!w.cities.get(rome).unwrap().is_managed());
    assert_ne!(worked(&w, rome), vec![plains]);
    execute(
        &mut w,
        &r,
        CivId(0),
        Command::AssignCitizens { city: rome, worked: vec![plains], specialists: Default::default() },
    )
    .unwrap();
    round(&mut w, &r);
    assert_eq!(worked(&w, rome), vec![plains]);
}

#[test]
fn a_pinned_tile_is_worked_first_and_released_last() {
    let (r, mut w, rome, plains) = fixture();
    let production = CityFocus::Single(Stat::Production);
    execute(&mut w, &r, CivId(0), Command::SetCityFocus { city: rome, focus: production }).unwrap();
    let centre = w.cities.get(rome).unwrap().centre;
    let grass: Vec<TileId> = w.grid.neighbours(centre).filter(|t| *t != plains).collect();

    let pin = |w: &mut World, tile: TileId, locked: bool| {
        execute(w, &r, CivId(0), Command::LockTile { city: rome, tile, locked })
    };
    pin(&mut w, grass[0], true).unwrap();
    assert_eq!(worked(&w, rome), vec![grass[0]], "the pin wins over the focus");
    assert_eq!(w.cities.get(rome).unwrap().locked_tiles, vec![grass[0]]);
    assert_eq!(pin(&mut w, grass[1], true), Err(CommandRejection::NotAllowed), "one pin per citizen");
    assert_eq!(pin(&mut w, centre, true), Err(CommandRejection::InvalidTarget), "the centre works for free");
    let far = w.grid.id_at(12, 8).unwrap();
    assert_eq!(pin(&mut w, far, true), Err(CommandRejection::InvalidTarget), "not Rome's to work");
    round(&mut w, &r);
    assert_eq!(worked(&w, rome), vec![grass[0]], "pins hold through the turn");

    // A second citizen follows the focus around the pin.
    w.cities.get_mut(rome).unwrap().population = 2;
    pin(&mut w, grass[0], true).unwrap();
    assert_eq!(worked(&w, rome), vec![grass[0], plains]);
    pin(&mut w, grass[1], true).unwrap();
    assert_eq!(worked(&w, rome), vec![grass[0], grass[1]]);

    // A city that shrinks below its pins keeps the first and lets the rest go.
    w.cities.get_mut(rome).unwrap().population = 1;
    civ_core::world::assignment::reassign_citizens(&mut w, &r, rome);
    assert_eq!(worked(&w, rome), vec![grass[0]]);
    assert_eq!(w.cities.get(rome).unwrap().locked_tiles, vec![grass[0]]);

    pin(&mut w, grass[0], false).unwrap();
    assert_eq!(worked(&w, rome), vec![plains], "unpinned, the focus decides again");
    assert!(w.cities.get(rome).unwrap().locked_tiles.is_empty());
    assert_eq!(pin(&mut w, grass[0], false), Err(CommandRejection::NotAllowed), "nothing to unpin");

    // A direct assignment that leaves a pinned tile out unpins it.
    pin(&mut w, grass[2], true).unwrap();
    execute(
        &mut w,
        &r,
        CivId(0),
        Command::AssignCitizens { city: rome, worked: vec![plains], specialists: Default::default() },
    )
    .unwrap();
    assert!(w.cities.get(rome).unwrap().locked_tiles.is_empty());
}

#[test]
fn a_managed_city_leaves_its_sisters_tiles_alone() {
    let (r, mut w, rome, plains) = fixture();
    // Ostia shares the plains tile with Rome and works it.
    let ostia = w.found_city_in(&r, City::found("Ostia", CivId(0), w.grid.id_at(6, 8).unwrap()));
    w.claim_tile(ostia, w.grid.id_at(6, 8).unwrap());
    w.claim_tile(ostia, plains);
    w.cities.get_mut(ostia).unwrap().worked = vec![plains];
    let production = CityFocus::Single(Stat::Production);
    execute(&mut w, &r, CivId(0), Command::SetCityFocus { city: rome, focus: production }).unwrap();
    assert!(!worked(&w, rome).contains(&plains));
    assert_eq!(
        execute(&mut w, &r, CivId(0), Command::LockTile { city: rome, tile: plains, locked: true }),
        Err(CommandRejection::InvalidTarget)
    );
    round(&mut w, &r);
    assert!(!worked(&w, rome).contains(&plains));
}

#[test]
fn a_puppet_takes_no_focus_or_pins_and_a_new_owner_starts_afresh() {
    let (r, mut w, rome, plains) = fixture();
    execute(&mut w, &r, CivId(0), Command::LockTile { city: rome, tile: plains, locked: true }).unwrap();
    execute(&mut w, &r, CivId(0), Command::SetCityFocus { city: rome, focus: CityFocus::Single(Stat::Gold) }).unwrap();
    w.cities.get_mut(rome).unwrap().puppet = true;
    assert_eq!(
        execute(&mut w, &r, CivId(0), Command::SetCityFocus { city: rome, focus: CityFocus::None }),
        Err(CommandRejection::NotAllowed)
    );
    assert_eq!(
        execute(&mut w, &r, CivId(0), Command::LockTile { city: rome, tile: plains, locked: false }),
        Err(CommandRejection::NotAllowed)
    );
    civ_core::world::conquest::transfer_city(&mut w, &r, rome, CivId(1)).unwrap();
    let c = w.cities.get(rome).unwrap();
    assert_eq!(c.focus, CityFocus::None);
    assert!(c.locked_tiles.is_empty());
}

#[test]
fn a_human_answers_a_capture_this_turn_or_keeps_the_puppet() {
    let (r, mut w, _, _) = fixture();
    // Antium keeps Veii, so taking Veii does not end civ 1.
    let veii = w.found_city_in(&r, City::found("Veii", CivId(1), w.grid.id_at(14, 8).unwrap()));
    w.claim_tile(veii, w.grid.id_at(14, 8).unwrap());
    w.cities.get_mut(veii).unwrap().population = 3;
    civ_core::world::combat::capture_city(&mut w, &r, veii, CivId(0));
    let c = w.cities.get(veii).unwrap();
    assert!(c.puppet && c.capture_pending);
    assert_eq!(
        civ_core::world::conquest::capture_choices(&w, veii),
        [CaptureChoice::Annex, CaptureChoice::Keep, CaptureChoice::Raze]
    );
    let mut kept = w.clone();
    execute(&mut kept, &r, CivId(0), Command::KeepPuppet { city: veii }).unwrap();
    assert!(kept.cities.get(veii).is_some_and(|c| c.puppet && !c.capture_pending));
    assert_eq!(execute(&mut kept, &r, CivId(0), Command::KeepPuppet { city: veii }), Err(CommandRejection::NotAllowed));

    let mut annexed = w.clone();
    execute(&mut annexed, &r, CivId(0), Command::Annex { city: veii }).unwrap();
    assert!(annexed.cities.get(veii).is_some_and(|c| !c.puppet && !c.capture_pending));
    assert!(civ_core::world::conquest::capture_choices(&annexed, veii).is_empty());

    // Unanswered, it stays a puppet once the round closes.
    round(&mut w, &r);
    assert!(w.cities.get(veii).is_some_and(|c| c.puppet && !c.capture_pending));
}

#[test]
fn an_ai_conqueror_decides_at_once() {
    let (r, mut w, _, _) = fixture();
    w.civ_mut(CivId(0)).unwrap().is_ai = true;
    let veii = w.found_city_in(&r, City::found("Veii", CivId(1), w.grid.id_at(14, 8).unwrap()));
    w.claim_tile(veii, w.grid.id_at(14, 8).unwrap());
    civ_core::world::combat::capture_city(&mut w, &r, veii, CivId(0));
    assert!(w.cities.get(veii).is_none_or(|c| !c.capture_pending));
}

#[test]
fn a_tile_is_priced_before_the_treasury_is_asked() {
    let (r, mut w, rome, _) = fixture();
    let centre = w.cities.get(rome).unwrap().centre;
    let tile = w.grid.within(centre, 2).into_iter().find(|t| w.grid.distance(centre, *t) == 2).unwrap();
    let gold = TurnModifiers::for_world(&w, &r).costs.gold_cost;
    w.civ_mut(CivId(0)).unwrap().gold = 0;
    let price = civ_core::world::expansion::tile_price(&w, &r, rome, tile, gold).unwrap();
    assert!(price > 0);
    assert_eq!(
        civ_core::world::expansion::buy_tile(&mut w, &r, rome, tile, gold),
        Err(civ_core::world::expansion::BuyTileRejection::NotEnoughGold { cost: price, gold: 0 })
    );
    w.civ_mut(CivId(0)).unwrap().gold = price as i32;
    assert_eq!(civ_core::world::expansion::buy_tile(&mut w, &r, rome, tile, gold), Ok(price));
}
