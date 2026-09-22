// Standalone test to verify if barbarians spawn during advance_turn
// This is a proof-of-concept test file

fn main() {
    // Import would go here, but we'll use the test runner instead
    println!("This file would be added to tests/ or src/bin/ to run");
}

#[cfg(test)]
mod tests {
    use civ_core::ruleset::Ruleset;
    use civ_core::world::{CivId, TileGrid, TileId, Civilization};
    use civ_core::world::turn::{advance_turn, CityFocus};
    use civ_core::world::city::GrowthModifiers;
    use civ_core::world::construction::CostModifiers;
    use civ_core::world::research::ScienceModifiers;
    use civ_core::world::policies::PolicyModifiers;

    #[test]
    fn barbarians_spawn_during_turn_advancement() {
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads");

        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut civs = vec![
            Civilization::new(CivId(0), nation.clone(), true),
            Civilization::new(CivId(1), nation, true),
        ];
        civs[1].is_barbarian = true;

        let mut w = civ_core::world::World::new(g, civs);
        w.diplomacy.mark_hostile(CivId(1));

        // Place a camp on the map
        civ_core::world::barbarians::place_camp(&mut w, &r, TileId(20));

        let initial_units = w.units.len();
        println!("Initial unit count: {}", initial_units);

        // Advance turn multiple times to allow barbarians to spawn
        for turn_num in 0..20 {
            w.turn = turn_num;

            let focus = CityFocus::default();
            let modifiers = GrowthModifiers::default();
            let costs = CostModifiers::default();
            let science = ScienceModifiers::default();
            let policies = PolicyModifiers::default();

            let _events = advance_turn(&mut w, &r, focus, modifiers, costs, science, None, policies);

            let current_units = w.units.len();
            println!("Turn {}: {} units", turn_num + 1, current_units);

            if current_units > initial_units {
                println!("SUCCESS: Barbarians spawned during turn advancement!");
                return;
            }
        }

        panic!("FAILED: No barbarians spawned after 20 turns with a camp on the map. This demonstrates the missing barbarians::spawn() call in advance_turn().");
    }
}
