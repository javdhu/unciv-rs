//! What the major civs think of each other, and why.
//!
//! Every directed relation carries named [`Sentiment`]s whose sum is the
//! opinion, and the opinion falls into a [`RelationshipLevel`] — Unforgivable
//! at −80, Ally at 80 (`DiplomacyManager.relationshipIgnoreAfraid`). Events
//! write sentiments: a declaration of war sets −20 on its victim's view of the
//! aggressor and −5 of warmongering on every bystander that liked the victim
//! (`DeclareWar.changeOpinions`); taking a city costs ten plus the share of the
//! loser's people it held (`diplomaticRepercussionsForConqueringCity`).
//!
//! Each turn every sentiment drifts back toward nothing at its own rate, and
//! peace itself builds a little warmth, up to thirty (`nextTurnDiplomaticModifiers`).
//! The rates are Unciv's, scaled by `0.67 / speed` so a long game forgets as
//! slowly, in turns, as the game is long.

use crate::stats::Fixed;
use crate::world::diplomacy::{Attitude, Demand, Sentiment};
use crate::world::grid::CivId;
use crate::world::world::World;

/// How one civ regards another, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelationshipLevel {
    Unforgivable,
    Enemy,
    Competitor,
    Neutral,
    Favorable,
    Friend,
    Ally,
}

/// `SPEED_ADJUSTMENT_NORMALIZATION_FACTOR`: rates are written for Quick speed.
const SPEED_NORMALIZATION: Fixed = Fixed::permille(670);

/// Warmth peace builds a turn, and the most it builds.
const PEACE_PER_TURN: Fixed = Fixed::permille(500);
const PEACE_CAP: Fixed = Fixed::whole(30);

/// Set on the victim's view of an aggressor.
const DECLARED_WAR: Fixed = Fixed::whole(-20);
/// Added to every bystander's view of an aggressor.
const WARMONGERING: Fixed = Fixed::whole(-5);
/// Added to the view of anyone fighting your enemy.
const SHARED_ENEMY: Fixed = Fixed::whole(5);

/// How fast each sentiment fades, a turn at Quick speed.
fn fade_rate(s: Sentiment) -> Option<Fixed> {
    Some(match s {
        Sentiment::DeclaredWarOnUs | Sentiment::BetrayedFriendship => Fixed::permille(125),
        Sentiment::WarMongerer => Fixed::permille(500),
        Sentiment::BetrayedDefensivePact => Fixed::from_fraction(1, 16),
        Sentiment::CapturedOurCities => Fixed::permille(250),
        Sentiment::LiberatedCity => Fixed::permille(125),
        Sentiment::GaveUsUnits => Fixed::permille(250),
        // Personal, so slow to fade.
        Sentiment::Denunciation => Fixed::permille(125),
        Sentiment::DenouncedOurAllies | Sentiment::DenouncedOurEnemies => Fixed::permille(250),
        // As fast as warmongering.
        Sentiment::BulliedProtectedMinor => Fixed::permille(500),
        Sentiment::SpiedOnUs | Sentiment::UnacceptableDemands => Fixed::permille(250),
        Sentiment::RefusedDemand(Demand::DoNotSettleNearUs) => Fixed::permille(250),
        Sentiment::RefusedDemand(Demand::DoNotAttackUs) => Fixed::permille(500),
        // Betrayals are remembered for a long time.
        Sentiment::BetrayedPromise(_) | Sentiment::BetrayedPromiseToOthers => Fixed::permille(125),
        // Handled with peace and open borders below. Shared enemies, kept
        // promises and a refusal to stop spying or preaching are not
        // forgotten at all in Unciv.
        Sentiment::Friendship
        | Sentiment::DefensivePact
        | Sentiment::DefensivePactWithAllies
        | Sentiment::DefensivePactWithEnemies
        | Sentiment::Embassies
        | Sentiment::YearsOfPeace
        | Sentiment::OpenBorders
        | Sentiment::UsedNuclearWeapons
        | Sentiment::SharedEnemy
        | Sentiment::FulfilledPromise(_)
        | Sentiment::RefusedDemand(Demand::DontSpyOnUs | Demand::DoNotSpreadReligion) => return None,
    })
}

fn is_major(world: &World, civ: CivId) -> bool {
    world.civ(civ).is_some_and(|c| c.is_major())
}

/// What `holder` thinks of `subject`, as a sum.
pub fn opinion(world: &World, holder: CivId, subject: CivId) -> Fixed {
    world.diplomacy.attitude(holder, subject).opinion()
}

