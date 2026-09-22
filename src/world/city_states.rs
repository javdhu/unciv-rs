//! City-states: the civs nobody plays, that everybody courts.
//!
//! A city-state is an ordinary [`Civilization`] with a
//! [`city_state_type`](crate::world::entities::Civilization::city_state_type)
//! set. It never expands, never wins, and exists to be a *source* — of
//! culture, food, happiness or units, depending on its type — that the major
//! civs compete over. Modelling it as a civ rather than as a kind of city is
//! what lets it hold territory, fight, be conquered, and appear in diplomacy
//! without any of those systems needing a special case.
//!
//! # Influence is directed, and the ally is derived
//!
//! Influence is not a relationship between two civs; it is a city-state's
//! opinion of one major civ, and there is no reciprocal value. So it is stored
//! directed, keyed `(city_state, major)`, next to the attitudes rather than
//! among the symmetric pacts.
//!
//! Who a city-state's *ally* is, on the other hand, is not stored at all. It
//! is whoever holds the most influence above the threshold. Unciv keeps an
//! `allyCiv` field, and a stored answer to a derived question is a thing that
//! can be wrong: it has to be updated when influence rises, when it decays,
//! when a rival overtakes you, when the ally is destroyed, and when the ally
//! declares war. Deriving it cannot fall out of step with the numbers it comes
//! from.

use crate::arena::Id;
use crate::ruleset::{CityStateType, Ruleset};
use crate::stats::{Fixed, Precise};
use crate::unique::Unique;
use crate::world::grid::CivId;
use crate::world::world::World;

/// Influence at or above which a major civ is the city-state's ally — if
/// nobody else has more.
pub const ALLY_THRESHOLD: Fixed = Fixed::permille(60_000);
/// Influence at or above which it counts you a friend.
pub const FRIEND_THRESHOLD: Fixed = Fixed::permille(30_000);
/// Below this it will not deal with you at all.
pub const UNFORGIVABLE_THRESHOLD: Fixed = Fixed::permille(-30_000);
/// Influence a protector adds to where the relationship settles.
pub const PROTECTOR_RESTING_BONUS: Fixed = Fixed::permille(10_000);

/// How a city-state sees one major civ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Relationship {
    Unforgivable,
    Enemy,
    Neutral,
    Friend,
    Ally,
}

/// What a city-state thinks of a major civ.
///
/// `Ally` needs more than influence: only one civ can be the ally, so this
/// asks who is actually in front. A second civ at 70 influence is a Friend,
/// not a co-ally.
pub fn relationship(world: &World, cs: CivId, major: CivId) -> Relationship {
    if world.diplomacy.at_war(cs, major) {
        return Relationship::Unforgivable;
    }
    let influence = world.diplomacy.influence(cs, major);
    if influence <= UNFORGIVABLE_THRESHOLD {
        Relationship::Unforgivable
    } else if influence < Fixed::ZERO {
        Relationship::Enemy
    } else if influence >= ALLY_THRESHOLD && ally_of(world, cs) == Some(major) {
        Relationship::Ally
    } else if influence >= FRIEND_THRESHOLD {
        Relationship::Friend
    } else {
        Relationship::Neutral
    }
}

/// Whoever holds the most influence over this city-state, if it is enough.
///
/// Ties break on the lower civ id — not because that is fair, but because two
/// civs at exactly 60 has to resolve the same way on every machine.
pub fn ally_of(world: &World, cs: CivId) -> Option<CivId> {
    world
        .diplomacy
        .influencers(cs)
        .into_iter()
        .filter(|(major, _)| !world.diplomacy.at_war(cs, *major))
        .filter(|(major, _)| world.civ(*major).is_some_and(|c| c.is_major() && c.is_alive()))
        .filter(|(_, v)| *v >= ALLY_THRESHOLD)
        .max_by(|a, b| a.1.cmp(&b.1).then(std::cmp::Reverse(a.0 .0).cmp(&std::cmp::Reverse(b.0 .0))))
        .map(|(major, _)| major)
}

/// Every city-state still standing, in id order.
pub fn city_states(world: &World) -> Vec<CivId> {
    world
        .civs
        .iter()
        .filter(|c| c.is_city_state() && c.is_alive())
        .map(|c| c.id)
        .collect()
}

