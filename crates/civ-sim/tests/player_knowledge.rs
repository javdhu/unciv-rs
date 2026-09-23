use civ_core::arena::Id;
use civ_core::fingerprint::embedded_sources;
use civ_core::ruleset::Ruleset;
use civ_core::world::city::City;
use civ_core::world::entities::Civilization;
use civ_core::world::fog::capture_observations;
use civ_core::world::grid::{RoadLevel, TileGrid};
use civ_core::world::{CivId, TileId, Unit, World};
use civ_sim::{player_view, reachable};

const ME: CivId = CivId(0);
const OTHER: CivId = CivId(1);

fn fixture() -> (World, Ruleset, Id<Unit>, Id<City>) {
    let r = Ruleset::from_sources(&embedded_sources()).unwrap();
    let civs = ["Rome", "Greece", "Egypt"]
        .into_iter()
        .enumerate()
        .map(|(i, n)| Civilization::new(CivId(i as u8), r.nations.id(n).unwrap(), false))
        .collect();
    let mut w = World::new(TileGrid::new(20, 8, r.terrain("Grassland").unwrap()), civs);
    let start = w.grid.id_at(2, 3).unwrap();
    let scout = w.place_unit(Unit::new(r.units.id("Scout").unwrap(), ME, start, 4));
    let city_tile = w.grid.id_at(12, 3).unwrap();
    let city = w.found_city(City::found("Remembered City", OTHER, city_tile));
    w.cities.get_mut(city).unwrap().population = 3;
    w.place_unit(Unit::new(
        r.units.id("Warrior").unwrap(),
        OTHER,
        city_tile,
        2,
    ));
    for t in [start, w.grid.id_at(3, 3).unwrap(), city_tile] {
        w.visibility.observe(ME, t);
    }
    capture_observations(&mut w, &r);
    w.visibility.clear_visible(ME);
    w.visibility.observe(ME, start);
    capture_observations(&mut w, &r);
    (w, r, scout, city)
}

#[test]
fn hidden_changes_leave_both_player_view_and_reachability_unchanged() {
    let (w, r, scout, city) = fixture();
    let expected_view = player_view(&w, &r, ME);
    let expected_reach = reachable(&w, &r, ME, scout.index(), scout.generation());
    assert!(
        expected_reach.iter().any(|p| p.uncertain),
        "the preview must expose uncertainty"
    );
    let mut changed = w.clone();
    changed.cities.get_mut(city).unwrap().population = 19;
    changed.cities.get_mut(city).unwrap().name = "Unseen Rename".into();
    changed
        .cities
        .get_mut(city)
        .unwrap()
        .production_progress
        .insert(
            civ_core::world::construction::Construction::Building(
                r.buildings.id("Monument").unwrap(),
            ),
            30,
        );
    changed.cities.get_mut(city).unwrap().production_overflow = 15;
    changed.civ_mut(OTHER).unwrap().gold = 42_000;
    changed.civ_mut(CivId(2)).unwrap().nation = r.nations.id("China").unwrap();
    let fog = changed.grid.id_at(3, 3).unwrap();
    let unknown = changed.grid.id_at(4, 3).unwrap();
    for tile in [fog, unknown] {
        changed.grid.terrain[tile.0 as usize] = r.terrain("Mountain").unwrap();
        changed.grid.road[tile.0 as usize] = RoadLevel::Railroad;
        changed.grid.owner[tile.0 as usize] = Some(OTHER);
        changed.place_unit(Unit::new(r.units.id("Warrior").unwrap(), OTHER, tile, 2));
    }
    capture_observations(&mut changed, &r);
    assert_eq!(player_view(&changed, &r, ME), expected_view);
    assert_eq!(
        reachable(&changed, &r, ME, scout.index(), scout.generation()),
        expected_reach
    );
    assert!(
        expected_view.civs.iter().all(|c| c.id != 2),
        "unknown nations must not appear in the view"
    );
}

#[test]
fn owned_city_progress_is_per_item_and_never_exposed_to_an_observer() {
    use civ_core::world::construction::Construction;
    let (mut w, r, _, _) = fixture();
    let centre = w.grid.id_at(2, 3).unwrap();
    let city = w.found_city(City::found("Production", ME, centre));
    let monument = Construction::Building(r.buildings.id("Monument").unwrap());
    let granary = Construction::Building(r.buildings.id("Granary").unwrap());
    let c = w.cities.get_mut(city).unwrap();
    c.queue = vec![monument];
    c.production_progress.insert(monument, 15);
    c.production_progress.insert(granary, 9);
    c.production_overflow = 4;
    w.visibility.observe(OTHER, centre);
    capture_observations(&mut w, &r);
    let own = player_view(&w, &r, ME)
        .cities
        .into_iter()
        .find(|c| c.id == city.index())
        .unwrap();
    assert_eq!(own.production_stored, Some(15));
    assert_eq!(own.production_overflow, Some(4));
    assert_eq!(own.production_progress.as_ref().unwrap().len(), 2);
    let observed = player_view(&w, &r, OTHER)
        .cities
        .into_iter()
        .find(|c| c.id == city.index())
        .unwrap();
    assert!(observed.production_stored.is_none());
    assert!(observed.production_overflow.is_none());
    assert!(observed.production_progress.is_none());
    w.cities.get_mut(city).unwrap().queue = vec![granary];
    let switched = player_view(&w, &r, ME)
        .cities
        .into_iter()
        .find(|c| c.id == city.index())
        .unwrap();
    assert_eq!(switched.production_stored, Some(9));
    assert_eq!(switched.production_progress, own.production_progress);
}

