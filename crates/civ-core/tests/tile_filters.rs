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
