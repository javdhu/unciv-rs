//! Trade between major civs: gold, gold a turn, resources, open borders.
//!
//! A deal is two lists — what each side gives. Gold changes hands at once;
//! everything else runs for the speed's deal length (thirty turns at
//! standard) and is read back each turn: gold a turn is paid, traded
//! resources count in the stock of the side that received them and not the
//! side that gave them, and open borders stay open (`TradeLogic.acceptTrade`,
//! `DiplomacyTurnManager.nextTurnTrades`). A deal a side can no longer honour
//! — it gave away a resource it has since lost — is called off
//! (`removeUntenableTrades`).
//!
//! What a deal is worth to an AI is Unciv's reckoning (`TradeEvaluation`): gold
//! at face value, gold a turn at four fifths of its whole run, a luxury it
//! lacks at three hundred (more when unhappy), its last copy of one at six
//! hundred, open borders by how much it likes the asker. Everything but gold
//! is scaled by how much gold means to the civ — less, the richer it is. It
//! accepts anything worth at least what it gives.

use crate::arena::Id;
use crate::ruleset::{Ruleset, TileResource};
use crate::stats::{Fixed, Precise};
use crate::unique_type::UniqueType;
use crate::world::grid::CivId;
use crate::world::opinion::RelationshipLevel;
use crate::world::world::World;

/// One thing a side gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Item {
    Gold(i32),
    GoldPerTurn(i32),
    Luxury(Id<TileResource>),
    Strategic(Id<TileResource>, i32),
    /// The giver opens its borders to the receiver.
    OpenBorders,
}

impl Item {
    fn immediate(self) -> bool {
        matches!(self, Item::Gold(_))
    }
}

/// An item still running, and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running {
    pub item: Item,
    pub turns_left: u32,
}

/// A deal in force between two civs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deal {
    pub a: CivId,
    pub b: CivId,
    pub a_gives: Vec<Running>,
    pub b_gives: Vec<Running>,
}

impl Deal {
    /// Everything this deal has `giver` giving, and to whom.
    fn gives(&self, giver: CivId) -> (&[Running], CivId) {
        if giver == self.a {
            (&self.a_gives, self.b)
        } else {
            (&self.b_gives, self.a)
        }
    }
}

/// Why a deal cannot be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeRejection {
    /// A city-state, a barbarian, or oneself.
    NotAMajor,
    NotMet,
    AtWar,
    NotEnoughGold,
    DoesNotHave(Id<TileResource>),
    /// Open borders needs the tech that allows them, on both sides.
    OpenBordersNotYet,
    AlreadyOpen,
}

impl std::fmt::Display for TradeRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradeRejection::NotAMajor => write!(f, "only two major civs trade"),
            TradeRejection::NotMet => write!(f, "they have not met"),
            TradeRejection::AtWar => write!(f, "not while at war"),
            TradeRejection::NotEnoughGold => write!(f, "not enough gold"),
            TradeRejection::DoesNotHave(_) => write!(f, "does not have that resource"),
            TradeRejection::OpenBordersNotYet => write!(f, "open borders are not yet possible"),
            TradeRejection::AlreadyOpen => write!(f, "those borders are already open"),
        }
    }
}

impl std::error::Error for TradeRejection {}

fn held(world: &World, civ: CivId, res: Id<TileResource>) -> i32 {
    world.civ(civ).and_then(|c| c.resources.get(&res)).copied().unwrap_or(0)
}

fn may_open_borders(world: &World, civ: CivId) -> bool {
    world.civ(civ).is_some_and(|c| c.uniques.has(UniqueType::EnablesOpenBorders))
}

