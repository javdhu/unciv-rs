//! Buying a construction outright, with gold or with faith.
//!
//! One pipeline for both stats, as in Unciv (`IConstruction.getBaseBuyCost`,
//! `Building.getStatBuyCost`, `BaseUnitCost.getStatBuyCost`):
//!
//! 1. A base price — the cheapest a unique names for this item and stat
//!    (a price that rises with each one bought, a multiple of the production
//!    cost, the era's base price, a flat amount); failing that the item's own
//!    `Can be purchased for`; failing that, for gold, `(30 × cost)^0.75`
//!    raised by the item's `hurryCostModifier`; failing that, for any other
//!    stat, the era's base price if the item says it can be bought with it.
//! 2. Every discount that applies, multiplied in.
//! 3. Truncated to a multiple of ten — down, not to nearest.
//!
//! The `^0.75` and the truncation together are why this is worked in
//! [`Precise`]: a price a hair under a multiple of ten drops a whole ten, and
//! thousandths are not fine enough to know which side of it a price is on.
//!
//! What cannot be bought: a wonder, with gold, unless a unique allows it;
//! anything `Cannot be purchased`; anything the city could not build anyway.

use crate::arena::Id;
use crate::ruleset::{Building, Ruleset};
use crate::stats::{Fixed, Precise, Stat};
use crate::unique::Unique;
use crate::unique_type::UniqueType;
use crate::world::city::City;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::construction::{production_cost_in, Construction, CostModifiers, Rejection};
use crate::world::entities::Unit;
use crate::world::filters::{base_unit_matches, building_matches, city_matches};
use crate::world::world::World;

/// Why something could not be bought.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurchaseRejection {
    NoSuchCity,
    /// The city could not build it at all.
    Cannot(Rejection),
    /// Not for sale for this stat — a wonder for gold, say.
    NotForSale,
    /// A puppet's governor does the buying, unless a unique says otherwise.
    Puppet,
    /// Nothing is bought in a city still resisting its conqueror.
    InResistance,
    /// A purchased unit needs a free slot of its type in its birth city.
    TileOccupied,
    NotEnough {
        cost: i64,
        has: i64,
    },
}

impl std::fmt::Display for PurchaseRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PurchaseRejection::NoSuchCity => write!(f, "no such city"),
            PurchaseRejection::Cannot(why) => write!(f, "cannot be built: {why}"),
            PurchaseRejection::NotForSale => write!(f, "cannot be bought with that"),
            PurchaseRejection::Puppet => write!(f, "a puppet does its own buying"),
            PurchaseRejection::InResistance => write!(f, "the city is resisting"),
            PurchaseRejection::TileOccupied => write!(f, "the city has no free slot for this unit"),
            PurchaseRejection::NotEnough { cost, has } => write!(f, "costs {cost}, has {has}"),
        }
    }
}

impl std::error::Error for PurchaseRejection {}

/// Everything about the city a purchase question needs, looked up once.
struct Shop<'a> {
    world: &'a World,
    r: &'a Ruleset,
    city: &'a City,
    ctx: GameContext<'a>,
    m: CostModifiers,
}

