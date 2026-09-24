//! Fields civ.game adds to the Unciv ruleset schema.
//!
//! `rules/base` writes these; the frozen `gods-and-kings` and `vanilla`
//! fixtures never do, so every mechanic that reads them is switched off there
//! by the data's absence rather than by a flag. The generated structs carry
//! them as load-time fields (see `DERIVED` in `tools/generate_ruleset.py`),
//! filled here from the object still in hand.

use super::generated::Nation;
use crate::json::Json;
use crate::unique::Unique;

/// The eight regions a nation or city-state belongs to (docs/REBUILD.md).
pub const REGIONS: [&str; 8] =
    ["Europe", "WestAsia", "Africa", "SouthAsia", "EastAsia", "SoutheastAsia", "Steppe", "Americas"];

/// A nation's second leader, chosen at game setup. Their uniques replace the
/// nation's own, which are the default leader's ability; unique units and
/// buildings stay with the nation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AltLeader {
    pub leader_name: String,
    pub unique_name: Option<String>,
    pub uniques: Vec<Unique>,
    pub voice: Option<String>,
}

/// One step of a city's history: `from` becomes `to` once its owner reaches
/// `era`. Steps may chain (Byzantion, Constantinople, Istanbul).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CityRename {
    pub from: String,
    pub to: String,
    pub era: String,
}

fn text(j: &Json, key: &str) -> Option<String> {
    j.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn uniques(j: &Json) -> Vec<Unique> {
    j.get("uniques").map(|v| v.strings().iter().map(|s| Unique::parse(s)).collect()).unwrap_or_default()
}

/// The name the fixtures give their barbarians, which carry no flag.
pub const LEGACY_BARBARIANS: &str = "Barbarians";

/// Read civ.game's nation fields from the object `n` was parsed from.
pub fn read_nation(j: &Json, n: &mut Nation) {
    n.region = text(j, "region");
    n.voice = text(j, "voice");
    n.alt_leader = j.get("altLeader").and_then(|a| {
        Some(AltLeader {
            leader_name: text(a, "leaderName")?,
            unique_name: text(a, "uniqueName"),
            uniques: uniques(a),
            voice: text(a, "voice"),
        })
    });
    n.city_renames = j
        .get("cityRenames")
        .and_then(|v| v.as_array())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| Some(CityRename { from: text(r, "from")?, to: text(r, "to")?, era: text(r, "era")? }))
        .collect();
    n.is_barbarian = j.get("isBarbarian").and_then(|v| v.as_bool()).unwrap_or(n.name == LEGACY_BARBARIANS);
    n.wave = j.get("wave").and_then(|v| v.as_i32());
}

impl Nation {
    /// A nation a player or the AI may lead: not a city-state, not the
    /// barbarians, and with someone to lead it (which rules out Spectator).
    pub fn is_major(&self) -> bool {
        self.city_state_type.is_none() && !self.is_barbarian && self.leader_name.is_some()
    }

    /// Playable under a ruleset's `maxWave`: a nation of a later roster wave
    /// is left out of setup. Nations without a wave always play.
    pub fn in_waves(&self, max_wave: Option<i32>) -> bool {
        max_wave.is_none_or(|max| self.wave.is_none_or(|w| w <= max))
    }

    /// Every leader this nation offers, the default first.
    pub fn leaders(&self) -> impl Iterator<Item = &str> {
        self.leader_name.as_deref().into_iter().chain(self.alt_leader.as_ref().map(|a| a.leader_name.as_str()))
    }

    /// The ability of whoever leads: the nation's own uniques, or the second
    /// leader's in their place.
    pub fn leader_uniques(&self, alt_leader: bool) -> &[Unique] {
        match (&self.alt_leader, alt_leader) {
            (Some(alt), true) => &alt.uniques,
            _ => &self.uniques,
        }
    }

    /// The name of whoever leads.
    pub fn leader(&self, alt_leader: bool) -> Option<&str> {
        match (&self.alt_leader, alt_leader) {
            (Some(alt), true) => Some(&alt.leader_name),
            _ => self.leader_name.as_deref(),
        }
    }
}

/// A real faith that arises in its era (docs/CONTENT.md, Religion): an
/// object in `Religions.json` rather than the fixtures' bare name. Any object
/// there switches the ruleset to faiths by era: no pantheons, no founding and
/// no belief picking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FaithDef {
    pub name: String,
    /// The era it arises in, once the first major civ reaches it.
    pub era: String,
    /// Turns into that era before it arises, at Standard speed.
    pub delay: i32,
    /// Nations it arises in when present, in order of preference.
    pub homeland: Vec<String>,
    /// Its fixed traits, by name in Beliefs.json: `Follower` beliefs apply
    /// in cities that follow it, `State` beliefs to civs that adopt it.
    pub beliefs: Vec<String>,
}