/// Where this relationship settles if left alone.
///
/// Influence always slides back here — down from a gift, up from a war — which
/// is what makes courting a city-state an ongoing cost rather than a purchase.
pub fn resting_point(world: &World, cs: CivId, major: CivId) -> Fixed {
    let mut point = Fixed::ZERO;
    if let Some(c) = world.civ(major) {
        for u in c.uniques.matching(crate::UniqueType::CityStateRestingPoint) {
            if let Some(v) = u.params.first().and_then(|p| Fixed::parse(p)) {
                point = point + v;
            }
        }
    }
    if world.diplomacy.is_protector(major, cs) {
        point = point + PROTECTOR_RESTING_BONUS;
    }
    point
}

/// One turn of drift toward the resting point.
///
/// A point a turn, modified — slow enough that an alliance bought at 60 lasts
/// thirty turns, which is the whole design of the mechanic: influence is rent,
/// not a purchase.
pub fn drift(world: &World, cs: CivId, major: CivId) -> Fixed {
    let resting = resting_point(world, cs, major);
    let influence = world.diplomacy.influence(cs, major);
    if influence == resting {
        return Fixed::ZERO;
    }
    let step = Fixed::ONE;
    if influence > resting {
        let mut percent = Fixed::ZERO;
        if let Some(c) = world.civ(major) {
            for u in c.uniques.matching(crate::UniqueType::CityStateInfluenceDegradation) {
                if let Some(v) = u.params.first().and_then(|p| Fixed::parse(p)) {
                    percent = percent + v;
                }
            }
        }
        let scaled = step
            .mul(Fixed::whole(100) + percent)
            .div(Fixed::whole(100))
            .unwrap_or(step)
            .max(Fixed::ZERO);
        -(scaled.min(influence - resting))
    } else {
        let mut percent = Fixed::ZERO;
        if world
            .civ(major)
            .is_some_and(|c| c.uniques.has(crate::UniqueType::CityStateInfluenceRecoversTwiceNormalRate))
        {
            percent = percent + Fixed::whole(100);
        }
        let scaled = step
            .mul(Fixed::whole(100) + percent)
            .div(Fixed::whole(100))
            .unwrap_or(step)
            .max(Fixed::ZERO);
        scaled.min(resting - influence)
    }
}

/// A relationship that changed rank this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelationshipChange {
    pub city_state: CivId,
    pub major: CivId,
    pub from: Relationship,
    pub to: Relationship,
}

/// Slide every relationship one step toward where it belongs.
///
/// Planned in full before anything is written, because an ally is decided by
/// comparing influences — applying one civ's decay before reading another's
/// would let the order of iteration decide who the ally is.
pub fn tick(world: &mut World) -> Vec<RelationshipChange> {
    let states = city_states(world);
    let majors: Vec<CivId> = world.civs.iter().filter(|c| c.is_major()).map(|c| c.id).collect();

    let mut before = Vec::new();
    let mut steps = Vec::new();
    for &cs in &states {
        for &major in &majors {
            before.push((cs, major, relationship(world, cs, major)));
            let step = drift(world, cs, major);
            if !step.is_zero() {
                steps.push((cs, major, step));
            }
        }
    }
    for (cs, major, step) in steps {
        world.diplomacy.add_influence(cs, major, step);
    }

    before
        .into_iter()
        .filter_map(|(cs, major, from)| {
            let to = relationship(world, cs, major);
            (to != from).then_some(RelationshipChange { city_state: cs, major, from, to })
        })
        .collect()
}

/// Influence bought by a gift of gold.
///
/// `amount^1.01 / 9.8`, falling to a third of that by the end of the game, then
/// rounded *down* to a multiple of five with a floor of five. The exponent is
/// barely above one — it exists to make very large gifts slightly better than
/// proportional, and it is the reason this needs a real fractional power
/// rather than a multiplication.
pub fn influence_from_gift(
    world: &World,
    gold: i32,
    game_length: i32,
    gold_gift_modifier: Fixed,
    donor: CivId,
) -> Fixed {
    if gold <= 0 {
        return Fixed::ZERO;
    }
    let base = Fixed::whole(gold)
        .pow(Fixed::permille(1_010))
        .unwrap_or(Fixed::whole(gold))
        .div(Fixed::permille(9_800))
        .unwrap_or(Fixed::ZERO);

    // Gifts are worth less as the game goes on: a third as much by the end.
    let progress = if game_length > 0 {
        Fixed::whole(world.turn as i32)
            .div(Fixed::whole(game_length))
            .unwrap_or(Fixed::ONE)
            .min(Fixed::ONE)
    } else {
        Fixed::ONE
    };
    let decay = Fixed::ONE - Fixed::permille(667).mul(progress);

    let mut gained = Precise::from_fixed(base)
        .mul(Precise::from_fixed(decay))
        .mul(Precise::from_fixed(if gold_gift_modifier.is_zero() {
            Fixed::ONE
        } else {
            gold_gift_modifier
        }))
        .to_fixed();

    if let Some(c) = world.civ(donor) {
        for u in c.uniques.matching(crate::UniqueType::CityStateGoldGiftsProvideMoreInfluence) {
            if let Some(pct) = u.params.first().and_then(|p| Fixed::parse(p)) {
                gained = gained.mul(Fixed::whole(100) + pct).div(Fixed::whole(100)).unwrap_or(gained);
            }
        }
    }

    // Down to a multiple of five, never below five. Civ does this so the
    // number on the button is a round one.
    let fives = (gained.floor() / 5).max(1);
    Fixed::whole((fives * 5) as i32)
}