/// Whether `giver` can give `item` to `receiver`.
pub fn can_give(world: &World, giver: CivId, receiver: CivId, item: Item) -> Result<(), TradeRejection> {
    match item {
        Item::Gold(n) | Item::GoldPerTurn(n) => {
            if n < 0 || world.civ(giver).map_or(0, |c| c.gold) < if matches!(item, Item::Gold(_)) { n } else { 0 } {
                return Err(TradeRejection::NotEnoughGold);
            }
        }
        Item::Luxury(res) if held(world, giver, res) < 1 => return Err(TradeRejection::DoesNotHave(res)),
        Item::Strategic(res, n) if held(world, giver, res) < n => return Err(TradeRejection::DoesNotHave(res)),
        Item::OpenBorders => {
            if !may_open_borders(world, giver) || !may_open_borders(world, receiver) {
                return Err(TradeRejection::OpenBordersNotYet);
            }
            if world.diplomacy.may_enter_borders_of(receiver, giver) {
                return Err(TradeRejection::AlreadyOpen);
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_parties(world: &World, a: CivId, b: CivId) -> Result<(), TradeRejection> {
    let major = |c: CivId| world.civ(c).is_some_and(|x| x.is_major() && x.is_alive());
    if a == b || !major(a) || !major(b) {
        return Err(TradeRejection::NotAMajor);
    }
    if !world.diplomacy.have_met(a, b) {
        return Err(TradeRejection::NotMet);
    }
    if world.diplomacy.at_war(a, b) {
        return Err(TradeRejection::AtWar);
    }
    Ok(())
}

/// Make the deal (`TradeLogic.acceptTrade`): gold at once, the rest for `turns`.
pub fn accept(
    world: &mut World,
    a: CivId,
    b: CivId,
    a_gives: &[Item],
    b_gives: &[Item],
    turns: u32,
) -> Result<(), TradeRejection> {
    check_parties(world, a, b)?;
    for item in a_gives {
        can_give(world, a, b, *item)?;
    }
    for item in b_gives {
        can_give(world, b, a, *item)?;
    }
    let running = |giver: CivId, receiver: CivId, items: &[Item], world: &mut World| -> Vec<Running> {
        let mut out = Vec::new();
        for item in items {
            match *item {
                Item::Gold(n) => {
                    if let Some(c) = world.civ_mut(giver) {
                        c.gold -= n;
                    }
                    if let Some(c) = world.civ_mut(receiver) {
                        c.gold += n;
                    }
                }
                Item::OpenBorders => world.diplomacy.set_open_borders(giver, receiver, true),
                _ => {}
            }
            if !item.immediate() {
                out.push(Running { item: *item, turns_left: turns });
            }
        }
        out
    };
    let a_gives = running(a, b, a_gives, world);
    let b_gives = running(b, a, b_gives, world);
    if !a_gives.is_empty() || !b_gives.is_empty() {
        world.diplomacy.deals.push(Deal { a, b, a_gives, b_gives });
    }
    Ok(())
}

/// What a turn of trade did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeEvent {
    /// Ran its course.
    Ended { a: CivId, b: CivId },
    /// Called off: a side could no longer give what it promised.
    Cancelled { a: CivId, b: CivId },
}

fn close(world: &mut World, r: &Ruleset, deal: &Deal) {
    for (giver, receiver, items) in [(deal.a, deal.b, &deal.a_gives), (deal.b, deal.a, &deal.b_gives)] {
        if items.iter().any(|o| o.item == Item::OpenBorders) {
            world.diplomacy.set_open_borders(giver, receiver, false);
            crate::world::movement::expel(world, r, receiver, giver);
        }
    }
}

/// One turn of every deal (`nextTurnTrades`, `removeUntenableTrades`): gold a
/// turn is paid, clocks run down, and what can no longer be honoured ends.
pub fn tick(world: &mut World, r: &Ruleset) -> Vec<TradeEvent> {
    let mut events = Vec::new();
    let deals = std::mem::take(&mut world.diplomacy.deals);
    let mut keep = Vec::new();
    for mut deal in deals {
        // A side that no longer holds a resource it gave cannot keep giving it.
        let untenable = [deal.a, deal.b].iter().any(|giver| {
            deal.gives(*giver).0.iter().any(|o| match o.item {
                Item::Luxury(res) | Item::Strategic(res, _) => held(world, *giver, res) < 0,
                _ => false,
            })
        });
        if untenable {
            close(world, r, &deal);
            events.push(TradeEvent::Cancelled { a: deal.a, b: deal.b });
            continue;
        }
        for (giver, receiver, items) in [(deal.a, deal.b, &deal.a_gives), (deal.b, deal.a, &deal.b_gives)] {
            for o in items.iter() {
                if let Item::GoldPerTurn(n) = o.item {
                    if let Some(c) = world.civ_mut(giver) {
                        c.gold -= n;
                    }
                    if let Some(c) = world.civ_mut(receiver) {
                        c.gold += n;
                    }
                }
            }
        }
        for o in deal.a_gives.iter_mut().chain(deal.b_gives.iter_mut()) {
            o.turns_left = o.turns_left.saturating_sub(1);
        }
        if deal.a_gives.iter().chain(deal.b_gives.iter()).all(|o| o.turns_left == 0) {
            close(world, r, &deal);
            events.push(TradeEvent::Ended { a: deal.a, b: deal.b });
        } else {
            keep.push(deal);
        }
    }
    world.diplomacy.deals = keep;
    events
}

/// Move traded resources from giver to receiver in the stocks just rebuilt
/// from the map (`resourcesFromTrade`).
pub fn apply_to_stock(world: &mut World) {
    let mut moves: Vec<(CivId, Id<TileResource>, i32)> = Vec::new();
    for deal in &world.diplomacy.deals {
        for (giver, receiver, items) in [(deal.a, deal.b, &deal.a_gives), (deal.b, deal.a, &deal.b_gives)] {
            for o in items {
                let (res, n) = match o.item {
                    Item::Luxury(res) => (res, 1),
                    Item::Strategic(res, n) => (res, n),
                    _ => continue,
                };
                moves.push((giver, res, -n));
                moves.push((receiver, res, n));
            }
        }
    }
    for (civ, res, n) in moves {
        if let Some(c) = world.civ_mut(civ) {
            *c.resources.entry(res).or_insert(0) += n;
        }
    }
}

// ---------------------------------------------------------------------------
// What a deal is worth, to an AI.

/// How much one gold means to this civ, against a fresh start
/// (`getGoldInflation`): `1000 / (income^1.2 + 1660) + 0.4`. Income here is
/// what its cities make in gold, before upkeep.
fn inflation(world: &World, r: &Ruleset, civ: CivId) -> Precise {
    let income: i64 = world
        .civ(civ)
        .map(|c| {
            c.cities
                .iter()
                .filter_map(|id| world.cities.get(*id))
                .map(|city| crate::world::city::city_yield(&world.grid, r, city).gold.floor())
                .sum()
        })
        .unwrap_or(0);
    let powered = Precise::from_ratio(income.max(1), 1).pow(Fixed::permille(1_200)).unwrap_or(Precise::whole(1));
    let denom = powered + Precise::whole(1660);
    Precise::whole(1000).checked_div(denom).unwrap_or(Precise::whole(0)) + Precise::from_ratio(2, 5)
}

fn could_use(world: &World, r: &Ruleset, civ: CivId, res: Id<TileResource>) -> bool {
    let Some(name) = r.resources.get(res).map(|x| x.name.as_str()) else { return false };
    let Some(c) = world.civ(civ) else { return false };
    let unlocked =
        |tech: &Option<String>| tech.as_deref().is_none_or(|t| r.technologies.id(t).is_some_and(|t| c.has_tech(t)));
    r.units.iter().any(|(_, u)| u.required_resource.as_deref() == Some(name) && unlocked(&u.required_tech))
        || r.buildings.iter().any(|(_, b)| b.required_resource.as_deref() == Some(name) && unlocked(&b.required_tech))
}

/// What `item`, given by `partner`, is worth to `civ` (`evaluateBuyCost`).
fn buy_value(world: &World, r: &Ruleset, civ: CivId, partner: CivId, item: Item, turns: u32) -> i64 {
    let _ = partner;
    match item {
        Item::Gold(n) => n as i64,
        Item::GoldPerTurn(n) => n as i64 * turns as i64 * 4 / 5,
        Item::Luxury(res) => {
            if held(world, civ, res) > 0 {
                return 0; // one copy is all anyone needs
            }
            let demanding = world.civ(civ).map_or(0, |c| {
                c.cities
                    .iter()
                    .filter_map(|id| world.cities.get(*id))
                    .filter(|c| c.demanded_resource == Some(res))
                    .count()
            });
            let happiness = crate::world::happiness::happiness(world, r, civ, None);
            demanding as i64 * 50
                + match happiness {
                    h if h < 0 => 450,
                    h if h < 10 => 350,
                    _ => 300,
                }
        }
        Item::Strategic(res, n) => {
            let wanted = 2 - held(world, civ, res);
            if wanted <= 0 || !could_use(world, r, civ, res) {
                return 0;
            }
            50 * wanted.min(n) as i64
        }
        Item::OpenBorders => 100,
    }
}

/// What giving `item` to `partner` costs `civ` (`evaluateSellCost`).
fn sell_value(world: &World, r: &Ruleset, civ: CivId, partner: CivId, item: Item, turns: u32) -> i64 {
    match item {
        Item::Gold(n) => n as i64,
        Item::GoldPerTurn(n) => n as i64 * turns as i64 * 4 / 5,
        Item::Luxury(res) => {
            if held(world, civ, res) > 1 {
                250
            } else {
                600 // our last one?
            }
        }
        Item::Strategic(res, n) => {
            let at_war =
                world.civ_ids().any(|o| world.diplomacy.at_war(civ, o) && world.civ(o).is_some_and(|x| x.is_major()));
            if !at_war || !could_use(world, r, civ, res) {
                return 50 * n as i64;
            }
            // Scarce in war: the fewer left, the dearer each.
            let left = held(world, civ, res);
            ((left - n + 1)..=left).map(|k| if k > 5 { 100 } else { (6 - k as i64) * 100 }).sum()
        }
        Item::OpenBorders => match crate::world::opinion::relationship(world, civ, partner) {
            RelationshipLevel::Unforgivable => 10_000,
            RelationshipLevel::Enemy => 2_000,
            RelationshipLevel::Competitor => 500,
            RelationshipLevel::Neutral => 200,
            _ => 100,
        },
    }
}

/// How much better off `civ` is for taking `gets` and giving `gives`
/// (`getTradeAcceptability`). At or above zero, it accepts.
pub fn acceptability(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    partner: CivId,
    gives: &[Item],
    gets: &[Item],
    turns: u32,
) -> i64 {
    let infl = inflation(world, r, civ);
    let deflate = |v: i64, item: &Item| {
        if matches!(item, Item::Gold(_) | Item::GoldPerTurn(_)) {
            v
        } else {
            Precise::from_ratio(v, 1).checked_div(infl).map_or(v, |p| p.floor())
        }
    };
    let theirs: i64 = gets.iter().map(|i| deflate(buy_value(world, r, civ, partner, *i, turns), i)).sum();
    let mut ours: i64 = gives.iter().map(|i| deflate(sell_value(world, r, civ, partner, *i, turns), i)).sum();
    match crate::world::opinion::relationship(world, civ, partner) {
        RelationshipLevel::Enemy => ours = ours * 3 / 2,
        RelationshipLevel::Unforgivable => ours *= 2,
        _ => {}
    }
    theirs - ours
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(20, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w =
            World::new(g, vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)]);
        w.found_city_in(r, City::found("West", CivId(0), w.grid.id_at(3, 6).unwrap()));
        w.found_city_in(r, City::found("East", CivId(1), w.grid.id_at(16, 6).unwrap()));
        w.diplomacy.meet(CivId(0), CivId(1));
        w
    }

    fn give(w: &mut World, civ: u8, res: Id<TileResource>, n: i32) {
        w.civ_mut(CivId(civ)).unwrap().resources.insert(res, n);
    }

    #[test]
    fn gold_changes_hands_at_once() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(CivId(0)).unwrap().gold = 100;
        accept(&mut w, CivId(0), CivId(1), &[Item::Gold(60)], &[], 30).unwrap();
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 40);
        assert_eq!(w.civ(CivId(1)).unwrap().gold, 60);
        assert!(w.diplomacy.deals.is_empty(), "nothing left running");
        assert_eq!(accept(&mut w, CivId(0), CivId(1), &[Item::Gold(60)], &[], 30), Err(TradeRejection::NotEnoughGold));
    }

    #[test]
    fn gold_a_turn_is_paid_for_thirty_turns_then_stops() {
        let r = ruleset();
        let mut w = world(&r);
        accept(&mut w, CivId(0), CivId(1), &[Item::GoldPerTurn(3)], &[], 30).unwrap();
        let mut ended = None;
        for turn in 1..=40 {
            if tick(&mut w, &r).contains(&TradeEvent::Ended { a: CivId(0), b: CivId(1) }) {
                ended = Some(turn);
            }
        }
        assert_eq!(ended, Some(30));
        assert_eq!(w.civ(CivId(1)).unwrap().gold, 90);
        assert_eq!(w.civ(CivId(0)).unwrap().gold, -90);
    }

    #[test]
    fn a_traded_luxury_moves_from_one_stock_to_the_other() {
        let r = ruleset();
        let mut w = world(&r);
        let silk = r.resources.id("Silk").unwrap();
        give(&mut w, 0, silk, 2);
        accept(&mut w, CivId(0), CivId(1), &[Item::Luxury(silk)], &[], 30).unwrap();
        apply_to_stock(&mut w);
        assert_eq!(held(&w, CivId(0), silk), 1);
        assert_eq!(held(&w, CivId(1), silk), 1);
    }

    #[test]
    fn losing_what_was_promised_calls_the_deal_off() {
        let r = ruleset();
        let mut w = world(&r);
        let silk = r.resources.id("Silk").unwrap();
        give(&mut w, 0, silk, 1);
        accept(&mut w, CivId(0), CivId(1), &[Item::Luxury(silk)], &[], 30).unwrap();
        // The silk tile is lost: the rebuilt stock is empty, less the copy promised.
        give(&mut w, 0, silk, 0);
        apply_to_stock(&mut w);
        assert_eq!(tick(&mut w, &r), vec![TradeEvent::Cancelled { a: CivId(0), b: CivId(1) }]);
        assert!(w.diplomacy.deals.is_empty());
    }

    #[test]
    fn open_borders_open_and_close_with_the_deal() {
        let r = ruleset();
        let mut w = world(&r);
        for c in [0, 1] {
            w.civ_mut(CivId(c))
                .unwrap()
                .uniques
                .insert(crate::unique::Unique::parse("Enables Open Borders agreements"));
        }
        accept(&mut w, CivId(1), CivId(0), &[Item::OpenBorders], &[], 2).unwrap();
        assert!(w.diplomacy.may_enter_borders_of(CivId(0), CivId(1)));
        tick(&mut w, &r);
        tick(&mut w, &r);
        assert!(!w.diplomacy.may_enter_borders_of(CivId(0), CivId(1)), "closed when the deal runs out");
    }

    #[test]
    fn open_borders_wait_for_the_tech() {
        let r = ruleset();
        let mut w = world(&r);
        assert_eq!(
            accept(&mut w, CivId(1), CivId(0), &[Item::OpenBorders], &[], 30),
            Err(TradeRejection::OpenBordersNotYet)
        );
    }

    #[test]
    fn a_fair_luxury_swap_is_accepted_and_a_lopsided_one_is_not() {
        let r = ruleset();
        let mut w = world(&r);
        let (silk, wine) = (r.resources.id("Silk").unwrap(), r.resources.id("Wine").unwrap());
        give(&mut w, 0, silk, 2);
        give(&mut w, 1, wine, 2);
        // A spare for one it lacks: 300-odd in, 250 out.
        assert!(acceptability(&w, &r, CivId(1), CivId(0), &[Item::Luxury(wine)], &[Item::Luxury(silk)], 30) >= 0);
        // Its last copy for nothing: never.
        give(&mut w, 1, wine, 1);
        assert!(acceptability(&w, &r, CivId(1), CivId(0), &[Item::Luxury(wine)], &[], 30) < 0);
    }

    #[test]
    fn an_ai_offers_a_spare_luxury_for_one_it_lacks() {
        let r = ruleset();
        let mut w = world(&r);
        let (silk, wine) = (r.resources.id("Silk").unwrap(), r.resources.id("Wine").unwrap());
        give(&mut w, 0, silk, 2);
        give(&mut w, 1, wine, 2);
        assert_eq!(
            crate::world::ai::choose_trades(&w, &r, CivId(0)),
            vec![(CivId(1), vec![Item::Luxury(silk)], vec![Item::Luxury(wine)])]
        );
        // Nothing to spare, nothing offered.
        give(&mut w, 0, silk, 1);
        assert!(crate::world::ai::choose_trades(&w, &r, CivId(0)).is_empty());
    }

    #[test]
    fn gold_a_turn_is_worth_four_fifths_of_its_run() {
        let r = ruleset();
        let w = world(&r);
        assert_eq!(acceptability(&w, &r, CivId(1), CivId(0), &[], &[Item::GoldPerTurn(5)], 30), 120);
    }
}