/// Read one `Religions.json` entry: a faith object, or a fixture's name.
pub fn read_faith(j: &Json) -> Option<FaithDef> {
    let name = text(j, "name")?;
    let homeland = match j.get("homeland") {
        Some(h) if h.as_str().is_some() => vec![h.as_str().unwrap_or_default().to_string()],
        Some(h) => h.strings(),
        None => Vec::new(),
    };
    Some(FaithDef {
        name,
        era: text(j, "era").unwrap_or_default(),
        delay: j.get("delay").and_then(|v| v.as_i32()).unwrap_or(0).max(0),
        homeland,
        beliefs: j.get("beliefs").map(|v| v.strings()).unwrap_or_default(),
    })
}

/// A list of names for units that get one (`[This Unit] gets a name from
/// the [X] group`): Luminaries in `rules/base`. `regions` holds a pool per
/// region, drawn from first by a nation of that region; `unitNames` is a
/// pool for everyone, drawn from next, then the other regions in turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameGroup {
    pub name: String,
    pub unit_names: Vec<String>,
    pub regions: std::collections::BTreeMap<String, Vec<String>>,
}

pub fn read_name_group(j: &Json) -> Option<NameGroup> {
    Some(NameGroup {
        name: text(j, "name")?,
        unit_names: j.get("unitNames").map(|v| v.strings()).unwrap_or_default(),
        regions: j
            .get("regions")
            .and_then(|v| v.as_object())
            .unwrap_or_default()
            .iter()
            .map(|(region, names)| (region.clone(), names.strings()))
            .collect(),
    })
}

/// Tile-filter words the engine understands beyond the ruleset's own names
/// (terrains, resources, improvements and buildings). See
/// `world::conditionals::tile_matches_in` and `tile_matches`.
pub const TILE_FILTER_WORDS: [&str; 25] = [
    "Fresh Water",
    "non-fresh water",
    "resource",
    "Water resource",
    "Enemy Land",
    "Enemy",
    "All",
    "all",
    "Land",
    "Water",
    "Coastal",
    "River",
    "Fresh water",
    "Natural Wonder",
    "Isthmus",
    "unimproved",
    "Featureless",
    "Rough terrain",
    "Open terrain",
    "your",
    "Friendly Land",
    "Foreign Land",
    "Friendly",
    "Foreign",
    "City center",
];

/// A tile-filter word in some unique that names nothing: a misspelled
/// building in `<within [2] tiles of a [Colosus]>` would otherwise read as
/// "not decidable" and quietly apply everywhere.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnknownTileFilter {
    /// Where it was written: `Building Colossus`, `Policy Tradition`...
    pub source: String,
    pub term: String,
}

impl super::Ruleset {
    /// Every tile-filter term, in every unique and conditional of the rules,
    /// that is neither one of the ruleset's names nor a word the engine
    /// knows ([`TILE_FILTER_WORDS`]).
    pub fn unknown_tile_filters(&self) -> Vec<UnknownTileFilter> {
        let known = |term: &str| {
            TILE_FILTER_WORDS.contains(&term)
                || term
                    .strip_suffix(" resource")
                    .is_some_and(|k| self.resources.iter().any(|(_, x)| x.resource_type == k))
                || self.terrains.id(term).is_some()
                || self.resources.id(term).is_some()
                || self.improvements.id(term).is_some()
                || self.buildings.id(term).is_some()
        };
        let mut out = Vec::new();
        let mut check = |source: String, u: &Unique| {
            let mut stack = vec![u];
            while let Some(u) = stack.pop() {
                stack.extend(u.conditionals.iter());
                let Some(kind) = u.unique_type() else { continue };
                let slots: Vec<bool> = placeholders(kind.template()).map(|p| p == "tileFilter").collect();
                for (param, _) in u.params.iter().zip(slots).filter(|(_, is_tile)| *is_tile) {
                    let terms = std::cell::RefCell::new(Vec::new());
                    crate::world::filters::multi_filter(param, &|term| {
                        terms.borrow_mut().push(term.to_string());
                        crate::world::conditionals::Verdict::True
                    });
                    for term in terms.into_inner() {
                        if !known(&term) {
                            out.push(UnknownTileFilter { source: source.clone(), term });
                        }
                    }
                }
            }
        };
        macro_rules! scan {
            ($kind:literal, $registry:expr) => {
                for (_, x) in $registry.iter() {
                    for u in &x.uniques {
                        check(format!("{} {}", $kind, x.name), u);
                    }
                }
            };
        }
        scan!("Building", self.buildings);
        scan!("Unit", self.units);
        scan!("Policy", self.policies);
        scan!("Technology", self.technologies);
        scan!("Nation", self.nations);
        scan!("Belief", self.beliefs);
        scan!("Terrain", self.terrains);
        scan!("Improvement", self.improvements);
        scan!("Resource", self.resources);
        scan!("Promotion", self.promotions);
        for (_, n) in self.nations.iter() {
            for u in n.alt_leader.iter().flat_map(|a| &a.uniques) {
                check(format!("Leader {}", n.name), u);
            }
        }
        for u in &self.global_uniques {
            check("Global".into(), u);
        }
        out.sort();
        out.dedup();
        out
    }
}