/// Hand over gold and gain the influence it buys.
///
/// Returns what was gained, or `None` if the gift could not be made — you
/// cannot gift what you do not have, and you cannot buy the favour of someone
/// you are at war with.
pub fn gift_gold(
    world: &mut World,
    major: CivId,
    cs: CivId,
    gold: i32,
    game_length: i32,
    gold_gift_modifier: Fixed,
) -> Option<Fixed> {
    if gold <= 0 || world.diplomacy.at_war(cs, major) {
        return None;
    }
    if !world.civ(cs).is_some_and(|c| c.is_city_state() && c.is_alive()) {
        return None;
    }
    if world.civ(major).map_or(0, |c| c.gold) < gold {
        return None;
    }
    let gained = influence_from_gift(world, gold, game_length, gold_gift_modifier, major);
    if let Some(c) = world.civ_mut(major) {
        c.gold -= gold;
    }
    if let Some(c) = world.civ_mut(cs) {
        c.gold += gold;
    }
    world.diplomacy.add_influence(cs, major, gained);
    Some(gained)
}

/// The uniques a major civ is currently owed by the city-states it has
/// befriended.
///
/// A friend grants the friend list; an ally grants the ally list *instead*,
/// not as well — the ally uniques already restate the friend ones at a higher
/// value, and adding both would pay twice.
pub fn bonus_uniques(world: &World, r: &Ruleset, major: CivId) -> Vec<Unique> {
    let mut out = Vec::new();
    for cs in city_states(world) {
        let level = relationship(world, cs, major);
        let Some(kind) = world.civ(cs).and_then(|c| c.city_state_type) else { continue };
        let Some(kind) = r.city_state_types.get(kind) else { continue };
        let source = match level {
            Relationship::Ally => &kind.ally_bonus_uniques,
            Relationship::Friend => &kind.friend_bonus_uniques,
            _ => continue,
        };
        out.extend(source.iter().map(|text| Unique::parse(text)));
    }
    out
}

/// Flat yields a civ is owed each turn from things that are not tiles.
///
/// City-state friendship, and anything else that grants `[+3 Culture]` or
/// `[+2 Food] [in capital]` at the civ level. Nothing read these before: the
/// uniques reached the civ's index and sat there, which meant every Cultured
/// city-state in the game paid out nothing at all.
///
/// `[stats]` lands once on the empire. `[stats] [cityFilter]` lands once per
/// city that matches — "in all cities" really does mean each of them.
pub fn civ_wide_yields(world: &World, r: &Ruleset, civ: CivId) -> crate::stats::Stats {
    use crate::stats::Stats;
    let Some(c) = world.civ(civ) else { return Stats::ZERO };
    let mut total = Stats::ZERO;

    let ctx = crate::world::conditionals::GameContext::new(r).with_world(world).for_civ(civ);
    let holds = |u: &Unique| {
        crate::world::conditionals::applies(u, &ctx)
            == crate::world::conditionals::Verdict::True
    };

    for u in c.uniques.matching(crate::UniqueType::Stats).filter(|u| holds(u)) {
        if let Some(s) = u.params.first().and_then(|p| Stats::parse(p)) {
            total = total + s;
        }
    }

    let capital = crate::world::connections::capital_of(world, r, civ);
    for u in c.uniques.matching(crate::UniqueType::StatsPerCity).filter(|u| holds(u)) {
        let (Some(stats), Some(filter)) =
            (u.params.first().and_then(|p| Stats::parse(p)), u.params.get(1))
        else {
            continue;
        };
        for city_id in &c.cities {
            let Some(city) = world.cities.get(*city_id) else { continue };
            // "in capital" is the one filter that is about which city this is
            // rather than whose it is, so it is answered here.
            let matched = if filter.contains("capital") {
                Some(*city_id) == capital
            } else {
                crate::world::filters::city_matches(world, r, city, Some(city), filter)
                    == crate::world::conditionals::Verdict::True
            };
            if matched {
                total = total + stats;
            }
        }
    }
    total
}

