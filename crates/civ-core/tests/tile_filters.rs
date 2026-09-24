//! Every tile-filter word in the shipped rules names something: a misspelled
//! building in `<within [2] tiles of a [X]>` would otherwise never be
//! decided, and an undecidable condition applies everywhere.

use civ_core::ruleset::civgame::UnknownTileFilter;
use civ_core::ruleset::Ruleset;
use civ_core::Unique;

#[test]
fn shipped_rules_name_only_things_that_exist_in_tile_filters() {
    for rules in ["base", "gods-and-kings", "vanilla"] {
        let r = Ruleset::load_dir(format!("rules/{rules}")).unwrap().unwrap();
        assert_eq!(r.unknown_tile_filters(), [], "{rules}");
    }
}

#[test]
fn a_misspelled_building_in_a_filter_is_caught() {
    let mut r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let colossus = r.buildings.id("Colossus").unwrap();
    let uniques = &mut r.buildings.get_mut(colossus).unwrap().uniques;
    uniques.push(Unique::parse("[+1] Movement <for [Water] units> <within [2] tiles of a [Colosus]>"));
    uniques.push(Unique::parse("[+1 Food] from [{Coast} {Harbour}] tiles [in this city]"));
    uniques.push(Unique::parse("[+1 Gold] from [Colossus] tiles [in this city]"));
    let unknown = r.unknown_tile_filters();
    let terms: Vec<_> = unknown.iter().map(|u| u.term.as_str()).collect();
    assert_eq!(terms, ["Colosus", "Harbour"]);
    assert_eq!(unknown[0], UnknownTileFilter { source: "Building Colossus".into(), term: "Colosus".into() });
}

mod words {
    //! Unciv's tile-filter words the shipped rules use (Tile.kt
    //! `matchesSingleTerrainFilter`), now evaluated.
    use civ_core::ruleset::Ruleset;
    use civ_core::world::conditionals::{tile_matches, tile_matches_in, GameContext, Verdict};
    use civ_core::world::entities::Civilization;
    use civ_core::world::grid::{CivId, TileGrid};
    use civ_core::world::world::World;

    fn setup() -> (Ruleset, World) {
        let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
        let civs = ["Rome", "Greece"]
            .iter()
            .enumerate()
            .map(|(i, n)| Civilization::new(CivId(i as u8), r.nations.id(n).unwrap(), false))
            .collect();
        let w = World::new(TileGrid::new(12, 8, r.terrain("Grassland").unwrap()), civs);
        (r, w)
    }

    #[test]
    fn fresh_water_in_either_spelling_and_its_opposite() {
        let (r, mut w) = setup();
        let lake = w.grid.id_at(4, 4).unwrap();
        w.grid.terrain[lake.0 as usize] = r.terrain("Lakes").unwrap();
        let shore = w.grid.id_at(5, 4).unwrap();
        let dry = w.grid.id_at(10, 1).unwrap();
        for (tile, fresh) in [(shore, true), (dry, false)] {
            let is = |f: &str| tile_matches_in(&w.grid, &r, tile, f, None) == Verdict::True;
            assert_eq!(is("Fresh water"), fresh);
            assert_eq!(is("Fresh Water"), fresh);
            assert_eq!(is("non-fresh water"), !fresh);
        }
    }

    #[test]
    fn resource_words_read_the_tiles_resource_and_its_type() {
        let (r, mut w) = setup();
        let sea = w.grid.id_at(2, 2).unwrap();
        w.grid.terrain[sea.0 as usize] = r.terrain("Coast").unwrap();
        w.grid.resource[sea.0 as usize] = r.resources.id("Fish");
        let field = w.grid.id_at(6, 6).unwrap();
        w.grid.resource[field.0 as usize] = r.resources.id("Wheat");
        let is = |t, f: &str| tile_matches_in(&w.grid, &r, t, f, None) == Verdict::True;
        assert!(is(sea, "Water resource") && !is(field, "Water resource"));
        assert!(is(field, "Bonus resource") && is(sea, "Bonus resource"));
        assert!(!is(field, "Luxury resource") && !is(w.grid.id_at(0, 0).unwrap(), "Bonus resource"));
        assert!(is(field, "resource") && !is(w.grid.id_at(0, 0).unwrap(), "resource"));
    }

    #[test]
    fn enemy_land_is_territory_of_someone_at_war_with_us() {
        let (r, mut w) = setup();
        let theirs = w.grid.id_at(8, 4).unwrap();
        w.grid.owner[theirs.0 as usize] = Some(CivId(1));
        w.diplomacy.meet(CivId(0), CivId(1));
        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0));
        assert_eq!(tile_matches(&ctx, theirs, "Enemy Land"), Verdict::False, "at peace");
        w.diplomacy.declare_war(CivId(0), CivId(1));
        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0));
        assert_eq!(tile_matches(&ctx, theirs, "Enemy Land"), Verdict::True);
        assert_eq!(tile_matches(&ctx, w.grid.id_at(1, 1).unwrap(), "Enemy Land"), Verdict::False, "nobody's");
    }
}