impl<'a> Shop<'a> {
    fn open(world: &'a World, r: &'a Ruleset, city_id: Id<City>, m: CostModifiers) -> Option<Shop<'a>> {
        let city = world.cities.get(city_id)?;
        let ctx = GameContext::new(r).with_world(world).for_civ(city.owner).in_city(city_id);
        Some(Shop { world, r, city, ctx, m })
    }

    /// Uniques in force here: the city's buildings', then the civ's.
    fn uniques(&self, t: UniqueType) -> impl Iterator<Item = &'a Unique> + '_ {
        crate::world::religion::city_uniques(self.world, self.r, self.city)
            .into_iter()
            .filter(move |u| u.unique_type() == Some(t) && applies(u, &self.ctx) == Verdict::True)
    }

    /// The item's own uniques, whose conditionals hold here.
    fn own(&self, what: Construction, t: UniqueType) -> impl Iterator<Item = &'a Unique> + '_ {
        let list: &'a [Unique] = match what {
            Construction::Unit(id) => self.r.units.get(id).map_or(&[], |u| u.uniques.as_slice()),
            Construction::Building(id) => self.r.buildings.get(id).map_or(&[], |b| b.uniques.as_slice()),
        };
        list.iter().filter(move |u| u.unique_type() == Some(t) && applies(u, &self.ctx) == Verdict::True)
    }

    fn city_is(&self, filter: &str) -> bool {
        city_matches(self.world, self.r, self.city, Some(self.city), filter) == Verdict::True
    }

    fn item_is(&self, what: Construction, filter: &str) -> bool {
        match what {
            Construction::Unit(id) => base_unit_matches(self.r, id, filter) == Verdict::True,
            Construction::Building(id) => building_matches(self.r, id, filter) == Verdict::True,
        }
    }

    /// Speed's multiplier for a stat (`statCostModifiers`).
    fn speed(&self, stat: Stat) -> Precise {
        Precise::from_fixed(match stat {
            Stat::Gold => self.m.gold_cost,
            Stat::Faith => self.m.faith_cost,
            _ => Fixed::ONE,
        })
    }

    /// The era's base price for a bought unit, at this speed.
    fn era_price(&self, stat: Stat) -> Precise {
        let era = crate::world::policies::era_number(self.world, self.r, self.city.owner);
        let base = self.r.eras.iter().find(|(_, e)| e.era_number == era).map_or(0, |(_, e)| e.base_unit_buy_cost);
        Precise::from_ratio(base as i64, 1) * self.speed(stat)
    }

    fn production_cost(&self, what: Construction) -> Option<i64> {
        production_cost_in(self.world, self.r, self.ctx.city?, what, self.m).map(i64::from)
    }

    /// Prices the city's and civ's uniques name for this item (`getSpecificBuyCost`).
    fn specific_prices(&self, what: Construction, stat: Stat) -> Vec<Precise> {
        let unit = matches!(what, Construction::Unit(_));
        let pick = |a: UniqueType, b: UniqueType| if unit { a } else { b };
        let name = stat.name();
        let mut out = Vec::new();

        // May buy [item] for [n] [stat] [cityFilter] at an increasing price ([step])
        for u in self.uniques(pick(UniqueType::BuyUnitsIncreasingCost, UniqueType::BuyBuildingsIncreasingCost)) {
            let p = &u.params;
            if p.len() >= 5 && p[2] == name && self.item_is(what, &p[0]) && self.city_is(&p[3]) {
                let (base, step) = (p[1].parse::<i64>().unwrap_or(0), p[4].parse::<i64>().unwrap_or(0));
                let bought = self.bought_before(what);
                // `(base + step / 2f * (n² + n)).toInt()`
                let price = Precise::from_ratio(2 * base + step * (bought * bought + bought), 2).floor();
                out.push(Precise::from_ratio(price, 1) * self.speed(stat));
            }
        }
        // May buy [item] with [stat] for [n] times their normal Production cost
        for u in self.uniques(pick(UniqueType::BuyUnitsByProductionCost, UniqueType::BuyBuildingsByProductionCost)) {
            let p = &u.params;
            if p.len() >= 3 && p[1] == name && self.item_is(what, &p[0]) {
                if let (Some(cost), Ok(times)) = (self.production_cost(what), p[2].parse::<i64>()) {
                    out.push(Precise::from_ratio(cost * times, 1));
                }
            }
        }
        // May buy [item] with [stat] [cityFilter]
        if self.uniques(pick(UniqueType::BuyUnitsWithStat, UniqueType::BuyBuildingsWithStat)).any(|u| {
            u.params.len() >= 3 && u.params[1] == name && self.item_is(what, &u.params[0]) && self.city_is(&u.params[2])
        }) {
            out.push(self.era_price(stat));
        }
        // May buy [item] for [n] [stat] [cityFilter]
        for u in self.uniques(pick(UniqueType::BuyUnitsForAmountStat, UniqueType::BuyBuildingsForAmountStat)) {
            let p = &u.params;
            if p.len() >= 4 && p[2] == name && self.item_is(what, &p[0]) && self.city_is(&p[3]) {
                out.push(Precise::from_ratio(p[1].parse::<i64>().unwrap_or(0), 1) * self.speed(stat));
            }
        }
        out
    }

    fn bought_before(&self, what: Construction) -> i64 {
        let name = item_name(self.r, what);
        self.world.civ(self.city.owner).and_then(|c| c.bought_with_increasing_price.get(name)).map_or(0, |n| *n as i64)
    }

    /// The price before discounts (`getBaseBuyCost`).
    fn base_price(&self, what: Construction, stat: Stat) -> Option<Precise> {
        if let Some(cheapest) = self.specific_prices(what, stat).into_iter().min() {
            return Some(cheapest);
        }
        // Can be purchased for [amount] [stat] [cityFilter] — the lowest.
        let named = self
            .own(what, UniqueType::CanBePurchasedForAmountStat)
            .filter(|u| u.params.len() >= 3 && u.params[1] == stat.name() && self.city_is(&u.params[2]))
            .filter_map(|u| u.params[0].parse::<i64>().ok())
            .min();
        if let Some(amount) = named {
            return Some(Precise::from_ratio(amount, 1) * self.speed(stat));
        }
        if stat == Stat::Gold {
            let hurry = match what {
                Construction::Unit(id) => self.r.units.get(id).and_then(|u| u.hurry_cost_modifier),
                Construction::Building(id) => self.r.buildings.get(id).and_then(|b| b.hurry_cost_modifier),
            };
            return Some(gold_price(self.production_cost(what)?, hurry.unwrap_or(0)));
        }
        let with_stat = self
            .own(what, UniqueType::CanBePurchasedWithStat)
            .any(|u| u.params.len() >= 2 && u.params[0] == stat.name() && self.city_is(&u.params[1]));
        with_stat.then(|| self.era_price(stat))
    }

    /// Whether this item is for sale for this stat at all (`canBePurchasedWithStat`).
    ///
    /// A city's or civ's unique naming the item outranks the item's own
    /// `Cannot be purchased` — which is how a Great Prophet, unpurchasable in
    /// itself, is bought with faith once a belief says it may be.
    fn for_sale(&self, what: Construction, stat: Stat) -> bool {
        if matches!(stat, Stat::Production | Stat::Happiness) {
            return false;
        }
        let cannot = self.own(what, UniqueType::CannotBePurchased).next().is_some();
        let allowed_by_unique = !cannot
            && (self
                .own(what, UniqueType::CanBePurchasedWithStat)
                .any(|u| u.params.len() >= 2 && u.params[0] == stat.name() && self.city_is(&u.params[1]))
                || self
                    .own(what, UniqueType::CanBePurchasedForAmountStat)
                    .any(|u| u.params.len() >= 3 && u.params[1] == stat.name() && self.city_is(&u.params[2])));
        if stat == Stat::Gold && is_wonder(self.r, what) && !allowed_by_unique {
            return false;
        }
        if !self.specific_prices(what, stat).is_empty() {
            return true;
        }
        if cannot {
            return false;
        }
        allowed_by_unique || (stat == Stat::Gold && self.own(what, UniqueType::Unbuildable).next().is_none())
    }

    fn price(&self, what: Construction, stat: Stat) -> Option<i64> {
        let mut cost = self.base_price(what, stat)?;
        let name = stat.name();
        let pct = |s: &str| Fixed::parse(s).map(|p| Precise::from_ratio(100 + p.round(), 100));
        // [stat] cost of purchasing items in cities [n]%
        for u in self.uniques(UniqueType::BuyItemsDiscount) {
            if u.params.first().is_some_and(|s| s == name) {
                if let Some(f) = u.params.get(1).and_then(|p| pct(p)) {
                    cost = cost * f;
                }
            }
        }
        // [stat] cost of purchasing [item] units/buildings [n]%
        let specific = match what {
            Construction::Unit(_) => UniqueType::BuyUnitsDiscount,
            Construction::Building(_) => UniqueType::BuyBuildingsDiscount,
        };
        for u in self.uniques(specific) {
            let p = &u.params;
            if p.len() >= 3 && p[0] == name && self.item_is(what, &p[1]) {
                if let Some(f) = pct(&p[2]) {
                    cost = cost * f;
                }
            }
        }
        // `(cost / 10f).toInt() * 10`
        Some((cost.floor() / 10) * 10)
    }
}

