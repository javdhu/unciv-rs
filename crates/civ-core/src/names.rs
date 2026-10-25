// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How the game's own words read to a player (docs/CONTENT.md, Names).
//!
//! Unique patterns are the engine's keys and keep Civ's wording ("Great
//! General provides double combat bonus", "Receive [..]% Gold from Barbarian
//! encampments"): rulesets are written against them and the parser matches
//! them. Anything shown to a player passes through [`player_text`] instead,
//! which puts civ.game's names on them. One table, one place.

/// Civ's coinages and what civ.game calls them, longest first so a phrase is
/// replaced before any shorter one inside it.
pub const NAMES: [(&str, &str); 30] = [
    ("We Love the King Day", "Feast Days"),
    ("We Love The King Day", "Feast Days"),
    ("we love the king day", "feast days"),
    ("Declaration of Friendship", "Treaty of Friendship"),
    ("Declaration of friendship", "Treaty of friendship"),
    ("declaration of friendship", "treaty of friendship"),
    ("Research Agreement", "Scholarly Exchange"),
    ("Research agreement", "Scholarly exchange"),
    ("research agreement", "scholarly exchange"),
    ("Great Prophet", "Sage"),
    ("Great Scientist", "Scholar"),
    ("Great Engineer", "Master Builder"),
    ("Great Merchant", "Magnate"),
    ("Great Artist", "Virtuoso"),
    ("Great General", "Marshal"),
    ("Great Admiral", "Admiral"),
    ("Great People", "Luminaries"),
    ("Great Person", "Luminary"),
    ("great people", "luminaries"),
    ("great person", "luminary"),
    ("great prophet", "sage"),
    ("Barbarians", "Raiders"),
    ("Barbarian", "Raider"),
    ("barbarians", "raiders"),
    ("barbarian", "raider"),
    ("Civilizations", "Nations"),
    ("Civilization", "Nation"),
    ("civilizations", "nations"),
    ("civilization", "nation"),
    ("Spaceship part", "Turing Project part"),
];

/// `text` as a player should read it: Civ's coined names replaced by
/// civ.game's. Everything else, including names in brackets that are not on
/// the list, is left alone.
pub fn player_text(text: &str) -> String {
    let mut out = text.to_string();
    for (from, to) in NAMES {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

/// A unique as a player reads it, in prose: its conditionals follow it as
/// words, the brackets and braces of its parameters go, `[All]` reads as
/// "all" (and vanishes after "constructing"), and Civ's names give way to
/// civ.game's. Numbers and signs stay. None for a unique hidden from players.
pub fn unique_prose(u: &crate::Unique) -> Option<String> {
    use crate::UniqueType;
    if u.conditionals.iter().any(|c| c.unique_type() == Some(UniqueType::ModifierHiddenFromUsers)) {
        return None;
    }
    if u.unique_type() == Some(UniqueType::Comment) {
        return u.params.first().map(|p| prose(p));
    }
    let mut text = u.text.clone();
    for c in u.conditionals.iter().filter(|c| c.unique_type() != Some(UniqueType::SuppressWarnings)) {
        text.push(' ');
        text.push_str(&c.text);
    }
    Some(prose(&text))
}

/// Rule text with brackets as prose (see [`unique_prose`]).
pub fn prose(text: &str) -> String {
    let mut s =
        text.replace("constructing [All] ", "constructing ").replace("[All] ", "all ").replace("[all] ", "all ");
    // `non-[Puppeted]` reads "non-Puppeted", innermost first.
    while let Some(start) = s.find("non-[") {
        let open = start + 4;
        let Some(len) = s[open..].find(']') else { break };
        let inner = s[open + 1..open + len].to_string();
        s.replace_range(start..open + len + 1, &format!("non-{inner}"));
    }
    let s: String = s.chars().filter(|c| !matches!(c, '[' | ']' | '{' | '}')).collect();
    player_text(&s.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::{player_text, prose, unique_prose};
    use crate::Unique;

    #[test]
    fn rule_text_reads_as_prose() {
        let read = |raw: &str| unique_prose(&Unique::parse(raw)).unwrap();
        assert_eq!(
            read("[+15]% Production when constructing [All] wonders [in all cities]"),
            "+15% Production when constructing wonders in all cities"
        );
        assert_eq!(
            read("[+33]% XP gained from combat <for [Military] units>"),
            "+33% XP gained from combat for Military units"
        );
        assert_eq!(read("[+1] Movement <for [{Military} {Water}] units>"), "+1 Movement for Military Water units");
        assert_eq!(
            read("Only available <if [Market] is constructed in all [non-[Puppeted]] cities>"),
            "Only available if Market is constructed in all non-Puppeted cities"
        );
        assert_eq!(
            read("[+15]% Strength <for [All] units> <when fighting in [Friendly Land] tiles>"),
            "+15% Strength for all units when fighting in Friendly Land tiles"
        );
        assert_eq!(read("Great General provides double combat bonus"), "Marshal provides double combat bonus");
        assert_eq!(
            read("Enemy [Land] units must spend [1] extra movement points when inside your territory <before discovering [Dynamite]> <Suppress warning [*contains a conditional on a unit movement unique*]>"),
            "Enemy Land units must spend 1 extra movement points when inside your territory before discovering Dynamite"
        );
        assert_eq!(
            read("Comment [Units ending their turn on [Mountain] tiles take [50] damage]"),
            "Units ending their turn on Mountain tiles take 50 damage"
        );
        assert_eq!(unique_prose(&Unique::parse("[+1 Gold] <hidden from users>")), None);
        assert_eq!(prose("[-15]% Culture cost of adopting new Policies"), "-15% Culture cost of adopting new Policies");
    }

    #[test]
    fn fixed_sentences_read_with_civ_games_names() {
        for (pattern, shown) in [
            ("Great General provides double combat bonus", "Marshal provides double combat bonus"),
            ("[+25]% Gold from Great Merchant trade missions", "[+25]% Gold from Magnate trade missions"),
            (
                "[-25]% Faith cost of generating Great Prophet equivalents",
                "[-25]% Faith cost of generating Sage equivalents",
            ),
            ("Gain enough Faith for [50]% of a Great Prophet", "Gain enough Faith for [50]% of a Sage"),
            (
                "Receive [100]% Gold from Barbarian encampments and pillaging Cities",
                "Receive [100]% Gold from Raider encampments and pillaging Cities",
            ),
            ("Free Great Person", "Free Luminary"),
            ("[+10]% Great Person generation [in all cities]", "[+10]% Luminary generation [in all cities]"),
            (
                "Allied City-States will occasionally gift Great People",
                "Allied City-States will occasionally gift Luminaries",
            ),
            ("Can trade civilization introductions for [100] Gold", "Can trade nation introductions for [100] Gold"),
            ("Limited to [1] per Civilization", "Limited to [1] per Nation"),
            ("Enables Research agreements", "Enables Scholarly exchanges"),
            ("[+15]% Food during We Love the King Day", "[+15]% Food during Feast Days"),
            ("Spaceship part", "Turing Project part"),
            ("Enables construction of Spaceship parts", "Enables construction of Turing Project parts"),
        ] {
            assert_eq!(player_text(pattern), shown);
        }
    }

    #[test]
    fn other_greats_and_plain_words_stay() {
        for text in
            ["[+2 Culture] from every [Great Wall]", "Great Library", "Golden Age", "City-State", "Open Borders"]
        {
            assert_eq!(player_text(text), text);
        }
    }
}