#[test]
fn a_fresh_view_retains_the_victory_condition_and_original_win_turn() {
    let (mut w, r, _) = civ_sim::new_game(&civ_sim::Settings::default()).unwrap();
    let condition = r.victories.id("Domination").unwrap();
    w.winner = Some((OTHER, condition));
    w.victory_turn = Some(7);
    w.turn = 12; // The legacy observer may keep simulating after the outcome.
    let snapshot = civ_core::save::encode(&w, &r).unwrap();
    let restored = civ_core::save::decode(&snapshot, &r).unwrap();
    let view = player_view(&restored, &r, ME);
    let result = view.result.unwrap();
    assert_eq!(view.winner, Some(OTHER.0));
    assert_eq!(result.winner, OTHER.0);
    assert_eq!(result.condition, condition.index());
    assert_eq!(result.turn, 7);
    assert!(civ_sim::tables(&r)
        .victories
        .iter()
        .any(|v| v.id == condition.index() && v.name == "Domination"));
}

#[test]
fn a_fogged_city_keeps_its_last_observation_until_the_ground_is_seen_again() {
    let (mut w, r, _, city) = fixture();
    let centre = w.cities.get(city).unwrap().centre;
    let remembered = player_view(&w, &r, ME)
        .cities
        .into_iter()
        .find(|c| c.centre == centre.0)
        .unwrap();
    assert_eq!(remembered.population, 3);
    assert!(!remembered.visible);
    w.cities.remove(city);
    capture_observations(&mut w, &r);
    assert_eq!(
        player_view(&w, &r, ME).cities[0],
        remembered,
        "unseen destruction is not known"
    );
    w.visibility.observe(ME, centre);
    capture_observations(&mut w, &r);
    assert!(
        player_view(&w, &r, ME).cities.is_empty(),
        "observed empty ground clears stale city memory"
    );
}

#[test]
fn resource_revelation_uses_the_players_technology_and_observed_deposit() {
    let (mut w, r, scout, _) = fixture();
    let (resource, tech) = r
        .resources
        .iter()
        .find_map(|(id, resource)| Some((id, r.technologies.id(resource.revealed_by.as_deref()?)?)))
        .unwrap();
    let tile = w.units.get(scout).unwrap().position;
    w.grid.resource[tile.0 as usize] = Some(resource);
    capture_observations(&mut w, &r);
    let shown = |w: &World| {
        player_view(w, &r, ME)
            .tiles
            .into_iter()
            .find(|t| t.id == tile.0)
            .unwrap()
            .resource
    };
    assert_eq!(shown(&w), None);
    w.visibility.clear_visible(ME);
    // The deposit on the hidden live map changes. Technology reveals what was
    // observed, without rereading the fogged tile.
    w.grid.resource[tile.0 as usize] = None;
    w.civ_mut(ME).unwrap().techs.insert(tech);
    capture_observations(&mut w, &r);
    assert_eq!(shown(&w), Some(resource.index()));
}

#[test]
fn map_reveal_does_not_reveal_a_city_or_live_roads_and_ownership() {
    let (mut w, r, _, _) = fixture();
    let tile = TileId(140);
    w.found_city(City::found("Secret", CivId(2), tile));
    w.grid.road[tile.0 as usize] = RoadLevel::Railroad;
    w.visibility.explore(ME, tile);
    capture_observations(&mut w, &r);
    let view = player_view(&w, &r, ME);
    let revealed = view.tiles.iter().find(|t| t.id == tile.0).unwrap();
    assert_eq!((revealed.owner, revealed.road), (None, 0));
    assert!(view.cities.iter().all(|c| c.centre != tile.0));
    assert!(view.civs.iter().all(|c| c.id != 2));
}