/// A city-state's type, by name, for setting one up.
pub fn type_named(r: &Ruleset, name: &str) -> Option<Id<CityStateType>> {
    r.city_state_types.id(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    /// Two major civs and one city-state.
    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut civs = vec![
            Civilization::new(CivId(0), nation, true),
            Civilization::new(CivId(1), nation, true),
            Civilization::new(CivId(2), nation, true),
        ];
        civs[2].city_state_type =
            Some(type_named(r, "Cultured").or_else(|| r.city_state_types.ids().next())
                .expect("the ruleset has city-state types"));
        let mut w = World::new(g, civs);
        // A city-state is its one city; without it the civ counts as dead.
        for (i, (col, row)) in [(2u16, 2u16), (8, 2), (5, 8)].into_iter().enumerate() {
            let t = w.grid.id_at(col, row).unwrap();
            w.found_city(crate::world::city::City::found(
                format!("City{i}"),
                CivId(i as u8),
                t,
            ));
        }
        w.diplomacy.meet(CivId(0), CivId(2));
        w.diplomacy.meet(CivId(1), CivId(2));
        w
    }

    const CS: CivId = CivId(2);

    #[test]
    fn a_city_state_is_a_civ_that_is_not_playing_to_win() {
        let r = ruleset();
        let w = world(&r);
        assert!(w.civ(CS).unwrap().is_city_state());
        assert!(!w.civ(CS).unwrap().is_major());
        assert!(w.civ(CivId(0)).unwrap().is_major());
        assert_eq!(city_states(&w), vec![CS]);
    }

    #[test]
    fn influence_buys_rank_at_the_thresholds() {
        let r = ruleset();
        let mut w = world(&r);
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Neutral);

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(30));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Friend);

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Ally);

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(-1));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Enemy);

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(-30));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Unforgivable);
    }

    #[test]
    fn only_one_civ_can_be_the_ally() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(70));
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(65));

        assert_eq!(ally_of(&w, CS), Some(CivId(0)));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Ally);
        assert_eq!(
            relationship(&w, CS, CivId(1)),
            Relationship::Friend,
            "second place at 65 influence is a friend, not a co-ally"
        );

        // Overtake and the title moves with the number, because it was never
        // stored anywhere it could get stuck.
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(80));
        assert_eq!(ally_of(&w, CS), Some(CivId(1)));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Friend);
    }

    #[test]
    fn a_tie_for_ally_resolves_the_same_way_every_time() {
        let r = ruleset();
        let mut a = world(&r);
        a.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));
        a.diplomacy.set_influence(CS, CivId(1), Fixed::whole(60));
        let mut b = world(&r);
        b.diplomacy.set_influence(CS, CivId(1), Fixed::whole(60));
        b.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));
        assert_eq!(ally_of(&a, CS), ally_of(&b, CS));
        assert_eq!(ally_of(&a, CS), Some(CivId(0)));
    }

    #[test]
    fn war_ends_the_friendship_whatever_the_number_says() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(90));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Ally);

        w.diplomacy.declare_war(CivId(0), CS);
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Unforgivable);
        assert_eq!(ally_of(&w, CS), None, "you are not allied to what you are invading");

        // And the influence is still there underneath, so peace restores it.
        w.diplomacy.make_peace(CivId(0), CS);
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Ally);
    }

    #[test]
    fn favour_slides_back_to_where_it_belongs() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));

        // A point a turn, so an alliance bought at sixty lasts sixty turns
        // and not a turn longer. Influence is rent, not a purchase.
        for _ in 0..59 {
            tick(&mut w);
        }
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), Fixed::ONE, "still just about ours");
        tick(&mut w);
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), Fixed::ZERO);

        // And it comes back up from below, too.
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(-20));
        for _ in 0..30 {
            tick(&mut w);
        }
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), Fixed::ZERO);
    }

    #[test]
    fn it_never_overshoots_the_resting_point() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::permille(500));
        tick(&mut w);
        assert_eq!(
            w.diplomacy.influence(CS, CivId(0)),
            Fixed::ZERO,
            "half a point from home, it stops at home rather than passing it"
        );
    }

    #[test]
    fn a_protector_is_owed_a_better_resting_point() {
        let r = ruleset();
        let mut w = world(&r);
        assert_eq!(resting_point(&w, CS, CivId(0)), Fixed::ZERO);
        w.diplomacy.set_protector(CivId(0), CS, true);
        assert_eq!(resting_point(&w, CS, CivId(0)), PROTECTOR_RESTING_BONUS);

        // Which means influence settles there rather than at zero.
        for _ in 0..40 {
            tick(&mut w);
        }
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), PROTECTOR_RESTING_BONUS);
    }

    #[test]
    fn protection_is_one_sided_and_war_ends_it() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_protector(CivId(0), CS, true);
        assert!(w.diplomacy.is_protector(CivId(0), CS));
        assert!(!w.diplomacy.is_protector(CS, CivId(0)), "a city-state protects nobody");

        w.diplomacy.declare_war(CivId(0), CS);
        assert!(!w.diplomacy.is_protector(CivId(0), CS), "you do not protect what you invade");
    }

    #[test]
    fn a_gift_of_gold_buys_influence_in_round_numbers() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(CivId(0)).unwrap().gold = 1000;

        let gained = gift_gold(&mut w, CivId(0), CS, 500, 500, Fixed::ONE).expect("a legal gift");
        assert!(gained >= Fixed::whole(5), "never less than five");
        assert_eq!(
            gained.raw() % 5000,
            0,
            "{gained:?} should be a multiple of five, as the button says"
        );
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 500, "the gold left our treasury");
        assert_eq!(w.civ(CS).unwrap().gold, 500, "and arrived in theirs");
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), gained);
    }

    #[test]
    fn a_bigger_gift_buys_more() {
        let r = ruleset();
        let w = world(&r);
        let small = influence_from_gift(&w, 100, 500, Fixed::ONE, CivId(0));
        let large = influence_from_gift(&w, 1000, 500, Fixed::ONE, CivId(0));
        assert!(large > small, "{large:?} should beat {small:?}");
    }

    #[test]
    fn gold_is_worth_less_late_in_the_game() {
        let r = ruleset();
        let mut w = world(&r);
        let early = influence_from_gift(&w, 1000, 500, Fixed::ONE, CivId(0));
        w.turn = 500;
        let late = influence_from_gift(&w, 1000, 500, Fixed::ONE, CivId(0));
        assert!(late < early, "a thousand gold late ({late:?}) should buy less than early ({early:?})");
    }

    #[test]
    fn you_cannot_gift_what_you_do_not_have() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(CivId(0)).unwrap().gold = 10;
        assert_eq!(gift_gold(&mut w, CivId(0), CS, 500, 500, Fixed::ONE), None);
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 10, "and nothing was taken trying");

        // Nor buy the favour of someone you are invading.
        w.civ_mut(CivId(0)).unwrap().gold = 1000;
        w.diplomacy.declare_war(CivId(0), CS);
        assert_eq!(gift_gold(&mut w, CivId(0), CS, 500, 500, Fixed::ONE), None);

        // Nor gift a major civ, which is a trade and not this.
        w.diplomacy.make_peace(CivId(0), CS);
        assert_eq!(gift_gold(&mut w, CivId(0), CivId(1), 500, 500, Fixed::ONE), None);
    }

    #[test]
    fn a_friend_pays_and_an_ally_pays_more() {
        let r = ruleset();
        let mut w = world(&r);
        assert!(bonus_uniques(&w, &r, CivId(0)).is_empty(), "a stranger is owed nothing");

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(30));
        let friend = bonus_uniques(&w, &r, CivId(0));
        assert!(!friend.is_empty(), "a friend is owed something");

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));
        let ally = bonus_uniques(&w, &r, CivId(0));
        assert!(!ally.is_empty());
        // The ally list replaces the friend list rather than adding to it —
        // the ally uniques already restate the friend ones at a higher value,
        // so granting both would pay twice.
        let kind = r.city_state_types.get(w.civ(CS).unwrap().city_state_type.unwrap()).unwrap();
        assert_eq!(ally.len(), kind.ally_bonus_uniques.len());
        assert_eq!(friend.len(), kind.friend_bonus_uniques.len());
    }

    #[test]
    fn a_city_state_cannot_win_the_game() {
        let r = ruleset();
        let mut w = world(&r);
        // Give the city-state everything a domination victory needs: it is
        // the only civ left standing.
        w.civ_mut(CivId(0)).unwrap().defeated = true;
        w.civ_mut(CivId(1)).unwrap().defeated = true;
        assert_eq!(
            crate::world::victory::winner(&w, &r),
            None,
            "a city-state is not playing, so it cannot win by outlasting those who are"
        );
    }
}
