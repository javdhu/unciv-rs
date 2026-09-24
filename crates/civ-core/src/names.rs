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

#[cfg(test)]
mod tests {
    use super::player_text;

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
