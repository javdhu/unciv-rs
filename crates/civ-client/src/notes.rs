//! The notification log's lines (docs/ACTIONS-SPEC.md, A1): each presentation event in the player's words, with
//! the tile to look at and how much it matters. Names come from the view and the rule tables; what neither
//! resolves reads as a plain noun ("a city", "a nation", "a unit"), never an id.
//!
//! `describe` matches every `PresentationEvent` variant by name, so a variant the engine adds fails to compile here.
//! It answers None only for what the map already shows: a step (`unitMoved`), a unit fortifying (its piece shows
//! it), another nation's unit promoted or upgraded in sight (its piece), and a fight or withdrawal between other
//! nations seen in the replay (animated, or walked as a `unitMoved`).
//!
//! Three payloads are typed in civ-sim but not exported from it (`NoticeKind`, `ReligiousEventKind`,
//! `GreatPersonOutcome`), so they are read by their wire names, and a name this client does not know reads as a
//! generic line rather than failing.

use crate::rules::Rules;
use civ_sim::protocol::{AttackTarget, ConstructionRef, Demand};
use civ_sim::{CityView, EventView, PlayerView, PresentationEvent as E, UnitView};

/// One line of the notification log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    /// Where the camera goes when the line is picked.
    pub tile: Option<u16>,
    pub kind: NoteKind,
}

/// How a line reads at a glance: its colour and its priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteKind {
    /// The player must act.
    Decision,
    Good,
    Bad,
    Info,
}