/// How `holder` regards `subject` (`relationshipIgnoreAfraid`).
///
/// A human holds no opinion the engine can know, so a human's regard is
/// taken to be the other side's, and two humans are simply Neutral. War
/// makes anyone at least an Enemy. A city-state's regard is its influence.
pub fn relationship(world: &World, holder: CivId, subject: CivId) -> RelationshipLevel {
    let human = |c: CivId| world.civ(c).is_some_and(|x| x.is_major() && !x.is_ai);
    if human(holder) && human(subject) {
        return RelationshipLevel::Neutral;
    }
    if human(holder) {
        return relationship(world, subject, holder);
    }
    if world.civ(holder).is_some_and(|c| c.is_city_state()) {
        use crate::world::city_states::Relationship as R;
        return match crate::world::city_states::relationship(world, holder, subject) {
            R::Unforgivable => RelationshipLevel::Unforgivable,
            R::Enemy => RelationshipLevel::Enemy,
            R::Neutral => RelationshipLevel::Neutral,
            R::Friend => RelationshipLevel::Friend,
            R::Ally => RelationshipLevel::Ally,
        };
    }
    let o = opinion(world, holder, subject);
    let at_war = world.diplomacy.at_war(holder, subject);
    match o {
        o if o <= Fixed::whole(-80) => RelationshipLevel::Unforgivable,
        o if o <= Fixed::whole(-40) || at_war => RelationshipLevel::Enemy,
        o if o <= Fixed::whole(-15) => RelationshipLevel::Competitor,
        o if o >= Fixed::whole(80) => RelationshipLevel::Ally,
        o if o >= Fixed::whole(40) => RelationshipLevel::Friend,
        o if o >= Fixed::whole(15) => RelationshipLevel::Favorable,
        _ => RelationshipLevel::Neutral,
    }
}

/// Majors `civ` has met, in id order.
fn known_majors(world: &World, civ: CivId) -> Vec<CivId> {
    world.civ_ids().filter(|o| *o != civ && is_major(world, *o) && world.diplomacy.have_met(civ, *o)).collect()
}

/// A war was declared (`DeclareWar.changeOpinions`).
pub fn on_war_declared(world: &mut World, aggressor: CivId, victim: CivId) {
    crate::world::promises::on_war_declared(world, aggressor, victim);
    world.diplomacy.attitude_mut(victim, aggressor).set(Sentiment::DeclaredWarOnUs, DECLARED_WAR);

    // Bystanders who thought well enough of the victim hold it against the aggressor.
    for third in known_majors(world, aggressor) {
        if third == victim || world.diplomacy.at_war(third, victim) {
            continue;
        }
        if relationship(world, third, victim) > RelationshipLevel::Competitor {
            world.diplomacy.attitude_mut(third, aggressor).add(Sentiment::WarMongerer, WARMONGERING, None);
        }
    }

    // And anyone already fighting one side warms to the other.
    let common: Vec<CivId> = world
        .civ_ids()
        .filter(|t| *t != aggressor && *t != victim)
        .filter(|t| world.diplomacy.have_met(aggressor, *t) && world.diplomacy.have_met(victim, *t))
        .collect();
    for third in common {
        let (toward, city_state) = if world.diplomacy.at_war(third, victim) && !world.diplomacy.at_war(third, aggressor)
        {
            (aggressor, world.civ(third).is_some_and(|c| c.is_city_state()))
        } else if world.diplomacy.at_war(third, aggressor) {
            (victim, world.civ(third).is_some_and(|c| c.is_city_state()))
        } else {
            continue;
        };
        if city_state {
            world.diplomacy.add_influence(third, toward, Fixed::whole(10));
        } else {
            world.diplomacy.attitude_mut(third, toward).add(Sentiment::SharedEnemy, SHARED_ENEMY, None);
        }
    }
}

