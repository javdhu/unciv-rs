//! What another major could give, in the view (R7 in docs/ROOMS-SPEC.md): shown only while the viewer holds an
//! embassy in their capital and the two are at peace, and then exactly what the engine's own trade rules accept
//! from them.

use civ_core::ruleset::Ruleset;
use civ_core::world::agreements::{self, Terms};
use civ_core::world::research_agreements::can_give_embassy;
use civ_core::world::resources::available_stock;
use civ_core::world::trade::{self, Item};
use civ_core::world::{advance_turn, CityFocus, CivId, TurnModifiers, World};
use civ_sim::{new_game, player_view, Settings, TradableView};

fn majors(w: &World) -> Vec<CivId> {
    w.civs
        .iter()
        .filter(|c| c.is_major() && c.is_alive())
        .map(|c| c.id)
        .collect()
}

fn turn(w: &mut World, r: &Ruleset) {
    let mods = TurnModifiers::for_world(w, r);
    advance_turn(w, r, CityFocus::None, mods);
}

/// Two majors at peace and free to go to war, neither holding an embassy with the other yet, both able to grant
/// one; `host` has two cities `viewer` knows of and a spare luxury, so the trade view has something in it.
fn seeded_pair() -> (World, Ruleset, CivId, CivId) {
    let (mut w, r, _) = new_game(&Settings {
        seed: 1,
        ..Settings::default()
    })
    .unwrap();
    for _ in 0..200 {
        turn(&mut w, &r);
        let m = majors(&w);
        for &host in &m {
            for &viewer in &m {
                let known = w
                    .civ(host)
                    .unwrap()
                    .cities
                    .iter()
                    .filter(|id| w.visibility.known_cities(viewer).any(|c| c.id == **id))
                    .count();
                let luxury = available_stock(&w, &r, host).into_iter().any(|(id, n)| {
                    n >= 1 && r.resources.get(id).unwrap().resource_type == "Luxury"
                });
                if host != viewer
                    && w.diplomacy.have_met(host, viewer)
                    && w.diplomacy.can_declare_war(viewer, host)
                    && can_give_embassy(&w, host, viewer)
                    && can_give_embassy(&w, viewer, host)
                    && !w.diplomacy.has_embassy(host, viewer)
                    && !w.diplomacy.has_embassy(viewer, host)
                    && known >= 2
                    && luxury
                {
                    return (w, r, host, viewer);
                }
            }
        }
    }
    panic!("no pair of majors qualified in 200 turns");
}

/// Whether the engine, asked by `viewer` for `item` alone, carries the trade out.
fn accepted(w: &World, r: &Ruleset, viewer: CivId, host: CivId, item: Item) -> bool {
    let mut next = w.clone();
    let terms = Terms::Trade {
        give: vec![],
        get: vec![item],
    };
    agreements::accept(
        &mut next,
        r,
        viewer,
        host,
        &terms,
        TurnModifiers::for_world(w, r),
    )
    .is_ok()
}

/// Every listed item passes `can_give` from `host` to `viewer` and the engine carries it out when asked for alone;
/// one more than a listed amount is refused; cities, open borders and an embassy are listed exactly when accepted.
fn agrees_with_the_engine(w: &World, r: &Ruleset, viewer: CivId, host: CivId, t: &TradableView) {
    let mods = TurnModifiers::for_world(w, r);
    // Amounts are the engine's: treasury, recurring cap, spare stock.
    assert_eq!(t.gold, w.civ(host).unwrap().gold.max(0));
    let cap = trade::available_gold_per_turn(w, r, host, mods).unwrap();
    assert_eq!(i64::from(t.gold_per_turn), cap);
    let stock = available_stock(w, r, host);
    let expected = |kind: &str| -> Vec<(u32, i32)> {
        stock
            .iter()
            .filter(|(id, n)| **n >= 1 && r.resources.get(**id).unwrap().resource_type == kind)
            .map(|(id, n)| (id.index(), *n))
            .collect()
    };
    let luxuries: Vec<_> = t.luxuries.iter().map(|s| (s.resource, s.amount)).collect();
    let strategics: Vec<_> = t
        .strategics
        .iter()
        .map(|s| (s.resource, s.amount))
        .collect();
    assert_eq!(luxuries, expected("Luxury"));
    assert_eq!(strategics, expected("Strategic"));
    let resource = |index: u32| *stock.keys().find(|id| id.index() == index).unwrap();

    let mut items = Vec::new();
    if t.gold > 0 {
        items.push(Item::Gold(t.gold));
    }
    if t.gold_per_turn > 0 {
        items.push(Item::GoldPerTurn(t.gold_per_turn));
    }
    items.extend(luxuries.iter().map(|(id, _)| Item::Luxury(resource(*id))));
    items.extend(
        strategics
            .iter()
            .map(|(id, n)| Item::Strategic(resource(*id), *n)),
    );
    for item in &items {
        assert_eq!(trade::can_give(w, host, viewer, *item), Ok(()), "{item:?}");
        assert!(accepted(w, r, viewer, host, *item), "{item:?}");
    }
    let more = [
        Item::Gold(t.gold + 1),
        Item::GoldPerTurn(t.gold_per_turn + 1),
    ]
    .into_iter()
    .chain(
        strategics
            .iter()
            .map(|(id, n)| Item::Strategic(resource(*id), n + 1)),
    );
    for item in more {
        assert!(
            !accepted(w, r, viewer, host, item),
            "{item:?} is more than they have"
        );
    }

    for &id in &w.civ(host).unwrap().cities {
        let shown = t
            .cities
            .iter()
            .any(|c| (c.id, c.gen) == (id.index(), id.generation()));
        assert_eq!(
            shown,
            accepted(w, r, viewer, host, Item::City(id)),
            "city {id:?}"
        );
        if shown {
            assert_eq!(trade::can_give(w, host, viewer, Item::City(id)), Ok(()));
        }
    }
    for (shown, item) in [
        (t.open_borders, Item::OpenBorders),
        (t.embassy, Item::Embassy),
    ] {
        assert_eq!(shown, accepted(w, r, viewer, host, item), "{item:?}");
        assert_eq!(
            shown,
            trade::can_give(w, host, viewer, item).is_ok(),
            "{item:?}"
        );
    }
}