macro_rules! key {
    ($r:expr) => {
        ($r.id, $r.r#gen)
    };
}

/// A payload enum's wire name (`"religionFounded"`), for the ones civ-sim does not export.
macro_rules! wire_name {
    ($v:expr) => {
        serde_json::to_value($v).ok().and_then(|v| v.as_str().map(str::to_string))
    };
}

/// The log line for one event as `v`'s player saw it, or None for what the map shows on its own.
pub fn describe(ev: &EventView, v: &PlayerView, rules: &Rules) -> Option<Note> {
    use NoteKind::*;
    let w = Words { v, rules };
    let me = v.me;
    match &ev.event {
        // The map walks every step.
        E::UnitMoved { .. } => None,
        E::CanPromote { unit } => {
            let u = w.unit(key!(unit));
            let kind = u.and_then(|u| w.unit_type(u.base)).unwrap_or("a unit");
            note(format!("Choose a promotion for {kind}"), u.map(|u| u.tile), Decision)
        }
        // Another nation's unit changing in sight (the replay sends these) is its piece's to show.
        E::Promoted { unit, .. } | E::Upgraded { unit, .. } if w.owner_of(key!(unit)) != Some(me) => None,
        E::Promoted { unit, at, promotion } => {
            let text = format!("{} earned {}", w.unit_phrase(key!(unit), "your unit"), w.promotion(*promotion));
            note(text, Some(*at), Good)
        }
        // The piece shows a fortified unit; the viewer's own fortify on its orders.
        E::Fortified { .. } => None,
        E::Upgraded { at, from, to, .. } => {
            let old = w.unit_type(*from).unwrap_or("unit");
            let text = match w.unit_type(*to) {
                Some(new) => format!("Your {old} was upgraded to {}", a(new)),
                None => format!("Your {old} was upgraded"),
            };
            note(text, Some(*at), Good)
        }
        E::CityFounded { city, by } => {
            let text = format!("{} founded {}", w.civ(*by), w.city_name(key!(city)));
            note(text, w.city_tile(key!(city)), if *by == me { Good } else { Info })
        }
        E::FaithArose { religion, name, city } => {
            let name = if name.is_empty() { w.religion(*religion) } else { name.clone() };
            match city {
                Some(c) => note(format!("{name} arose in {}", w.city_name(key!(c))), w.city_tile(key!(c)), Good),
                None => note(format!("{name} arose"), None, Info),
            }
        }
        E::StateFaithAdopted { religion, name } => {
            let name = if name.is_empty() { w.religion(*religion) } else { name.clone() };
            note(format!("You adopted {name} as your state faith"), None, Good)
        }
        E::CityRenamed { city, from, to } => note(format!("{from} is now called {to}"), w.city_tile(key!(city)), Info),
        E::CityCaptured { city, by } => {
            let (name, tile) = (w.city_name(key!(city)), w.city_tile(key!(city)));
            if *by == me {
                let pending = v.pending_captures.iter().flatten().any(|p| key!(p.city) == key!(city));
                if pending {
                    note(format!("You captured {name}; decide its fate"), tile, Decision)
                } else {
                    note(format!("You captured {name}"), tile, Good)
                }
            } else {
                // The engine tells a city's former owner, and in the replay anyone who saw it fall. Only a nation
                // at war with the viewer can have taken one of the viewer's cities.
                let kind = if w.hostile(*by) { Bad } else { Info };
                note(format!("{} captured {name}", w.civ(*by)), tile, kind)
            }
        }
        // Growth and starvation arrive alike: the event carries the new size, not which way it went.
        E::CityPopulation { city, population } => {
            let text = format!("{} population is now {population}", possessive(&w.city_name(key!(city))));
            note(text, w.city_tile(key!(city)), Info)
        }
        E::TechLearned { tech } => note(format!("You learned {}", w.tech(*tech)), None, Good),
        // The reward's id names a rule the client's tables do not carry.
        E::RuinsExplored { tile, .. } => note("You explored ancient ruins", Some(*tile), Good),
        E::RuinsCleared { tile } => note("You cleared ancient ruins and found nothing", Some(*tile), Info),
        E::NaturalWonderDiscovered { tile, wonder, first } => {
            let name = w.terrain(*wonder);
            let text = if *first { format!("You were the first to discover {name}") } else { format!("You discovered {name}") };
            note(text, Some(*tile), Good)
        }
        E::ReligiousUnitActed { unit, action } => {
            let u = w.unit(key!(unit));
            let who = w.unit_phrase(key!(unit), "your religious unit");
            let carried = u.and_then(|u| u.own.as_ref()).and_then(|o| o.religion).map_or_else(|| "its religion".to_string(), |r| w.religion(r));
            let (did, kind) = match wire_name!(action).as_deref() {
                Some("founded") => ("founded a religion".to_string(), Good),
                Some("enhanced") => ("enhanced your religion".to_string(), Good),
                Some("spread") => (format!("spread {carried}"), Good),
                Some("heresyRemoved") => ("removed heresy".to_string(), Good),
                Some("holySiteConstructed") => ("built a holy site".to_string(), Good),
                Some("attrition") => ("lost strength to attrition".to_string(), Bad),
                // An action this client does not name yet.
                _ => ("acted".to_string(), Info),
            };
            note(format!("{who} {did}"), u.map(|u| u.tile), kind)
        }
        E::ProphetBorn { unit } => {
            let u = w.unit(key!(unit));
            let kind = u.and_then(|u| w.unit_type(u.base)).unwrap_or("sage");
            note(format!("{} was born", a(kind)), u.map(|u| u.tile), Good)
        }
        // Neither purchase says what was bought.
        E::FaithPurchased { city, faith } => {
            note(format!("You spent {faith} faith in {}", w.city_name(key!(city))), w.city_tile(key!(city)), Info)
        }
        E::Purchased { city, gold } => {
            note(format!("You spent {gold} gold in {}", w.city_name(key!(city))), w.city_tile(key!(city)), Info)
        }
        E::CivsMet { other } => note(format!("You met {}", w.civ(*other)), None, Info),
        E::WarDeclared { aggressor, against } => {
            let kind = if *against == me { Bad } else { Info };
            note(format!("{} declared war on {}", w.civ(*aggressor), w.civ(*against)), None, kind)
        }
        E::PeaceMade { other } => note(format!("You made peace with {}", w.civ(*other)), None, Good),
        E::Victory { civ, condition } => {
            let text = match w.victory(*condition) {
                Some(how) => format!("{} won {} victory", w.civ(*civ), a(how)),
                None => format!("{} won the game", w.civ(*civ)),
            };
            note(text, None, if *civ == me { Good } else { Bad })
        }
        E::PartAssembled { part, total } => {
            let name = w.unit_type(*part).unwrap_or("a part");
            let text = match v.endgame.spaceship_parts.iter().find(|p| p.part == *part) {
                Some(p) => format!("You assembled {name}: {total} of {}", p.needed),
                None => format!("You assembled {name}: {total} so far"),
            };
            note(text, None, Good)
        }
        E::VotingOpened { .. } => {
            if v.endgame.voting.as_ref().is_some_and(|x| x.may_vote && !x.has_voted) {
                note("Diplomatic voting opened; cast your vote", None, Decision)
            } else {
                note("Diplomatic voting opened", None, Info)
            }
        }
        E::VoteResolved { result } => match result.winner {
            Some(winner) => {
                let votes = result.votes.iter().find(|t| t.candidate == winner).map(|t| t.votes);
                let text = match votes {
                    Some(n) => format!("{} won the diplomatic vote with {n} votes, {} needed", w.civ(winner), result.needed),
                    None => format!("{} won the diplomatic vote", w.civ(winner)),
                };
                note(text, None, if winner == me { Good } else { Info })
            }
            None => note(format!("No one won the diplomatic vote; {} votes were needed", result.needed), None, Info),
        },
        E::DefensivePactSigned { civ } => note(format!("You signed a defensive pact with {}", w.civ(*civ)), None, Good),
        E::FriendshipSigned { other } => {
            note(format!("You signed a treaty of friendship with {}", w.civ(*other)), None, Good)
        }
        E::ResearchAgreementSigned { other } => {
            note(format!("You began a scholarly exchange with {}", w.civ(*other)), None, Good)
        }
        E::ResearchAgreementCompleted { other } => {
            note(format!("Your scholarly exchange with {} finished", w.civ(*other)), None, Good)
        }
        E::CoupResolved { city_state, succeeded } => {
            let cs = w.city_state(*city_state);
            if *succeeded {
                note(format!("Your coup in {cs} succeeded"), None, Good)
            } else {
                note(format!("Your coup in {cs} failed"), None, Bad)
            }
        }
        E::TechStolen { technology, other, as_thief } => {
            let tech = w.tech(*technology);
            match (as_thief, other) {
                (true, Some(victim)) => note(format!("Your spy stole {tech} from {}", w.civ(*victim)), None, Good),
                (true, None) => note(format!("Your spy stole {tech}"), None, Good),
                (false, Some(thief)) => note(format!("{} stole {tech} from you", w.civ(*thief)), None, Bad),
                (false, None) => note(format!("A spy stole {tech} from you"), None, Bad),
            }
        }
        E::SpyCaught { city } => match w.city(key!(city)) {
            Some(c) => note(format!("Your spy was caught in {}", c.name), Some(c.centre), Bad),
            None => note("Your spy was caught", None, Bad),
        },
        // The engine says how much, not what hurt it.
        E::UnitDamaged { unit, damage, died } => {
            let tile = w.unit(key!(unit)).map(|u| u.tile);
            if *died {
                let who = w.unit_phrase(key!(unit), "one of your units");
                note(format!("{who} took {damage} damage and was lost"), tile, Bad)
            } else {
                note(format!("{} took {damage} damage", w.unit_phrase(key!(unit), "your unit")), tile, Bad)
            }
        }
        E::Attacked { viewer_attacked, target, at, attacker_died, defender_died, damage_to_defender, .. } => {
            // A unit that fell is gone from the view, so the engine's own line (worded for this player, with both
            // sides named as they stood) is the text. It says "your" in every fight of the viewer's except one its
            // city defends, and cities stay in the view to be recognised.
            let defender = w.side_owner(target);
            let viewers = *viewer_attacked || defender == Some(me) || (defender.is_none() && says_you(&ev.text));
            if !viewers {
                return None;
            }
            let kind = match (viewer_attacked, attacker_died, defender_died) {
                (true, _, true) | (false, true, false) => Good,
                (true, true, false) | (false, _, true) => Bad,
                (true, false, false) => Info,
                (false, false, false) => {
                    if *damage_to_defender > 0 {
                        Bad
                    } else {
                        Info
                    }
                }
            };
            let text = match ev.text.trim() {
                "" if *viewer_attacked => "You attacked".to_string(),
                "" => "You were attacked".to_string(),
                text => text.to_string(),
            };
            note(text, Some(*at), kind)
        }
        E::Captured { unit, at, from, to, becomes, .. } => {
            // The unit keeps its id; `becomes` is its type now when that changed, so its old type is not known.
            let now = match becomes {
                Some(b) => w.unit_type(*b),
                None => w.unit(key!(unit)).and_then(|u| w.unit_type(u.base)),
            };
            let what = now.map_or_else(|| "a unit".to_string(), a);
            if *to == me {
                note(format!("You captured {what} from {}", w.civ(*from)), Some(*at), Good)
            } else if *from == me {
                let yours = match (becomes, now) {
                    (None, Some(kind)) => format!("your {kind}"),
                    _ => "one of your units".to_string(),
                };
                note(format!("{} captured {yours}", w.civ(*to)), Some(*at), Bad)
            } else {
                note(format!("{} captured {what} from {}", w.civ(*to), w.civ(*from)), Some(*at), Info)
            }
        }
        E::Withdrew { unit, to, attacker, .. } => {
            let striker = attacker.and_then(|a| w.unit(key!(a)));
            if w.owner_of(key!(unit)) == Some(me) {
                let from_whom = attacker.map_or_else(|| "an attack".to_string(), |a| w.unit_phrase(key!(a), "an attack"));
                note(format!("{} withdrew from {from_whom}", w.unit_phrase(key!(unit), "your unit")), Some(*to), Good)
            } else if striker.is_some_and(|s| s.owner == me) {
                let yours = attacker.map_or_else(|| "your unit".to_string(), |a| w.unit_phrase(key!(a), "your unit"));
                note(format!("{} withdrew before {yours} could strike", w.unit_phrase(key!(unit), "a unit")), Some(*to), Info)
            } else {
                // Between others: the step also arrives as a unitMoved, which the map walks.
                None
            }
        }
        E::Pillaged { unit, tile, gold, .. } => {
            let pillager = unit.and_then(|u| w.unit(key!(u)));
            let who = match unit {
                Some(u) if pillager.is_some() => w.unit_phrase(key!(u), "a unit"),
                _ => "someone".to_string(),
            };
            let land = v.tiles.iter().find(|t| t.id == *tile).and_then(|t| t.owner);
            if pillager.is_some_and(|p| p.owner == me) {
                if *gold > 0 {
                    note(format!("{who} pillaged a tile and took {gold} gold"), Some(*tile), Good)
                } else {
                    note(format!("{who} pillaged a tile"), Some(*tile), Info)
                }
            } else if land == Some(me) {
                note(format!("{who} pillaged your land"), Some(*tile), Bad)
            } else {
                note(format!("{who} pillaged a tile"), Some(*tile), Info)
            }
        }
        E::Nuked { unit, at, tiles, destroyed, cities } => {
            // The weapon is named only when it was the viewer's, and only the viewer's losses are listed.
            let mine = unit.is_some();
            let subject = match unit.and_then(|u| w.unit(key!(u))).and_then(|u| w.unit_type(u.base)) {
                Some(kind) => format!("your {kind}"),
                None if mine => "your nuclear strike".to_string(),
                None => "a nuclear strike".to_string(),
            };
            let hit: Vec<String> = cities.iter().map(|c| w.city_name(key!(c))).collect();
            let own_hit = cities.iter().any(|c| w.city(key!(c)).is_some_and(|c| c.owner == me));
            let mut did = Vec::new();
            if !hit.is_empty() {
                did.push(format!("hit {}", and_list(&hit)));
            }
            match destroyed.len() {
                0 => {}
                1 => did.push("destroyed one of your units".to_string()),
                n => did.push(format!("destroyed {n} of your units")),
            }
            let text = if did.is_empty() { format!("{subject} landed") } else { format!("{subject} {}", and_list(&did)) };
            let kind = if mine && destroyed.is_empty() && !own_hit { Info } else { Bad };
            note(text, at.or_else(|| tiles.first().copied()), kind)
        }
        E::CampCleared { tile, gold, recruited } => {
            let mut did = vec!["cleared a raider camp".to_string()];
            if *gold > 0 {
                did.push(format!("took {gold} gold"));
            }
            if let Some(r) = recruited {
                let kind = w.unit(key!(r)).and_then(|u| w.unit_type(u.base));
                did.push(format!("recruited {}", kind.map_or_else(|| "a unit".to_string(), a)));
            }
            note(format!("You {}", and_list(&did)), Some(*tile), Good)
        }
        E::RebelsSpawned { tile, unit } => {
            let at = tile.or_else(|| unit.and_then(|u| w.unit(key!(u))).map(|u| u.tile));
            note("Rebels rose against you", at, Bad)
        }
        E::BarbarianCampAppeared { tile } => note("A raider camp appeared", Some(*tile), Info),
        E::DemandMade { by, of, demand } => {
            let asker = if *by == me { "you" } else { "them" };
            if *of == me && v.demands.iter().flatten().any(|d| d.by == *by && d.demand == *demand) {
                let text = format!("Answer {} demand that you not {}", w.whose(*by), stop(*demand, asker));
                note(text, None, Decision)
            } else {
                let text = format!("{} demanded that {} not {}", w.civ(*by), w.civ(*of), stop(*demand, asker));
                note(text, None, Info)
            }
        }
        E::DemandAnswered { by, to, demand, agreed } => {
            let (text, kind) = if *agreed {
                let object = if *to == me { "you".to_string() } else { w.civ(*to) };
                (format!("{} agreed not to {}", w.civ(*by), stop(*demand, &object)), if *to == me { Good } else { Info })
            } else {
                let object = if *to == me { "you" } else { "them" };
                let text = format!("{} refused {} demand not to {}", w.civ(*by), w.whose(*to), stop(*demand, object));
                (text, if *to == me { Bad } else { Info })
            };
            note(text, None, kind)
        }
        E::PromiseBroken { by, to, demand } => note(promise(&w, *by, *to, *demand, "broke"), None, Bad),
        E::PromiseKept { by, to, demand } => note(promise(&w, *by, *to, *demand, "kept"), None, Good),
        E::GreatPersonBorn { name, unit } => {
            let kind = if name.is_empty() { "luminary" } else { name.as_str() };
            note(format!("{} was born", a(kind)), unit.and_then(|u| w.unit(key!(u))).map(|u| u.tile), Good)
        }
        E::GreatPersonActed { outcome } => {
            let o = serde_json::to_value(outcome).unwrap_or_default();
            let num = |field: &str| o.get(field).and_then(serde_json::Value::as_i64);
            match o.get("type").and_then(serde_json::Value::as_str) {
                Some("science") => {
                    let mut text = format!("Your luminary added {} science", num("amount").unwrap_or(0));
                    if let Some(tech) = num("learned") {
                        text += &format!(" and finished {}", w.tech(tech as u32));
                    }
                    note(text, None, Good)
                }
                Some("production") => {
                    let city = o.get("city").and_then(|c| Some((c.get("id")?.as_u64()? as u32, c.get("gen")?.as_u64()? as u32)));
                    let name = city.map_or_else(|| "a city".to_string(), |k| w.city_name(k));
                    let text = format!("Your luminary added {} production in {name}", num("hammers").unwrap_or(0));
                    note(text, city.and_then(|k| w.city_tile(k)), Good)
                }
                Some("tradeMission") => {
                    let cs = num("cityState").map_or_else(|| "a city-state".to_string(), |c| w.city_state(c as u8));
                    let (gold, influence) = (num("gold").unwrap_or(0), num("influence").unwrap_or(0));
                    note(format!("Your trade mission to {cs} earned {gold} gold and {influence} influence"), None, Good)
                }
                Some("goldenAge") => {
                    note(format!("Your luminary brought a golden age of {} turns", num("turns").unwrap_or(0)), None, Good)
                }
                Some("constructed") => {
                    let built = num("improvement").map_or_else(|| "an improvement".to_string(), |i| a(&w.improvement(i as u32)));
                    note(format!("Your luminary built {built}"), num("tile").map(|t| t as u16), Good)
                }
                // An outcome this client does not name yet.
                _ => note("Your luminary was spent", None, Good),
            }
        }
        E::CityCompleted { city, construction } => {
            let name = w.city_name(key!(city));
            let text = match construction {
                ConstructionRef::Building { id } => format!("{name} completed {}", w.building(*id)),
                ConstructionRef::Unit { id } => {
                    format!("{name} trained {}", w.unit_type(*id).map_or_else(|| "a unit".to_string(), a))
                }
            };
            note(text, w.city_tile(key!(city)), Good)
        }
        E::QuestOffered { city_state, name, .. } => {
            let cs = w.city_state(*city_state);
            let text = if name.is_empty() { format!("{cs} asked a favour") } else { format!("{cs} asked a favour: {name}") };
            note(text, None, Info)
        }
        E::QuestCompleted { city_state, name, influence, .. } => {
            let what = if name.is_empty() { "a favour" } else { name.as_str() };
            let text = format!("You completed {what} for {} and gained {influence} influence", w.city_state(*city_state));
            note(text, None, Good)
        }
        E::QuestExpired { city_state, name, .. } => {
            let what = if name.is_empty() { "a favour" } else { name.as_str() };
            note(format!("You did not complete {what} for {} in time", w.city_state(*city_state)), None, Bad)
        }
        E::CityStateRelationship { city_state, from, to } => {
            let cs = w.city_state(*city_state);
            let text = match to.as_str() {
                "Ally" => format!("{cs} became your ally"),
                "Friend" => format!("{cs} became your friend"),
                "Neutral" => format!("{cs} became neutral toward you"),
                "Enemy" => format!("{cs} became your enemy"),
                "Unforgivable" => format!("{cs} became unforgiving toward you"),
                // A standing this client does not name yet.
                _ => format!("{cs} changed its standing with you"),
            };
            let kind = match (standing(from), standing(to)) {
                (Some(a), Some(b)) if b > a => Good,
                (Some(a), Some(b)) if b < a => Bad,
                _ => Info,
            };
            note(text, None, kind)
        }
        E::GoldGifted { city_state, gold, influence } => {
            note(format!("You gave {} {gold} gold for {influence} influence", w.city_state(*city_state)), None, Info)
        }
        E::TributePaid { city_state, gold, worker } => {
            let cs = w.city_state(*city_state);
            if *worker {
                note(format!("{cs} gave you a worker as tribute"), None, Good)
            } else {
                note(format!("{cs} paid you {gold} gold in tribute"), None, Good)
            }
        }
        E::ElectionRigged { city_state } => {
            note(format!("Your spy carried the election in {}", w.city_state(*city_state)), None, Good)
        }
        E::Denounced { by, target } => {
            let kind = if *target == me { Bad } else { Info };
            note(format!("{} denounced {}", w.civ(*by), w.civ(*target)), None, kind)
        }
        E::CityConverted { city, religion } => {
            let text = format!("{} converted to {}", w.city_name(key!(city)), w.religion(*religion));
            note(text, w.city_tile(key!(city)), Info)
        }
        E::WeLoveTheKingDay { city, resource } => {
            let name = w.city_name(key!(city));
            let text = match w.rules.resource(*resource) {
                Some((luxury, _)) => format!("{name} began Feast Days for its {luxury}"),
                None => format!("{name} began Feast Days"),
            };
            note(text, w.city_tile(key!(city)), Good)
        }
        E::WeLoveTheKingDayEnded { city } => {
            note(format!("{} Feast Days ended", possessive(&w.city_name(key!(city)))), w.city_tile(key!(city)), Info)
        }
        E::BordersExpanded { city, tile } => {
            note(format!("{} borders grew", possessive(&w.city_name(key!(city)))), Some(*tile), Info)
        }
        E::CityBurned { city, population } => {
            let text = format!("{} burned down to {population}", w.city_name(key!(city)));
            note(text, w.city_tile(key!(city)), Info)
        }
        // Its id no longer resolves, so it cannot be named.
        E::CityRazed { .. } => note("A city you were razing burned to the ground", None, Info),
        E::CityStateGift { city_state, unit } => {
            let u = w.unit(key!(unit));
            let gift = u.and_then(|u| w.unit_type(u.base)).map_or_else(|| "a unit".to_string(), a);
            note(format!("{} gave you {gift}", w.city_state(*city_state)), u.map(|u| u.tile), Good)
        }
        E::CityStateMarried { city_state, gold } => {
            note(format!("{} joined your nation by marriage for {gold} gold", w.city_state(*city_state)), None, Good)
        }
        E::Notice { notice } => match wire_name!(notice).as_deref() {
            Some("religionFounded") => note("You founded a religion", None, Good),
            Some("religionEnhanced") => note("You enhanced your religion", None, Good),
            Some("pantheonFounded") => note("You founded a pantheon", None, Good),
            Some("civDefeated") => note("Your nation was defeated", None, Bad),
            Some("goldenAgeStarted") => note("Your golden age began", None, Good),
            Some("goldenAgeEnded") => note("Your golden age ended", None, Info),
            // A notice this client does not name yet.
            _ => note("Something changed in your nation", None, Info),
        },
    }
}

fn note(text: impl Into<String>, tile: Option<u16>, kind: NoteKind) -> Option<Note> {
    Some(Note { text: sentence(&text.into()), tile, kind })
}


/// What a demand asks its target to stop doing, to `object`: "spy on them".
fn stop(demand: Demand, object: &str) -> String {
    match demand {
        Demand::DontSpyOnUs => format!("spy on {object}"),
        Demand::DoNotSpreadReligion => format!("spread religion to {object}"),
        Demand::DoNotSettleNearUs => format!("settle near {object}"),
        Demand::DoNotAttackUs => format!("attack {object}"),
    }
}

/// "England broke its promise not to spy on you", "You kept your promise to England not to spy on them".
fn promise(w: &Words, by: u8, to: u8, demand: Demand, did: &str) -> String {
    if to == w.v.me {
        format!("{} {did} its promise not to {}", w.civ(by), stop(demand, "you"))
    } else {
        let its = if by == w.v.me { "your" } else { "its" };
        format!("{} {did} {its} promise to {} not to {}", w.civ(by), w.civ(to), stop(demand, "them"))
    }
}

/// A city-state's standing toward the viewer, worst first (`CityStateRelationsView.relationship`'s names).
fn standing(name: &str) -> Option<u8> {
    ["Unforgivable", "Enemy", "Neutral", "Friend", "Ally"].iter().position(|s| *s == name).map(|i| i as u8)
}

/// The engine words the viewer's fights with "your" (`battle_text`).
fn says_you(text: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric()).any(|word| word.eq_ignore_ascii_case("your"))
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn a(name: &str) -> String {
    let an = name.chars().next().is_some_and(|c| "AEIOUaeiou".contains(c));
    format!("{} {name}", if an { "an" } else { "a" })
}

fn possessive(name: &str) -> String {
    if name.ends_with('s') { format!("{name}'") } else { format!("{name}'s") }
}

fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Names as the viewer knows them.
struct Words<'a> {
    v: &'a PlayerView,
    rules: &'a Rules,
}