/// A city changed hands by force (`diplomaticRepercussionsForConqueringCity`).
///
/// `population` is the city's before the conquest took its toll, and
/// `loser_population` the loser's whole population including it.
pub fn on_city_captured(world: &mut World, conqueror: CivId, loser: CivId, population: u32, loser_population: u32) {
    if !is_major(world, loser) && !world.civ(loser).is_some_and(|c| c.is_city_state()) {
        return;
    }
    let share = if loser_population == 0 {
        0
    } else {
        (population as i64 * 100 * 2 + loser_population as i64) / (2 * loser_population as i64)
    };
    let aggro = 10 + share;
    world.diplomacy.attitude_mut(loser, conqueror).add(
        Sentiment::CapturedOurCities,
        Fixed::whole(-(aggro as i32)),
        None,
    );

    // `(aggro / 10).roundToInt()`
    let echo = Fixed::whole(((aggro * 2 + 10) / 20) as i32);
    for third in known_majors(world, conqueror) {
        if third == loser {
            continue;
        }
        if world.diplomacy.at_war(third, loser) {
            world.diplomacy.attitude_mut(third, conqueror).add(Sentiment::SharedEnemy, echo, None);
        } else {
            world.diplomacy.attitude_mut(third, conqueror).add(Sentiment::WarMongerer, -echo, None);
        }
    }
}

/// How long a denunciation stands (`DiplomacyFlags.Denunciation`), in turns,
/// whatever the speed — Unciv does not scale it either.
pub const DENUNCIATION_TURNS: u32 = 30;

/// The sting of the first denunciation; each later one stings less.
const DENOUNCED: Fixed = Fixed::whole(-35);

/// `from` publicly denounces `to` (`DiplomacyManager.activateDenounceEffects`).
///
/// The target's opinion of the accuser drops by 35 — less if others already
/// denounce it: −35, −25, −18, −12, halving every two denouncers. Everyone who
/// knows both then judges the accuser by what they think of the target.
pub fn denounce(world: &mut World, from: CivId, to: CivId) {
    use crate::world::diplomacy::Flag;
    let already =
        known_majors(world, to).into_iter().filter(|c| world.diplomacy.has_flag(*c, to, Flag::Denunciation)).count();
    let sting = DENOUNCED * Fixed::permille(-500 * already as i64).exp2();
    world.diplomacy.attitude_mut(to, from).set(Sentiment::Denunciation, sting);
    world.diplomacy.set_flag(from, to, Flag::Denunciation, DENUNCIATION_TURNS);

    for third in known_majors(world, from) {
        if third == to || !world.diplomacy.have_met(third, to) {
            continue;
        }
        let (s, amount) = match relationship(world, third, to) {
            RelationshipLevel::Unforgivable => (Sentiment::DenouncedOurEnemies, 15),
            RelationshipLevel::Enemy => (Sentiment::DenouncedOurEnemies, 5),
            RelationshipLevel::Friend => (Sentiment::DenouncedOurAllies, -5),
            RelationshipLevel::Ally => (Sentiment::DenouncedOurAllies, -15),
            _ => continue,
        };
        world.diplomacy.attitude_mut(third, from).add(s, Fixed::whole(amount), None);
    }
}

/// How far `holder`'s opinion of `subject` has fallen below its recent
/// average (`smoothedOpinionDelta`). Negative is a sudden souring.
pub fn smoothed_delta(world: &World, holder: CivId, subject: CivId) -> Fixed {
    let a = world.diplomacy.attitude(holder, subject);
    a.opinion() - a.smoothed
}

/// Turns the opinion average spans at standard speed (`EMA_PERIOD`).
const SMOOTHING_PERIOD: i32 = 30;

