use crate::unique_type::UniqueType;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Whether a unique string resolved to a declared rule or is a bare filter tag.
///
/// Unciv's rulesets contain both: 638 declared `UniqueType`s carrying behaviour,
/// plus a handful of undeclared flags (e.g. `"Aircraft"` on a `UnitType`) that
/// exist only to be matched by name from a filter. Modelling the second case
/// explicitly is what keeps mods loadable without a code change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UniqueKind {
    Known(UniqueType),
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unique {
    pub kind: UniqueKind,
    /// Base text with all `<conditionals>` removed.
    pub text: String,
    /// Contents of each top-level `[...]`, nesting preserved verbatim.
    pub params: Vec<String>,
    /// `<...>` modifiers, themselves parsed as uniques.
    pub conditionals: Vec<Unique>,
}

fn lookup() -> &'static HashMap<&'static str, UniqueType> {
    static MAP: OnceLock<HashMap<&'static str, UniqueType>> = OnceLock::new();
    MAP.get_or_init(|| UniqueType::ALL.iter().map(|&t| (t.placeholder(), t)).collect())
}

/// Split `a <b> c <d>` into (`"a c"`, `["b", "d"]`), respecting nesting.
fn split_conditionals(s: &str) -> (String, Vec<String>) {
    let (mut base, mut mods, mut cur, mut depth) = (String::new(), Vec::new(), String::new(), 0u32);
    for c in s.chars() {
        match c {
            '<' => {
                depth += 1;
                if depth == 1 {
                    cur.clear();
                    continue;
                }
            }
            '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    mods.push(std::mem::take(&mut cur));
                    continue;
                }
            }
            _ => {}
        }
        if depth == 0 {
            base.push(c)
        } else {
            cur.push(c)
        }
    }
    (base.split_whitespace().collect::<Vec<_>>().join(" "), mods)
}

/// Collapse each top-level `[param]` to `[]` and collect the params.
///
/// Bracket-aware, so nested filters like `[{non-[Natural Wonder]} {Mountain}]`
/// stay a single parameter instead of being torn apart.
fn placeholder_and_params(s: &str) -> (String, Vec<String>) {
    let (mut ph, mut params, mut cur, mut depth) = (String::new(), Vec::new(), String::new(), 0u32);
    for c in s.chars() {
        match c {
            '[' => {
                depth += 1;
                if depth == 1 {
                    ph.push_str("[]");
                    cur.clear();
                    continue;
                }
            }
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    params.push(std::mem::take(&mut cur));
                    continue;
                }
            }
            _ => {}
        }
        if depth == 0 {
            ph.push(c)
        } else {
            cur.push(c)
        }
    }
    (ph, params)
}

impl Unique {
    pub fn parse(raw: &str) -> Unique {
        let (base, mods) = split_conditionals(raw);
        let (ph, params) = placeholder_and_params(&base);
        Unique {
            kind: lookup().get(ph.as_str()).copied().map_or(UniqueKind::Tag, UniqueKind::Known),
            text: base,
            params,
            conditionals: mods.iter().map(|m| Unique::parse(m)).collect(),
        }
    }

    pub fn unique_type(&self) -> Option<UniqueType> {
        match self.kind {
            UniqueKind::Known(t) => Some(t),
            UniqueKind::Tag => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_unique() {
        let u = Unique::parse("[+2 Food] from [Forest] tiles [in all cities]");
        assert_eq!(u.unique_type(), Some(UniqueType::StatsFromTiles));
        assert_eq!(u.params, ["+2 Food", "Forest", "in all cities"]);
    }

    #[test]
    fn keeps_nested_filters_as_one_param() {
        let u = Unique::parse("Must have an owned [{non-[Natural Wonder]} {Mountain}] within [2] tiles");
        assert_eq!(u.params, ["{non-[Natural Wonder]} {Mountain}", "2"]);
        assert!(u.unique_type().is_some());
    }

    #[test]
    fn splits_conditionals_and_parses_them_too() {
        let u = Unique::parse("[+15]% Strength <when fighting units from a Civilization with more Cities than you>");
        assert!(u.unique_type().is_some());
        assert_eq!(u.conditionals.len(), 1);
        assert!(u.conditionals[0].unique_type().is_some());
    }

    #[test]
    fn undeclared_flags_become_tags_not_errors() {
        assert_eq!(Unique::parse("Aircraft").kind, UniqueKind::Tag);
    }

    #[test]
    fn every_template_round_trips_to_itself() {
        for t in UniqueType::ALL {
            assert_eq!(Unique::parse(t.template()).unique_type(), Some(t), "{:?}", t);
        }
    }
}