#[test]
fn tradable_lists_exactly_what_the_engine_accepts_from_them() {
    let (mut w, r, host, viewer) = seeded_pair();
    let relation = |w: &World, me: CivId, them: CivId| {
        player_view(w, &r, me)
            .relations
            .into_iter()
            .find(|x| x.civ == them.0)
            .expect("met")
    };
    assert_eq!(relation(&w, viewer, host).tradable, None, "no embassy yet");
    assert_eq!(relation(&w, host, viewer).tradable, None, "no embassy yet");

    // The host grants the viewer an embassy in its capital, by the engine's own trade.
    let mods = TurnModifiers::for_world(&w, &r);
    let grant = Terms::Trade {
        give: vec![Item::Embassy],
        get: vec![],
    };
    agreements::accept(&mut w, &r, host, viewer, &grant, mods).unwrap();
    let seen = relation(&w, viewer, host);
    assert!(seen.our_embassy && !seen.their_embassy);
    let back = relation(&w, host, viewer);
    assert!(back.their_embassy && !back.our_embassy);
    assert_eq!(
        back.tradable, None,
        "an embassy in your own capital shows you nothing"
    );
    let t = seen
        .tradable
        .expect("the viewer holds an embassy in the host's capital");

    agrees_with_the_engine(&w, &r, viewer, host, &t);
    assert!(t.gold > 0);
    assert!(!t.luxuries.is_empty() && !t.strategics.is_empty());
    assert!(
        !t.cities.is_empty(),
        "the host's second known city is tradable"
    );
    assert!(
        !t.embassy,
        "the embassy that opens this view is already granted"
    );

    // Sorted, so the view is deterministic.
    assert!(t.luxuries.windows(2).all(|p| p[0].resource < p[1].resource));
    assert!(t
        .strategics
        .windows(2)
        .all(|p| p[0].resource < p[1].resource));
    assert!(t.cities.windows(2).all(|p| p[0].id < p[1].id));

    // On the wire: an optional camelCase field, absent when there is nothing to show.
    let relation_json = |me: CivId, them: CivId| {
        let json = serde_json::to_value(player_view(&w, &r, me)).unwrap();
        json["relations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["civ"] == them.0)
            .unwrap()
            .clone()
    };
    let ours = relation_json(viewer, host);
    assert_eq!(ours["tradable"]["goldPerTurn"], t.gold_per_turn);
    assert_eq!(ours["tradable"]["openBorders"], t.open_borders);
    assert!(relation_json(host, viewer).get("tradable").is_none());

    // At war the view shows nothing (war also closes the embassy).
    let mut war = w.clone();
    assert!(war.diplomacy.can_declare_war(viewer, host));
    civ_core::world::diplomacy_actions::declare_war(&mut war, &r, viewer, host);
    assert!(war.diplomacy.at_war(viewer, host));
    assert_eq!(relation(&war, viewer, host).tradable, None);
    assert_eq!(relation(&war, host, viewer).tradable, None);
}

/// Over a seeded game with natural embassies: a civ never sees `tradable` for itself or a city-state, and sees it
/// for another major exactly while it holds an embassy there and the two are at peace.
#[test]
fn tradable_follows_the_embassy_gate_for_every_seat() {
    let (mut w, r, _) = new_game(&Settings {
        seed: 4,
        ..Settings::default()
    })
    .unwrap();
    let (mut shown, mut paying, mut city_states) = (0, 0, 0);
    for _ in 0..60 {
        turn(&mut w, &r);
        for me in majors(&w) {
            for rel in player_view(&w, &r, me).relations {
                assert_ne!(rel.civ, me.0, "a civ has no relation with itself");
                let them = w.civ(CivId(rel.civ)).unwrap();
                if them.is_city_state() {
                    city_states += 1;
                    assert_eq!(rel.tradable, None, "city-states do not trade");
                    continue;
                }
                let gate = them.is_alive() && rel.our_embassy && !rel.at_war;
                assert_eq!(rel.our_embassy, w.diplomacy.has_embassy(me, them.id));
                assert_eq!(
                    rel.tradable.is_some(),
                    gate,
                    "{me:?} on {:?}, turn {}",
                    them.id,
                    w.turn
                );
                if let Some(t) = &rel.tradable {
                    agrees_with_the_engine(&w, &r, me, them.id, t);
                    shown += 1;
                    paying += usize::from(t.gold_per_turn > 0);
                }
            }
        }
    }
    assert!(
        shown > 0,
        "the AI traded embassies, so some view showed tradable"
    );
    assert!(paying > 0, "some shown civ had gold a turn to spare");
    assert!(city_states > 0, "some city-state was met");
}