/// One turn of feelings settling (`nextTurnDiplomaticModifiers`).
pub fn tick(world: &mut World, speed: Fixed) {
    crate::world::defensive_pacts::refresh_opinions(world);
    // `0.67 × rate / speed` as one ratio, so thousandths are not lost twice.
    let scale = |rate: Fixed| {
        if speed.raw() <= 0 {
            return rate;
        }
        Fixed::permille((rate.raw() as i128 * SPEED_NORMALIZATION.raw() as i128 / speed.raw() as i128) as i64)
    };
    let majors: Vec<CivId> = world.civ_ids().filter(|c| is_major(world, *c)).collect();
    for &holder in &majors {
        for &subject in &majors {
            if holder == subject || !world.diplomacy.have_met(holder, subject) {
                continue;
            }
            let at_war = world.diplomacy.at_war(holder, subject);
            let open = world.diplomacy.may_enter_borders_of(holder, subject) as i32
                + world.diplomacy.may_enter_borders_of(subject, holder) as i32;
            let friendship = world.diplomacy.pact(holder, subject).is_some_and(|p| p.friendship_turns > 0);
            let defensive = world.diplomacy.pact(holder, subject).is_some_and(|p| p.defensive_pact);
            let a: &mut Attitude = world.diplomacy.attitude_mut(holder, subject);
            if !defensive {
                a.fade(Sentiment::DefensivePact, scale(Fixed::ONE));
            }
            if !friendship {
                a.fade(Sentiment::Friendship, scale(Fixed::permille(500)));
            }
            if at_war {
                a.fade(Sentiment::YearsOfPeace, scale(PEACE_PER_TURN));
            } else {
                a.grow(Sentiment::YearsOfPeace, scale(PEACE_PER_TURN), Some(PEACE_CAP));
            }
            if open > 0 {
                a.grow(Sentiment::OpenBorders, scale(Fixed::permille(125 * open as i64)), None);
            } else {
                a.fade(Sentiment::OpenBorders, scale(Fixed::permille(125)));
            }
            let held: Vec<Sentiment> = a.sentiments.keys().copied().collect();
            for s in held {
                if let Some(rate) = fade_rate(s) {
                    a.fade(s, scale(rate));
                }
            }
            // The average moves a period's share toward today's opinion, one
            // turn behind (`saveSmoothedOpinionOfOtherCiv`).
            let period = Fixed::whole(SMOOTHING_PERIOD) * speed;
            a.smoothed = a.smoothed_next;
            if let Some(next) = (a.opinion() + a.smoothed * (period - Fixed::ONE)).checked_div(period) {
                a.smoothed_next = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> crate::ruleset::Ruleset {
        crate::ruleset::Ruleset::load_dir("rules/gods-and-kings")
            .expect("readable")
            .expect("loads")
    }

    /// Three AI majors who have all met.
    fn world(r: &crate::ruleset::Ruleset) -> World {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, (0..3).map(|i| Civilization::new(CivId(i), nation, true)).collect());
        for (a, b) in [(0, 1), (0, 2), (1, 2)] {
            w.diplomacy.meet(CivId(a), CivId(b));
        }
        w
    }

    #[test]
    fn the_thresholds_are_unciv_s() {
        let r = ruleset();
        let mut w = world(&r);
        let cases = [
            (-80, RelationshipLevel::Unforgivable),
            (-40, RelationshipLevel::Enemy),
            (-15, RelationshipLevel::Competitor),
            (0, RelationshipLevel::Neutral),
            (15, RelationshipLevel::Favorable),
            (40, RelationshipLevel::Friend),
            (80, RelationshipLevel::Ally),
        ];
        for (o, level) in cases {
            w.diplomacy.attitude_mut(CivId(0), CivId(1)).set(Sentiment::YearsOfPeace, Fixed::whole(o));
            assert_eq!(relationship(&w, CivId(0), CivId(1)), level, "opinion {o}");
        }
        w.diplomacy.declare_war(CivId(0), CivId(1));
        assert_eq!(relationship(&w, CivId(0), CivId(1)), RelationshipLevel::Enemy, "war makes an enemy of a friend");
    }

    #[test]
    fn a_declaration_of_war_is_remembered_by_the_victim_and_the_neighbours() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(0), CivId(1));
        on_war_declared(&mut w, CivId(0), CivId(1));
        assert_eq!(opinion(&w, CivId(1), CivId(0)), Fixed::whole(-20));
        assert_eq!(opinion(&w, CivId(2), CivId(0)), Fixed::whole(-5), "a neutral bystander calls it warmongering");
        assert_eq!(opinion(&w, CivId(0), CivId(1)), Fixed::ZERO, "the aggressor thinks no worse of anyone");
    }

    #[test]
    fn a_war_fades_at_an_eighth_of_a_point_a_turn_at_quick_speed() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.attitude_mut(CivId(1), CivId(0)).set(Sentiment::DeclaredWarOnUs, Fixed::whole(-20));
        // At a speed modifier of 0.67 (Quick) the rate is exactly the written one.
        tick(&mut w, Fixed::permille(670));
        assert_eq!(w.diplomacy.attitude(CivId(1), CivId(0)).get(Sentiment::DeclaredWarOnUs), Fixed::permille(-19_875));
    }

    #[test]
    fn peace_builds_warmth_up_to_thirty() {
        let r = ruleset();
        let mut w = world(&r);
        for _ in 0..200 {
            tick(&mut w, Fixed::permille(670));
        }
        let a = w.diplomacy.attitude(CivId(0), CivId(1));
        assert_eq!(a.get(Sentiment::YearsOfPeace), Fixed::whole(30));
        assert_eq!(relationship(&w, CivId(0), CivId(1)), RelationshipLevel::Favorable);
    }

    #[test]
    fn a_denunciation_stings_less_the_more_there_are() {
        let r = ruleset();
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, (0..4).map(|i| Civilization::new(CivId(i), nation, true)).collect());
        for a in 0..4 {
            for b in a + 1..4 {
                w.diplomacy.meet(CivId(a), CivId(b));
            }
        }
        let sting = |w: &World, by: u8| w.diplomacy.attitude(CivId(0), CivId(by)).get(Sentiment::Denunciation);
        denounce(&mut w, CivId(1), CivId(0));
        denounce(&mut w, CivId(2), CivId(0));
        denounce(&mut w, CivId(3), CivId(0));
        // -35, -35·2^-0.5, -35·2^-1.
        assert_eq!(sting(&w, 1), Fixed::whole(-35));
        assert!((Fixed::permille(-24_800)..=Fixed::permille(-24_700)).contains(&sting(&w, 2)), "{}", sting(&w, 2));
        assert_eq!(sting(&w, 3), Fixed::permille(-17_500));
        assert!(w.diplomacy.has_flag(CivId(1), CivId(0), crate::world::diplomacy::Flag::Denunciation));
        assert_eq!(w.diplomacy.flag_turns(CivId(1), CivId(0), crate::world::diplomacy::Flag::Denunciation), 30);
    }

    #[test]
    fn denouncing_a_friends_enemy_wins_the_friend() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.attitude_mut(CivId(2), CivId(1)).set(Sentiment::CapturedOurCities, Fixed::whole(-90));
        denounce(&mut w, CivId(0), CivId(1));
        assert_eq!(w.diplomacy.attitude(CivId(2), CivId(0)).get(Sentiment::DenouncedOurEnemies), Fixed::whole(15));

        let mut w = world(&r);
        w.diplomacy.attitude_mut(CivId(2), CivId(1)).set(Sentiment::YearsOfPeace, Fixed::whole(85));
        denounce(&mut w, CivId(0), CivId(1));
        assert_eq!(w.diplomacy.attitude(CivId(2), CivId(0)).get(Sentiment::DenouncedOurAllies), Fixed::whole(-15));
    }

    #[test]
    fn the_average_trails_the_opinion_by_a_turn() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(1), CivId(2)); // keep peace from moving 0→1
        w.diplomacy.attitude_mut(CivId(0), CivId(1)).set(Sentiment::SharedEnemy, Fixed::whole(30));
        tick(&mut w, Fixed::ONE);
        let a = w.diplomacy.attitude(CivId(0), CivId(1));
        assert_eq!(a.smoothed, Fixed::ZERO, "the first turn only computes");
        tick(&mut w, Fixed::ONE);
        let a = w.diplomacy.attitude(CivId(0), CivId(1));
        assert!(a.smoothed > Fixed::ZERO && a.smoothed < Fixed::whole(3), "{}", a.smoothed);
        assert!(smoothed_delta(&w, CivId(0), CivId(1)) > Fixed::whole(25));
    }

    #[test]
    fn a_sudden_souring_is_denounced_and_a_slow_one_is_not() {
        let r = ruleset();
        let mut w = world(&r);
        // Opinion has crashed from nothing to -70 in a turn: well past -65·1.005^-70 ≈ -46.
        w.diplomacy.attitude_mut(CivId(0), CivId(1)).set(Sentiment::CapturedOurCities, Fixed::whole(-70));
        assert_eq!(crate::world::ai::choose_denunciations(&w, &r, CivId(0)), vec![CivId(1)]);

        // The same opinion, long held, is no news.
        w.diplomacy.attitude_mut(CivId(0), CivId(1)).smoothed = Fixed::whole(-70);
        assert!(crate::world::ai::choose_denunciations(&w, &r, CivId(0)).is_empty());
    }

    #[test]
    fn taking_a_city_costs_ten_and_its_share_of_the_losers_people() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(0), CivId(1));
        // A city of four out of ten: 10 + 40.
        on_city_captured(&mut w, CivId(0), CivId(1), 4, 10);
        assert_eq!(w.diplomacy.attitude(CivId(1), CivId(0)).get(Sentiment::CapturedOurCities), Fixed::whole(-50));
        assert_eq!(w.diplomacy.attitude(CivId(2), CivId(0)).get(Sentiment::WarMongerer), Fixed::whole(-5));
    }
}