impl Words<'_> {
    fn unit(&self, k: (u32, u32)) -> Option<&UnitView> {
        self.v.units.iter().find(|u| (u.id, u.r#gen) == k)
    }

    fn owner_of(&self, k: (u32, u32)) -> Option<u8> {
        self.unit(k).map(|u| u.owner)
    }

    /// "your Warrior", "England's Warrior", or `fallback` when the unit is out of sight or gone.
    fn unit_phrase(&self, k: (u32, u32), fallback: &str) -> String {
        match self.unit(k) {
            Some(u) => format!("{} {}", self.whose(u.owner), self.unit_type(u.base).unwrap_or("unit")),
            None => fallback.to_string(),
        }
    }

    fn city(&self, k: (u32, u32)) -> Option<&CityView> {
        self.v.cities.iter().find(|c| (c.id, c.r#gen) == k)
    }

    fn city_name(&self, k: (u32, u32)) -> String {
        self.city(k).map_or_else(|| "a city".to_string(), |c| c.name.clone())
    }

    fn city_tile(&self, k: (u32, u32)) -> Option<u16> {
        self.city(k).map(|c| c.centre)
    }

    fn side_owner(&self, side: &AttackTarget) -> Option<u8> {
        match side {
            AttackTarget::Unit { unit } => self.owner_of(key!(unit)),
            AttackTarget::City { city } => self.city(key!(city)).map(|c| c.owner),
        }
    }

    /// The nation's name, "the raiders" for the raiders.
    fn nation(&self, civ: u8) -> Option<String> {
        let nation = self.v.civs.iter().find(|c| c.id == civ)?.nation;
        let row = self.rules.tables.nations.iter().find(|n| n.id == nation)?;
        Some(if row.barbarian == Some(true) { "the raiders".to_string() } else { row.name.clone() })
    }

    /// A nation as the subject or object of a line: "you" for the viewer.
    fn civ(&self, civ: u8) -> String {
        if civ == self.v.me {
            return "you".to_string();
        }
        self.nation(civ).unwrap_or_else(|| "a nation".to_string())
    }

    fn whose(&self, civ: u8) -> String {
        if civ == self.v.me { "your".to_string() } else { possessive(&self.civ(civ)) }
    }

    fn city_state(&self, civ: u8) -> String {
        self.nation(civ).unwrap_or_else(|| "a city-state".to_string())
    }

    fn hostile(&self, civ: u8) -> bool {
        let raiders = self.v.civs.iter().find(|c| c.id == civ).is_some_and(|c| {
            self.rules.tables.nations.iter().any(|n| n.id == c.nation && n.barbarian == Some(true))
        });
        raiders || self.v.relations.iter().any(|r| r.civ == civ && r.at_war)
    }

    /// A religion by its key: a faith's name, or its founder's.
    fn religion(&self, key: u8) -> String {
        let faith = self.rules.tables.faiths.iter().flatten().find(|f| f.key == key).map(|f| f.name.clone());
        faith.or_else(|| match key {
            k if k == self.v.me => Some("your religion".to_string()),
            k if k < 128 => self.nation(k).map(|n| format!("{} religion", possessive(&n))),
            _ => None,
        })
        .unwrap_or_else(|| "a religion".to_string())
    }

    fn unit_type(&self, id: u32) -> Option<&str> {
        self.rules.tables.units.iter().find(|u| u.id == id).map(|u| u.name.as_str())
    }

    fn tech(&self, id: u32) -> String {
        let row = self.rules.tables.technologies.iter().find(|t| t.id == id);
        row.map_or_else(|| "a technology".to_string(), |t| t.name.clone())
    }

    fn building(&self, id: u32) -> String {
        let row = self.rules.tables.buildings.iter().find(|b| b.id == id);
        row.map_or_else(|| "a building".to_string(), |b| b.name.clone())
    }

    fn promotion(&self, id: u32) -> String {
        let row = self.rules.tables.promotions.iter().find(|p| p.id == id);
        row.map_or_else(|| "a promotion".to_string(), |p| p.name.clone())
    }

    fn improvement(&self, id: u32) -> String {
        let row = self.rules.tables.improvements.iter().find(|i| i.id == id);
        row.map_or_else(|| "improvement".to_string(), |i| i.name.clone())
    }

    fn terrain(&self, id: u32) -> String {
        let row = self.rules.tables.terrains.iter().find(|t| t.id == id);
        row.map_or_else(|| "a natural wonder".to_string(), |t| t.name.clone())
    }

    fn victory(&self, id: u32) -> Option<&str> {
        self.rules.tables.victories.iter().find(|x| x.id == id).map(|x| x.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use civ_sim::protocol::{Action, CommandRequest, CommandStatus, EntityRef, GameUpdate};
    use civ_sim::session::Session;
    use civ_sim::Settings;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn session(seed: u64) -> (Session, Rules) {
        let settings = Settings { seed, width: 24, height: 16, majors: 2, city_states: 1, ..Settings::default() };
        let session = Session::new(settings, "notes".into(), vec![0]).unwrap();
        let rules = Rules::new(session.content_hash(), civ_sim::tables(&civ_sim::content::load().unwrap()));
        (session, rules)
    }

    /// An event as it crosses the wire; the view's entity refs are not nameable outside civ-sim.
    fn event(event: serde_json::Value) -> EventView {
        serde_json::from_value(json!({ "kind": "test", "text": "", "event": event })).unwrap()
    }

    /// A fresh view with one city of the viewer's, Roma, added; and another major's id and name.
    fn fixture() -> (PlayerView, Rules, u8, String) {
        let (session, rules) = session(3);
        let mut v = session.current_update(0).unwrap().view;
        let centre = v.units.iter().find(|u| u.owner == v.me).unwrap().tile;
        v.cities.push(CityView {
            id: 900,
            r#gen: 1,
            name: "Roma".into(),
            owner: v.me,
            centre,
            population: 4,
            tiles: vec![centre],
            queue: None,
            queue_refs: None,
            wonder_construction: None,
            food_stored: None,
            production_stored: None,
            production_overflow: None,
            production_progress: None,
            own: None,
            visible: true,
            observed_turn: v.turn,
        });
        // A nation met: any major's that is not the viewer's.
        let mine = v.civs.iter().find(|c| c.id == v.me).unwrap().nation;
        let nation = rules.tables.nations.iter().find(|n| n.id != mine && !n.city_state && n.barbarian != Some(true)).unwrap();
        let (nation, name) = (nation.id, nation.name.clone());
        let id = 40;
        v.civs.push(civ_sim::CivView {
            id,
            nation,
            leader: None,
            state_faith: None,
            city_state: false,
            alive: true,
            gold: None,
            science: None,
            culture: None,
            researching: None,
            techs: None,
            observed_turn: v.turn,
        });
        (v, rules, id, name)
    }

    fn roma() -> serde_json::Value {
        json!({ "id": 900, "gen": 1 })
    }

    #[test]
    fn each_kind_reads_as_the_player_would() {
        let (v, rules, other, england) = fixture();
        let me = v.me;
        let warrior = v.units.iter().find(|u| u.owner == me).unwrap();
        let kind = rules.tables.units.iter().find(|u| u.id == warrior.base).unwrap().name.clone();
        let centre = v.cities[0].centre;

        let promote = event(json!({ "type": "canPromote", "unit": { "id": warrior.id, "gen": warrior.r#gen } }));
        let want = Note { text: format!("Choose a promotion for {kind}"), tile: Some(warrior.tile), kind: NoteKind::Decision };
        assert_eq!(describe(&promote, &v, &rules), Some(want));

        let founded = event(json!({ "type": "cityFounded", "city": roma(), "by": me }));
        let want = Note { text: "You founded Roma".into(), tile: Some(centre), kind: NoteKind::Good };
        assert_eq!(describe(&founded, &v, &rules), Some(want));

        let war = event(json!({ "type": "warDeclared", "aggressor": other, "against": me }));
        let want = Note { text: format!("{england} declared war on you"), tile: None, kind: NoteKind::Bad };
        assert_eq!(describe(&war, &v, &rules), Some(want));

        let grew = event(json!({ "type": "cityPopulation", "city": roma(), "population": 5 }));
        let want = Note { text: "Roma's population is now 5".into(), tile: Some(centre), kind: NoteKind::Info };
        assert_eq!(describe(&grew, &v, &rules), Some(want));

        let step = event(json!({
            "type": "unitMoved", "unit": { "id": warrior.id, "gen": warrior.r#gen },
            "from": warrior.tile, "to": warrior.tile, "cost": 1.0, "embark": null, "zoc": false,
        }));
        assert_eq!(describe(&step, &v, &rules), None);
    }

    #[test]
    fn what_the_view_cannot_name_reads_as_a_plain_noun() {
        let (v, rules, _, _) = fixture();
        let lost = event(json!({ "type": "cityCaptured", "city": { "id": 77, "gen": 0 }, "by": 250 }));
        let note = describe(&lost, &v, &rules).unwrap();
        assert_eq!((note.text.as_str(), note.tile), ("A nation captured a city", None));
        let tech = event(json!({ "type": "techLearned", "tech": 99999 }));
        assert_eq!(describe(&tech, &v, &rules).unwrap().text, "You learned a technology");
        let died = event(json!({ "type": "unitDamaged", "unit": { "id": 5000, "gen": 0 }, "damage": 15, "died": true }));
        assert_eq!(describe(&died, &v, &rules).unwrap().text, "One of your units took 15 damage and was lost");
    }

    #[test]
    fn demands_waiting_for_an_answer_are_decisions() {
        let (mut v, rules, other, england) = fixture();
        let made = event(json!({ "type": "demandMade", "by": other, "of": v.me, "demand": "dontSpyOnUs" }));
        let note = describe(&made, &v, &rules).unwrap();
        assert_eq!((note.text, note.kind), (format!("{england} demanded that you not spy on them"), NoteKind::Info));
        v.demands = serde_json::from_value(json!([{
            "by": other, "demand": "dontSpyOnUs", "promiseUntil": 40, "refusalDeclaresWar": false, "mayRefuse": true,
        }]))
        .unwrap();
        let note = describe(&made, &v, &rules).unwrap();
        let want = format!("Answer {england}'s demand that you not spy on them");
        assert_eq!((note.text, note.kind), (want, NoteKind::Decision));
    }

    /// The viewer's fights read as the engine words them; a fight between others is the map's to show.
    #[test]
    fn fights_are_the_viewers_or_the_maps() {
        let (v, rules, _, _) = fixture();
        let fight = |viewer_attacked: bool, text: &str| {
            serde_json::from_value::<EventView>(json!({ "kind": "Attacked", "text": text, "event": {
                "type": "attacked", "viewerAttacked": viewer_attacked,
                "attacker": { "type": "unit", "unit": { "id": 6000, "gen": 0 } },
                "target": { "type": "unit", "unit": { "id": 6001, "gen": 0 } },
                "from": 1, "at": 2, "ranged": false, "damageToAttacker": 0, "damageToDefender": 100,
                "attackerDied": false, "defenderDied": true,
                "attackerMods": [], "defenderMods": [], "flankers": [], "general": null,
            }}))
            .unwrap()
        };
        let won = describe(&fight(true, "Your Warrior destroyed a Brute"), &v, &rules).unwrap();
        assert_eq!((won.text.as_str(), won.tile, won.kind), ("Your Warrior destroyed a Brute", Some(2), NoteKind::Good));
        let lost = describe(&fight(false, "A Brute destroyed your Warrior"), &v, &rules).unwrap();
        assert_eq!(lost.kind, NoteKind::Bad);
        assert_eq!(describe(&fight(false, "English Warrior attacked French"), &v, &rules), None);
    }

    /// Payloads civ-sim does not export are read by wire name: every name here must still be one the engine sends.
    #[test]
    fn unexported_payloads_are_read_by_their_wire_names() {
        let (v, rules, _, _) = fixture();
        let generic = ["Something changed in your nation", "Your luminary was spent"];
        let notices = ["religionFounded", "religionEnhanced", "pantheonFounded", "civDefeated", "goldenAgeStarted", "goldenAgeEnded"];
        for notice in notices {
            let note = describe(&event(json!({ "type": "notice", "notice": notice })), &v, &rules).unwrap();
            assert!(!generic.contains(&note.text.as_str()), "{notice}");
        }
        let unit = json!({ "id": 5000, "gen": 0 });
        for action in ["founded", "enhanced", "spread", "heresyRemoved", "holySiteConstructed", "attrition"] {
            let acted = event(json!({ "type": "religiousUnitActed", "unit": unit, "action": action }));
            assert!(!describe(&acted, &v, &rules).unwrap().text.ends_with(" acted"), "{action}");
        }
        let outcomes = [
            json!({ "type": "science", "amount": 300, "learned": 0 }),
            json!({ "type": "production", "city": roma(), "hammers": 200 }),
            json!({ "type": "tradeMission", "cityState": 1, "gold": 300, "influence": 30 }),
            json!({ "type": "goldenAge", "turns": 8 }),
            json!({ "type": "constructed", "tile": 3, "improvement": 0 }),
        ];
        for outcome in outcomes {
            let note = describe(&event(json!({ "type": "greatPersonActed", "outcome": outcome })), &v, &rules).unwrap();
            assert!(!generic.contains(&note.text.as_str()), "{outcome}");
        }
    }

    fn clean(note: &Note) -> Result<(), String> {
        let t = &note.text;
        let leftovers = ["{", "}", "Some(", "None", "EntityRef", "::", "?", "!", "  ", "\"", "_"];
        if t.trim().is_empty() || t.trim() != t {
            return Err(format!("blank or padded: {t:?}"));
        }
        if !t.chars().next().is_some_and(char::is_uppercase) {
            return Err(format!("not sentence case: {t:?}"));
        }
        match leftovers.iter().find(|l| t.contains(*l)) {
            Some(l) => Err(format!("{l:?} left in {t:?}")),
            None => Ok(()),
        }
    }

    type Tally = BTreeMap<String, (usize, usize)>;

    /// What the events of one update say, checked against the view they came with and against a view that knows
    /// no units, cities or nations; tallied by engine kind as (seen, noted).
    fn read(update: &GameUpdate, rules: &Rules, kinds: &mut Tally) {
        let mut bare = update.view.clone();
        bare.units.clear();
        bare.cities.clear();
        bare.civs.clear();
        let replayed = update.replay.iter().flatten().flat_map(|b| &b.events);
        for ev in replayed.chain(update.events.iter().map(|e| &e.event)) {
            let entry = kinds.entry(ev.kind.clone()).or_default();
            entry.0 += 1;
            for view in [&update.view, &bare] {
                if let Some(Err(why)) = describe(ev, view, rules).map(|n| clean(&n)) {
                    panic!("{why}, from {:?}", ev.event);
                }
            }
            entry.1 += usize::from(describe(ev, &update.view, rules).is_some());
        }
    }

    /// Seat 0 of a seeded match founds a city, sends its other units exploring, keeps a warrior in the city's
    /// queue and research going, and finishes `rounds` turns; every update it gets is read.
    fn play(seed: u64, rounds: u32, kinds: &mut Tally) {
        let (mut session, rules) = session(seed);
        let warrior = rules.tables.units.iter().find(|u| u.name == "Warrior").map(|u| u.id);
        let mut n = 0;
        let mut send = |session: &mut Session, action: Action, kinds: &mut Tally| {
            n += 1;
            let update = session.current_update(0).unwrap();
            let request = CommandRequest {
                command_id: format!("c{n}"),
                epoch: update.cursor.epoch.clone(),
                turn_token: update.turn_token.clone(),
                action,
            };
            let transition = session.stage_command(0, request).unwrap();
            for (_, update) in transition.updates.iter().filter(|(seat, _)| *seat == 0) {
                read(update, &rules, kinds);
            }
            let status = transition.receipt.result.clone();
            *session = transition.next;
            status
        };
        let entity = |id: u32, r#gen: u32| EntityRef { id, r#gen };
        for round in 0..rounds {
            let v = session.current_update(0).unwrap().view;
            if round == 0 {
                let mut founded = false;
                for u in v.units.iter().filter(|u| u.owner == v.me) {
                    let unit = entity(u.id, u.r#gen);
                    if !founded {
                        let found = Action::FoundCity { unit, name: "Roma".into() };
                        founded = send(&mut session, found, kinds) == CommandStatus::Applied;
                        if founded {
                            continue;
                        }
                    }
                    send(&mut session, Action::ExploreUnit { unit }, kinds);
                }
            }
            let idle = v.cities.iter().filter(|c| c.owner == v.me && c.queue_refs.as_ref().is_none_or(Vec::is_empty));
            for city in idle {
                if let Some(id) = warrior {
                    let queue = vec![ConstructionRef::Unit { id }];
                    send(&mut session, Action::SetProduction { city: entity(city.id, city.r#gen), queue }, kinds);
                }
            }
            if v.civs.iter().find(|c| c.id == v.me).is_some_and(|c| c.researching.is_none()) {
                let known = &v.knowledge.technologies;
                let next = rules
                    .tables
                    .technologies
                    .iter()
                    .find(|t| !known.contains(&t.id) && t.prerequisites.iter().all(|p| known.contains(p)));
                if let Some(t) = next {
                    send(&mut session, Action::SetResearch { technology: t.id }, kinds);
                }
            }
            send(&mut session, Action::FinishTurn, kinds);
            if session.finished() {
                break;
            }
        }
    }

    /// Three seeded matches of 60 rounds: every event of every update (the replay too) reads cleanly.
    #[test]
    fn every_event_of_a_played_match_reads_cleanly() {
        let mut kinds = Tally::new();
        for seed in [11, 12, 13] {
            play(seed, 60, &mut kinds);
        }
        let notes: usize = kinds.values().map(|k| k.1).sum();
        eprintln!("{notes} notes; events by kind (seen, noted): {kinds:?}");
        assert!(notes > 0, "the matches gave no notes");
        assert!(kinds.contains_key("CityFounded"), "the viewer's founding never arrived");
    }
}