/// `(30 × cost)^0.75 × (100 + hurry)%`, before any discount (`getBaseGoldCost`).
pub fn gold_price(production_cost: i64, hurry_cost_modifier: i32) -> Precise {
    let base = Precise::from_ratio(30 * production_cost, 1).pow(Fixed::permille(750)).unwrap_or(Precise::whole(0));
    base * Precise::from_ratio(100 + hurry_cost_modifier as i64, 100)
}

fn item_name(r: &Ruleset, what: Construction) -> &str {
    match what {
        Construction::Unit(id) => r.units.get(id).map_or("", |u| u.name.as_str()),
        Construction::Building(id) => r.buildings.get(id).map_or("", |b| b.name.as_str()),
    }
}

fn is_wonder(r: &Ruleset, what: Construction) -> bool {
    match what {
        Construction::Building(id) => r
            .buildings
            .get(id)
            .is_some_and(|b: &Building| b.is_wonder == Some(true) || b.is_national_wonder == Some(true)),
        Construction::Unit(_) => false,
    }
}

/// What buying this here would cost in this stat, if it is for sale at all.
pub fn cost(
    world: &World,
    r: &Ruleset,
    city: Id<City>,
    what: Construction,
    stat: Stat,
    m: CostModifiers,
) -> Option<i64> {
    let shop = Shop::open(world, r, city, m)?;
    if !shop.for_sale(what, stat) {
        return None;
    }
    shop.price(what, stat)
}