/// A human's automated units decide from their civ's knowledge: rewriting
/// everything that civ has never explored leaves their orders unchanged.
#[test]
fn automated_units_decide_the_same_whatever_the_fog_hides() {
    use civ_core::world::automation::{order, Automation};
    use civ_core::world::commands::{execute, Command};
    let mut caught = false;
    for seed in [1, 2, 3, 5, 8] {
        let (mut w, r, me) = civ_sim::new_game(&civ_sim::Settings {
            seed,
            ..civ_sim::Settings::default()
        })
        .unwrap();
        let settler = w
            .units
            .iter()
            .find(|(_, u)| u.owner == me && civ_core::world::settling::is_settler(&r, u.base))
            .unwrap()
            .0;
        execute(&mut w, &r, me, Command::FoundCity { unit: settler, name: "Home".into() }).unwrap();
        let city = w.civ(me).unwrap().cities[0];
        let centre = w.cities.get(city).unwrap().centre;
        let worker = civ_core::world::construction::prepare_unit_near(
            &w,
            &r,
            me,
            r.units.id("Worker").unwrap(),
            centre,
            Some(city),
        )
        .unwrap();
        let worker = w.place_unit(worker);
        w.units.get_mut(worker).unwrap().automation = Some(Automation::Work);
        let explorer = w
            .units
            .iter()
            .find(|(_, u)| u.owner == me && u.is_military(&r))
            .unwrap()
            .0;
        w.units.get_mut(explorer).unwrap().automation = Some(Automation::Explore);
        // Know only the ring around our own units and city, so the fog starts
        // inside the units' reach.
        w.visibility = civ_core::world::Visibility::new(w.grid.len(), w.civs.len());
        let mine: Vec<TileId> = w
            .units
            .iter()
            .filter(|(_, u)| u.owner == me)
            .map(|(_, u)| u.position)
            .chain([centre])
            .collect();
        for t in mine {
            for n in w.grid.within(t, 1) {
                w.visibility.observe(me, n);
            }
        }
        capture_observations(&mut w, &r);

        // Everything this civ has never explored, rewritten two ways: walled
        // off with mountains and strangers, or open desert full of half-dug
        // farms. Neither may change what our units do.
        let (desert, mountain) = (r.terrain("Desert").unwrap(), r.terrain("Mountain").unwrap());
        let farm = r.improvements.id("Farm").unwrap();
        let barbarians = w.civs.iter().find(|c| c.is_barbarian).unwrap().id;
        let brute = r.units.id("Brute").unwrap();
        let rewrite = |walled: bool| {
            let mut other = w.clone();
            for t in other.grid.ids().collect::<Vec<_>>() {
                if other.visibility.has_explored(me, t) || other.grid.is_water(&r, t) {
                    continue;
                }
                let i = t.0 as usize;
                other.grid.features[i].clear();
                other.grid.resource[i] = None;
                if walled {
                    other.grid.terrain[i] = mountain;
                    if t.0 % 5 == 0 {
                        other.place_unit(Unit::new(brute, barbarians, t, 2));
                    }
                } else {
                    other.grid.terrain[i] = desert;
                    other.grid.improvement_progress[i] = Some((farm, 1));
                }
            }
            other
        };
        for other in [rewrite(true), rewrite(false)] {
            // The same comparison catches the AI's own routines, which read the whole map.
            caught |= civ_core::world::ai::explore_order(&w, &r, me, explorer)
                != civ_core::world::ai::explore_order(&other, &r, me, explorer)
                || civ_core::world::ai::worker_order(&w, &r, me, worker, &Default::default())
                    != civ_core::world::ai::worker_order(&other, &r, me, worker, &Default::default());
            for unit in [worker, explorer] {
                let ours = order(&w, &r, unit, &Default::default());
                let theirs = order(&other, &r, unit, &Default::default());
                assert_eq!(ours, theirs, "seed {seed}: {unit:?} decided from hidden tiles");
            }
        }
    }
    assert!(caught, "the rewritten fog never changed even a whole-map routine's answer");
}

#[test]
fn scores_and_attitudes_are_published_only_for_met_civilizations() {
    let (mut w, r, _, city) = fixture();
    let before = player_view(&w, &r, ME);
    assert!(before.relations.is_empty(), "no one is met yet");
    // an unmet rival's growth stays out of the view
    w.cities.get_mut(city).unwrap().population = 12;
    assert_eq!(player_view(&w, &r, ME).relations, before.relations);
    assert_eq!(before.endgame.score, 0, "no cities and no technologies yet");
    let _ = w.diplomacy.meet(ME, OTHER);
    let met = player_view(&w, &r, ME);
    let other = met.relations.iter().find(|x| x.civ == OTHER.0).expect("a met civ is listed");
    assert!(other.score > 0);
    assert!(["unforgivable", "enemy", "competitor", "neutral", "favorable", "friend", "ally"].contains(&other.attitude.as_str()));
    assert!(met.relations.iter().all(|x| x.civ != 2), "Egypt is still unmet");
}