/// The names of a template's top-level `[placeholders]`, in order.
fn placeholders(template: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut out = Vec::new();
    for (i, c) in template.char_indices() {
        match c {
            '[' => {
                if depth == 0 {
                    start = i + 1;
                }
                depth += 1;
            }
            ']' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    out.push(&template[start..i]);
                }
            }
            _ => {}
        }
    }
    out.into_iter()
}

/// A Blitz slice (docs/CONTENT.md, Spine): a game that starts in `eras[0]`,
/// cannot research past the last of `eras`, and ends after `turns` turns,
/// when The Chronicle (the highest score) decides if nobody has won. Written
/// in `ModOptions.json` under `slices`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SliceDef {
    pub name: String,
    /// The eras it covers, in order; the first is where it starts.
    pub eras: Vec<String>,
    pub turns: u32,
    /// Calendar years at its first and last turn, for the date shown.
    pub start_year: Option<i32>,
    pub end_year: Option<i32>,
}

pub fn read_slice(j: &Json) -> Option<SliceDef> {
    Some(SliceDef {
        name: text(j, "name")?,
        eras: j.get("eras").map(|v| v.strings()).unwrap_or_default(),
        turns: j.get("turns").and_then(|v| v.as_i32()).unwrap_or(60).max(1) as u32,
        start_year: j.get("startYear").and_then(|v| v.as_i32()),
        end_year: j.get("endYear").and_then(|v| v.as_i32()),
    })
}

/// Tuning for the influence and trade victories (`world::influence`), from
/// `ModOptions.json`. Every field has an engine default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VictoryKnobs {
    /// `influencePercent`: percent of new culture that becomes influence.
    pub influence_percent: i64,
    /// `influenceBonusPercent`: added for each of open borders, a running
    /// deal and a shared state faith.
    pub influence_bonus_percent: i64,
    /// `influenceFloor`: influence below this never counts as influential.
    pub influence_floor: i64,
    /// `tradeShareFromEra`: a civ's share of world trade counts only once it
    /// has reached this era (the first reserve currencies came with sail).
    pub trade_share_from_era: Option<String>,
    /// `worldTradePerMajor` and `worldTradePerMajorPerEra`: world trade
    /// counts once it reaches this much gold per living major civ, plus the
    /// second figure for each era the leading civ has reached.
    pub world_trade_per_major: i64,
    pub world_trade_per_major_per_era: i64,
}

impl Default for VictoryKnobs {
    fn default() -> Self {
        VictoryKnobs {
            // Tuned in AI self-play (12 games, 240 turns, all victories on);
            // see civ-sim's balance example.
            influence_percent: 100,
            influence_bonus_percent: 25,
            influence_floor: 1500,
            trade_share_from_era: None,
            world_trade_per_major: 15,
            world_trade_per_major_per_era: 3,
        }
    }
}

pub fn read_victory_knobs(j: &Json, into: &mut VictoryKnobs) {
    let int = |key: &str| j.get(key).and_then(|v| v.as_i64());
    if let Some(v) = int("influencePercent") {
        into.influence_percent = v.max(0);
    }
    if let Some(v) = int("influenceBonusPercent") {
        into.influence_bonus_percent = v.max(0);
    }
    if let Some(v) = int("influenceFloor") {
        into.influence_floor = v.max(0);
    }
    if let Some(v) = text(j, "tradeShareFromEra") {
        into.trade_share_from_era = Some(v);
    }
    if let Some(v) = int("worldTradePerMajor") {
        into.world_trade_per_major = v.max(0);
    }
    if let Some(v) = int("worldTradePerMajorPerEra") {
        into.world_trade_per_major_per_era = v.max(0);
    }
}