/// What a unique could do to [`cost`] for one item in one stat, were it in
/// force.
///
/// Answered from the unique's kind, stat and item filter alone — the
/// parameters the pipeline above matches on — never from its conditionals or
/// its city filter, so a caller that must not evaluate those (a player who
/// cannot see what they hinge on) can still tell whether they matter here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bearing {
    /// It never touches this item's price in this stat.
    Unrelated,
    /// It may move the price of an item that is for sale either way.
    Price,
    /// It may decide whether the item is for sale in this stat at all.
    Sale,
}

/// How a unique of the city or its civ bears on this item's price: the
/// `May buy …` prices name an item and a stat, the discounts a stat (and, for
/// the specific ones, an item).
pub fn bearing(r: &Ruleset, unique: &Unique, what: Construction, stat: Stat) -> Bearing {
    use Construction::{Building as B, Unit as U};
    use UniqueType as T;
    if matches!(stat, Stat::Production | Stat::Happiness) {
        return Bearing::Unrelated;
    }
    let p = &unique.params;
    // `p[at]` is this stat, `p[item]` matches this item, as `Shop` asks.
    let names = |len: usize, at: usize, item: usize| {
        p.len() >= len
            && p[at] == stat.name()
            && match what {
                U(id) => base_unit_matches(r, id, &p[item]) == Verdict::True,
                B(id) => building_matches(r, id, &p[item]) == Verdict::True,
            }
    };
    match (unique.unique_type(), what) {
        (Some(T::BuyUnitsIncreasingCost), U(_)) | (Some(T::BuyBuildingsIncreasingCost), B(_)) if names(5, 2, 0) => {
            Bearing::Sale
        }
        (Some(T::BuyUnitsByProductionCost), U(_)) | (Some(T::BuyBuildingsByProductionCost), B(_)) if names(3, 1, 0) => {
            Bearing::Sale
        }
        (Some(T::BuyUnitsWithStat), U(_)) | (Some(T::BuyBuildingsWithStat), B(_)) if names(3, 1, 0) => Bearing::Sale,
        (Some(T::BuyUnitsForAmountStat), U(_)) | (Some(T::BuyBuildingsForAmountStat), B(_)) if names(4, 2, 0) => {
            Bearing::Sale
        }
        (Some(T::BuyItemsDiscount), _) if p.first().is_some_and(|s| s == stat.name()) => Bearing::Price,
        (Some(T::BuyUnitsDiscount), U(_)) | (Some(T::BuyBuildingsDiscount), B(_)) if names(3, 0, 1) => Bearing::Price,
        _ => Bearing::Unrelated,
    }
}

/// How one of the item's own uniques bears on its price in `stat`: whether it
/// is for sale is the item's to say (`Can be purchased …`, `Cannot be
/// purchased`, and for gold `Unbuildable`), and its production cost is what a
/// gold price, or a price in `times their normal Production cost`, is worked
/// from.
pub fn own_bearing(unique: &Unique, stat: Stat) -> Bearing {
    use UniqueType as T;
    if matches!(stat, Stat::Production | Stat::Happiness) {
        return Bearing::Unrelated;
    }
    let p = &unique.params;
    match unique.unique_type() {
        Some(T::CannotBePurchased) => Bearing::Sale,
        Some(T::CanBePurchasedWithStat) if p.first().is_some_and(|s| s == stat.name()) => Bearing::Sale,
        Some(T::CanBePurchasedForAmountStat) if p.get(1).is_some_and(|s| s == stat.name()) => Bearing::Sale,
        Some(T::Unbuildable) if stat == Stat::Gold => Bearing::Sale,
        Some(T::CostIncreasesPerCity | T::CostPercentageChange) => Bearing::Price,
        _ => Bearing::Unrelated,
    }
}

fn reserve(world: &World, civ: crate::world::grid::CivId, stat: Stat) -> i64 {
    world.civ(civ).map_or(0, |c| match stat {
        Stat::Gold => c.gold as i64,
        Stat::Faith => c.faith.stored as i64,
        _ => 0,
    })
}

/// Whether it can be bought now, and at what price (`isConstructionPurchaseAllowed`).
pub fn can_purchase(
    world: &World,
    r: &Ruleset,
    city: Id<City>,
    what: Construction,
    stat: Stat,
    m: CostModifiers,
) -> Result<i64, PurchaseRejection> {
    let c = world.cities.get(city).ok_or(PurchaseRejection::NoSuchCity)?;
    let civ = world.civ(c.owner).ok_or(PurchaseRejection::NoSuchCity)?;
    let puppet_may_buy = c.uniques.has(UniqueType::MayBuyConstructionsInPuppets)
        || civ.uniques.has(UniqueType::MayBuyConstructionsInPuppets);
    if c.puppet && !puppet_may_buy {
        return Err(PurchaseRejection::Puppet);
    }
    if c.resistance > 0 {
        return Err(PurchaseRejection::InResistance);
    }
    // Religious units are intentionally unbuildable, so they have no
    // production cost. Their faith-purchase unique supplies eligibility and
    // price; all other restrictions still apply.
    let religious = match what {
        Construction::Unit(id) => r
            .units
            .get(id)
            .is_some_and(|b| b.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::ReligiousUnit))),
        Construction::Building(_) => false,
    };
    crate::world::construction::can_purchase_in(world, r, c.owner, c, what).map_err(PurchaseRejection::Cannot)?;
    if let Construction::Unit(id) = what {
        // Also applies to faith-only units that bypass NotProducible above.
        if !crate::world::resources::can_supply_unit(world, r, c.owner, id, None) {
            return Err(PurchaseRejection::Cannot(Rejection::MissingResources));
        }
        let base = r.units.get(id).ok_or(PurchaseRejection::Cannot(Rejection::Unknown))?;
        let ctx = GameContext::new(r).with_world(world).for_civ(c.owner).in_city(city).on_tile(c.centre);
        if base
            .uniques
            .iter()
            .any(|u| u.unique_type() == Some(UniqueType::OnlyAvailable) && applies(u, &ctx) != Verdict::True)
        {
            return Err(PurchaseRejection::NotForSale);
        }
        let mut unit = Unit::new(id, c.owner, c.centre, base.movement);
        crate::world::religious_units::initialize_unit(world, r, &mut unit, Some(city));
        crate::world::military::train(world, r, city, &mut unit);
        if religious && unit.religion.is_none() && !unit.has_unique(r, UniqueType::MayFoundReligion) {
            return Err(PurchaseRejection::NotForSale);
        }
        if crate::world::movement::is_tile_occupied_for(world, r, c.centre, &unit) {
            return Err(PurchaseRejection::TileOccupied);
        }
    }
    let cost = self::cost(world, r, city, what, stat, m).ok_or(PurchaseRejection::NotForSale)?;
    let has = reserve(world, c.owner, stat);
    if cost > 0 && has < cost {
        return Err(PurchaseRejection::NotEnough { cost, has });
    }
    Ok(cost)
}

/// Buy it: pay, and have it now (`purchaseConstruction`).
///
/// A building stands at once; a unit appears in the city. Existing work stays
/// attached to its item, so buying a unit does not erase a partially produced
/// unit of that type or turn its investment into another item's production.
pub fn purchase(
    world: &mut World,
    r: &Ruleset,
    city: Id<City>,
    what: Construction,
    stat: Stat,
    m: CostModifiers,
) -> Result<i64, PurchaseRejection> {
    let science = crate::world::research::ScienceModifiers {
        speed: crate::world::slices::science_cost(world, r, world.speed_rules(r, m.speed).map_or(m.speed, |s| s.science_cost_modifier)),
        ..Default::default()
    };
    purchase_with_science(world, r, city, what, stat, m, science)
}
pub fn purchase_with_science(
    world: &mut World,
    r: &Ruleset,
    city: Id<City>,
    what: Construction,
    stat: Stat,
    m: CostModifiers,
    science: crate::world::research::ScienceModifiers,
) -> Result<i64, PurchaseRejection> {
    let cost = can_purchase(world, r, city, what, stat, m)?;
    let rises = {
        let shop = Shop::open(world, r, city, m).ok_or(PurchaseRejection::NoSuchCity)?;
        let (units, buildings) = (UniqueType::BuyUnitsIncreasingCost, UniqueType::BuyBuildingsIncreasingCost);
        let rises = shop.uniques(units).chain(shop.uniques(buildings)).any(|u| {
            u.params.len() >= 4
                && u.params[2] == stat.name()
                && shop.item_is(what, &u.params[0])
                && shop.city_is(&u.params[3])
        });
        rises
    };
    let (owner, centre) = {
        let c = world.cities.get_mut(city).ok_or(PurchaseRejection::NoSuchCity)?;
        if let Some(at) = c.queue.iter().position(|q| *q == what) {
            c.queue.remove(at);
        }
        if let Construction::Building(id) = what {
            c.built.insert(id);
            crate::world::city::rebuild_city_uniques(r, c);
        }
        (c.owner, c.centre)
    };
    if let Construction::Unit(base) = what {
        let movement = r.units.get(base).map_or(2, |u| u.movement);
        let mut unit = Unit::new(base, owner, centre, movement);
        for name in &r.units.get(base).expect("validated base").promotions {
            if let Some(promotion) = r.promotions.id(name) {
                unit.promotions.insert(promotion);
            }
        }
        crate::world::religious_units::initialize_unit(world, r, &mut unit, Some(city));
        crate::world::military::train(world, r, city, &mut unit);
        if unit.is_air(r) {
            unit.transport = crate::world::air::base_for(world, r, &unit, centre, None)
                .expect("purchase validated air capacity")
                .transport();
        }
        if unit.has_unique(r, UniqueType::ReligiousUnit)
            && !unit.has_unique(r, UniqueType::CanMoveImmediatelyOnceBought)
        {
            unit.movement_left = Fixed::ZERO;
        }
        let id = world.place_unit(unit);
        crate::world::luminaries::name_unit(world, r, id);
    }
    let name = item_name(r, what).to_string();
    if let Some(c) = world.civ_mut(owner) {
        match stat {
            Stat::Gold => c.gold -= cost as i32,
            Stat::Faith => c.faith.stored -= cost as i32,
            _ => {}
        }
        if rises {
            *c.bought_with_increasing_price.entry(name).or_insert(0) += 1;
        }
    }
    let stock = crate::world::resources::available_stock(world, r, owner);
    if let Some(c) = world.civ_mut(owner) {
        c.resources = stock;
    }
    crate::world::effects::on_construction_completed_with_science(world, r, city, what, m.speed, science);
    Ok(cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::construction::production_cost;
    use crate::world::entities::Civilization;
    use crate::world::grid::{CivId, TileGrid};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> (World, Id<City>) {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let city = w.found_city_in(r, City::found("Home", CivId(0), w.grid.id_at(8, 8).unwrap()));
        let civ = w.civ_mut(CivId(0)).unwrap();
        civ.gold = 10_000;
        // Warriors need Agriculture in this ruleset.
        civ.techs.insert(r.technologies.id("Agriculture").unwrap());
        (w, city)
    }

    fn building(r: &Ruleset, name: &str) -> Construction {
        Construction::Building(r.buildings.id(name).unwrap())
    }

    #[test]
    fn the_gold_formula_matches_the_floating_point_original() {
        // `((30 * cost)^0.75 * (1 + hurry / 100) / 10).toInt() * 10`, in f64,
        // across every production cost a game reaches and the hurry
        // modifiers the ruleset uses.
        let mut checked = 0;
        for cost in 1..=3000i64 {
            for hurry in [0i32, 25, 50, -25] {
                let want =
                    (((30.0 * cost as f64).powf(0.75) * (100.0 + hurry as f64) / 100.0) / 10.0).trunc() as i64 * 10;
                let got = (gold_price(cost, hurry).floor() / 10) * 10;
                assert_eq!(got, want, "cost {cost}, hurry {hurry}");
                checked += 1;
            }
        }
        assert_eq!(checked, 12_000);
    }

    #[test]
    fn a_monument_costs_what_unciv_charges() {
        let r = ruleset();
        let (w, city) = world(&r);
        let monument = building(&r, "Monument");
        let hammers = production_cost(&r, monument, CostModifiers::default()).unwrap() as i64;
        let hurry = r.buildings[r.buildings.id("Monument").unwrap()].hurry_cost_modifier.unwrap_or(0);
        let want = (gold_price(hammers, hurry).floor() / 10) * 10;
        assert_eq!(cost(&w, &r, city, monument, Stat::Gold, CostModifiers::default()), Some(want));
        println!("Monument: {hammers} hammers, {want} gold");
    }

    #[test]
    fn a_bought_building_stands_at_once_and_the_gold_is_gone() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let monument = building(&r, "Monument");
        w.cities.get_mut(city).unwrap().queue.push(monument);
        w.cities.get_mut(city).unwrap().production_progress.insert(monument, 7);

        let paid = purchase(&mut w, &r, city, monument, Stat::Gold, CostModifiers::default()).expect("for sale");
        let c = w.cities.get(city).unwrap();
        assert!(c.built.contains(&r.buildings.id("Monument").unwrap()));
        assert!(c.queue.is_empty(), "it leaves the queue");
        assert_eq!(c.production_for(monument), 7, "purchase does not transfer the investment");
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 10_000 - paid as i32);

        assert!(matches!(
            purchase(&mut w, &r, city, monument, Stat::Gold, CostModifiers::default()),
            Err(PurchaseRejection::Cannot(Rejection::AlreadyBuilt))
        ));
    }

    #[test]
    fn a_bought_unit_appears_in_the_city() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let warrior = Construction::Unit(r.units.id("Warrior").unwrap());
        let before = w.units.len();
        purchase(&mut w, &r, city, warrior, Stat::Gold, CostModifiers::default()).expect("for sale");
        assert_eq!(w.units.len(), before + 1);
        let centre = w.cities.get(city).unwrap().centre;
        assert!(w.units_at(centre).iter().any(|u| w.units.get(*u).unwrap().base == r.units.id("Warrior").unwrap()));
    }

    #[test]
    fn a_wonder_is_not_for_sale() {
        let r = ruleset();
        let (w, city) = world(&r);
        let pyramids = building(&r, "The Pyramids");
        assert_eq!(cost(&w, &r, city, pyramids, Stat::Gold, CostModifiers::default()), None);
    }

    #[test]
    fn nothing_is_for_sale_for_faith_without_a_unique_that_says_so() {
        let r = ruleset();
        let (w, city) = world(&r);
        let monument = building(&r, "Monument");
        assert_eq!(cost(&w, &r, city, monument, Stat::Faith, CostModifiers::default()), None);
    }

    #[test]
    fn a_short_purse_is_refused_with_the_price() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.civ_mut(CivId(0)).unwrap().gold = 5;
        let monument = building(&r, "Monument");
        assert!(matches!(
            can_purchase(&w, &r, city, monument, Stat::Gold, CostModifiers::default()),
            Err(PurchaseRejection::NotEnough { has: 5, .. })
        ));
    }

    #[test]
    fn a_discount_is_multiplied_in_before_the_truncation() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let monument = building(&r, "Monument");
        let hammers = production_cost(&r, monument, CostModifiers::default()).unwrap() as i64;
        w.civ_mut(CivId(0)).unwrap().uniques.insert(Unique::parse("[Gold] cost of purchasing items in cities [-25]%"));
        let hurry = r.buildings[r.buildings.id("Monument").unwrap()].hurry_cost_modifier.unwrap_or(0);
        let want = ((gold_price(hammers, hurry) * Precise::from_ratio(75, 100)).floor() / 10) * 10;
        assert_eq!(want, 210, "280 for a Monument, a quarter off");
        assert_eq!(cost(&w, &r, city, monument, Stat::Gold, CostModifiers::default()), Some(want));
    }

    #[test]
    fn a_belief_can_sell_what_is_otherwise_not_for_sale() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let prophet = r.units.id("Great Prophet").expect("the ruleset has prophets");
        let what = Construction::Unit(prophet);
        assert_eq!(cost(&w, &r, city, what, Stat::Gold, CostModifiers::default()), None, "not for gold");
        assert_eq!(cost(&w, &r, city, what, Stat::Faith, CostModifiers::default()), None, "nor faith, yet");
        w.civ_mut(CivId(0)).unwrap().uniques.insert(Unique::parse(
            "May buy [Great Prophet] units for [200] [Faith] [in all cities] at an increasing price ([100])",
        ));
        assert_eq!(cost(&w, &r, city, what, Stat::Faith, CostModifiers::default()), Some(200));
    }

    #[test]
    fn a_rich_ai_buys_what_its_city_is_building() {
        use crate::world::assignment::CityFocus;
        use crate::world::turn::{advance_turn, TurnEvent, TurnModifiers};
        let r = ruleset();
        let (mut w, city) = world(&r);
        let monument = building(&r, "Monument");
        w.cities.get_mut(city).unwrap().queue.push(monument);
        // A Monument's hurry modifier is 40, so Unciv's AI wants 4,000 in hand.
        w.civ_mut(CivId(0)).unwrap().gold = 5_000;

        let events = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
        let bought: Vec<_> = events
            .iter()
            .filter_map(|e| if let TurnEvent::Purchased { what, gold, .. } = e { Some((*what, *gold)) } else { None })
            .collect();
        assert_eq!(bought, vec![(monument, 280)]);
        assert!(w.cities.get(city).unwrap().built.contains(&r.buildings.id("Monument").unwrap()));
    }

    #[test]
    fn a_frugal_ai_does_not_overpay() {
        use crate::world::assignment::CityFocus;
        use crate::world::turn::{advance_turn, TurnEvent, TurnModifiers};
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.cities.get_mut(city).unwrap().queue.push(building(&r, "Monument"));
        // Enough to pay, not enough to accept a 40% markup.
        w.civ_mut(CivId(0)).unwrap().gold = 1_000;
        let events = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
        assert!(!events.iter().any(|e| matches!(e, TurnEvent::Purchased { .. })));
    }

    #[test]
    fn a_faith_price_that_rises_with_each_one_bought() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.civ_mut(CivId(0)).unwrap().faith.stored = 10_000;
        w.civ_mut(CivId(0)).unwrap().uniques.insert(Unique::parse(
            "May buy [Warrior] units for [200] [Faith] [in all cities] at an increasing price ([100])",
        ));
        let warrior = Construction::Unit(r.units.id("Warrior").unwrap());

        // 200, then 200 + 50·(1 + 1) = 300, then 200 + 50·(4 + 2) = 500.
        for (index, want) in [200, 300, 500].into_iter().enumerate() {
            assert_eq!(cost(&w, &r, city, warrior, Stat::Faith, CostModifiers::default()), Some(want));
            purchase(&mut w, &r, city, warrior, Stat::Faith, CostModifiers::default()).expect("for sale");
            let born = w.units_at(w.cities.get(city).unwrap().centre)[0];
            w.units.get_mut(born).unwrap().position = w.grid.id_at(index as u16, 0).unwrap();
        }
        assert_eq!(w.civ(CivId(0)).unwrap().faith.stored, 10_000 - 1_000);
    }

    #[test]
    fn purchases_cannot_reserve_the_same_last_resource_twice() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let iron = r.resources.id("Iron").unwrap();
        let swordsman = Construction::Unit(r.units.id("Swordsman").unwrap());
        let centre = w.cities.get(city).unwrap().centre;
        for name in ["Iron Working", "Mining", "Bronze Working"] {
            w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id(name).unwrap());
        }
        w.grid.resource[centre.0 as usize] = Some(iron);
        w.grid.resource_amount[centre.0 as usize] = 1;
        purchase(&mut w, &r, city, swordsman, Stat::Gold, CostModifiers::default()).unwrap();
        let born = w.units_at(centre)[0];
        w.units.get_mut(born).unwrap().position = crate::world::grid::TileId(0);
        w.civ_mut(CivId(0)).unwrap().resources.insert(iron, 100); // stale cache must not grant supply
        let before = w.clone();
        assert_eq!(
            purchase(&mut w, &r, city, swordsman, Stat::Gold, CostModifiers::default()),
            Err(PurchaseRejection::Cannot(Rejection::MissingResources))
        );
        assert_eq!(w, before, "failed reservation spends no money or production");
    }
}
