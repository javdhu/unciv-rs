use super::*;

fn session() -> Session {
    Session::new(
        Settings {
            majors: 2,
            city_states: 1,
            ..Settings::default()
        },
        "test-epoch".into(),
        vec![0, 1],
    )
    .unwrap()
}
fn request(s: &Session, seat: u8, id: &str, action: Action) -> CommandRequest {
    CommandRequest {
        command_id: id.into(),
        epoch: s.epoch.clone(),
        turn_token: s.token(seat),
        action,
    }
}
fn own_unit(s: &Session, seat: u8, settler: bool) -> EntityRef {
    let id = s
        .world
        .units
        .iter()
        .find(|(_, u)| {
            u.owner == CivId(seat)
                && civ_core::world::settling::is_settler(&s.rules, u.base) == settler
        })
        .unwrap()
        .0;
    EntityRef {
        id: id.index(),
        gen: id.generation(),
    }
}
fn apply(s: Session, seat: u8, id: &str, action: Action) -> Session {
    let t = s
        .stage_command(seat, request(&s, seat, id, action))
        .unwrap();
    assert_eq!(t.receipt.result, CommandStatus::Applied, "command {id}");
    t.next
}

#[test]
fn one_finish_does_not_run_other_humans_or_advance_the_round() {
    let s = session();
    let original = s.world.clone();
    let s = apply(s, 0, "finish-a", Action::FinishTurn);
    assert_eq!(s.turn(), 0);
    assert_eq!(s.world, original);
    let blocked = s
        .stage_command(
            0,
            request(
                &s,
                0,
                "late",
                Action::SkipUnit {
                    unit: own_unit(&s, 0, false),
                },
            ),
        )
        .unwrap();
    assert_eq!(
        blocked.receipt.result,
        CommandStatus::Rejected {
            code: RejectionCode::TurnFinished
        }
    );
    let s = apply(s, 1, "finish-b", Action::FinishTurn);
    assert_eq!(s.turn(), 1);
    assert!(s.world.civs[0].cities.is_empty());
    assert!(s.world.civs[1].cities.is_empty());
    for seat in [0, 1] {
        assert!(!s.world.civs[seat].is_ai);
        assert!(s.world.civs[seat].researching.is_none());
    }
    s.verify_replay().unwrap();
}

#[test]
fn prepare_does_not_change_live_state_and_restore_recovers_old_turn_receipt() {
    let s = session();
    let req = request(
        &s,
        0,
        "found",
        Action::FoundCity {
            unit: own_unit(&s, 0, true),
            name: "First city".into(),
        },
    );
    let t = s.stage_command(0, req.clone()).unwrap();
    assert_eq!(t.receipt.result, CommandStatus::Applied);
    assert!(
        s.world.civs[0].cities.is_empty(),
        "failed persistence can discard the staged transition"
    );
    let receipt = t.receipt.clone();
    let s = Session::restore(&t.next.save().unwrap()).unwrap();
    let s = apply(s, 0, "ready-a", Action::FinishTurn);
    let s = apply(s, 1, "ready-b", Action::FinishTurn);
    let saved = s.save().unwrap();
    let s = Session::restore(&saved).unwrap();
    let retry = s.stage_command(0, req).unwrap();
    assert!(retry.duplicate);
    assert_eq!(retry.receipt, receipt);
    assert_eq!(retry.next.world, s.world);
    assert!(retry.updates.is_empty());
    assert_eq!(s.world.civs[0].cities.len(), 1);
    s.verify_replay().unwrap();
}

#[test]
fn duplicate_purchase_spends_once_and_altered_command_id_is_rejected() {
    let s = session();
    let settler = own_unit(&s, 0, true);
    let mut s = apply(
        s,
        0,
        "found",
        Action::FoundCity {
            unit: settler,
            name: "Capital".into(),
        },
    );
    s.world.civs[0].gold = 10_000;
    s.refresh(&[], None).unwrap();
    let city = s.world.civs[0].cities[0];
    let what = ConstructionRef::Building {
        id: s.rules.buildings.id("Monument").unwrap().index(),
    };
    let req = request(
        &s,
        0,
        "buy",
        Action::Purchase {
            city: EntityRef {
                id: city.index(),
                gen: city.generation(),
            },
            what,
            currency: Currency::Gold,
        },
    );
    let t = s.stage_command(0, req.clone()).unwrap();
    assert_eq!(t.receipt.result, CommandStatus::Applied);
    assert!(t.next.world.civs[0].gold < s.world.civs[0].gold);
    let recovered = Session::restore(&t.next.save().unwrap()).unwrap();
    let retry = recovered.stage_command(0, req.clone()).unwrap();
    assert_eq!(retry.receipt, t.receipt);
    assert_eq!(retry.next.world, recovered.world);
    let mut altered = req;
    altered.action = Action::FinishTurn;
    let reused = recovered.stage_command(0, altered).unwrap();
    assert_eq!(
        reused.receipt.result,
        CommandStatus::Rejected {
            code: RejectionCode::CommandIdReused
        }
    );
    assert_eq!(reused.next.world, recovered.world);
}

#[test]
fn foreign_and_missing_entity_probes_share_one_safe_rejection() {
    let s = session();
    let foreign = own_unit(&s, 1, false);
    let missing = EntityRef {
        id: u32::MAX,
        gen: 1,
    };
    for (id, unit) in [("foreign", foreign), ("missing", missing)] {
        let t = s
            .stage_command(0, request(&s, 0, id, Action::SkipUnit { unit }))
            .unwrap();
        assert_eq!(
            t.receipt.result,
            CommandStatus::Rejected {
                code: RejectionCode::IllegalAction
            }
        );
        assert_eq!(t.next.world, s.world);
        assert!(t.updates.is_empty());
    }
}

#[test]
fn hidden_actions_do_not_publish_a_foreign_revision() {
    let s = session();
    let unit = own_unit(&s, 0, false);
    assert!(!s
        .current_update(1)
        .unwrap()
        .view
        .units
        .iter()
        .any(|u| u.id == unit.id && u.gen == unit.gen));
    let before = s.current_update(1).unwrap();
    let t = s
        .stage_command(0, request(&s, 0, "skip", Action::SkipUnit { unit }))
        .unwrap();
    assert_eq!(t.receipt.result, CommandStatus::Applied);
    assert!(t.updates.iter().all(|(seat, _)| *seat != 1));
    let after = t.next.current_update(1).unwrap();
    assert_eq!(before.cursor, after.cursor);
    assert_eq!(before.view, after.view);
}

#[test]
fn timeout_is_once_per_turn_and_controller_changes_wait_for_boundary() {
    let s = session();
    let pending = s.stage_controller(0, false).unwrap().next;
    assert!(!pending.world.civs[0].is_ai);
    let next = pending.stage_timeout(0).unwrap().unwrap().next;
    assert_eq!(next.turn(), 1);
    assert!(next.world.civs[0].is_ai);
    assert!(
        next.world.civs[0].cities.is_empty(),
        "bot cannot act before takeover boundary"
    );
    assert!(next.stage_timeout(0).unwrap().is_none());
    let blocked = next
        .stage_command(0, request(&next, 0, "during-bot", Action::FinishTurn))
        .unwrap();
    assert_eq!(
        blocked.receipt.result,
        CommandStatus::Rejected {
            code: RejectionCode::NotControlled
        }
    );
    let returning = next.stage_controller(0, true).unwrap().next;
    let returned = returning.stage_timeout(1).unwrap().unwrap().next;
    assert!(!returned.world.civs[0].is_ai);
    returned.verify_replay().unwrap();
    Session::restore(&returned.save().unwrap()).unwrap();
}

#[test]
fn complete_content_and_host_settings_are_checked_before_play() {
    let s = session();
    assert!(!s.rules.city_state_types.is_empty());
    assert!(!s.rules.victories.is_empty());
    assert!(!s.rules.specialists.is_empty());
    assert!(!s.rules.global_uniques.is_empty());
    assert!(s.world.civs.iter().any(|c| c.is_barbarian));
    assert!(s.world.civs[2].city_state_type.is_some());
    for settings in [
        Settings {
            width: u16::MAX,
            ..Settings::default()
        },
        Settings {
            majors: 0,
            ..Settings::default()
        },
        Settings {
            seed: u64::MAX,
            ..Settings::default()
        },
        Settings {
            nation: Some("missing".into()),
            ..Settings::default()
        },
    ] {
        assert!(Session::new(settings, "e".into(), vec![0]).is_err());
    }
    assert!(Session::restore("{}").is_err());
    let mut save: serde_json::Value = serde_json::from_str(&s.save().unwrap()).unwrap();
    save["version"] = 999.into();
    assert!(Session::restore(&save.to_string()).is_err());
}

#[test]
fn eliminated_seats_cannot_force_bot_rounds() {
    let mut s = session();
    s.world.civs[0].defeated = true;
    let transition = s
        .stage_command(0, request(&s, 0, "dead-finish", Action::FinishTurn))
        .unwrap();
    assert_eq!(
        transition.receipt.result,
        CommandStatus::Rejected {
            code: RejectionCode::NotControlled
        }
    );
    assert_eq!(transition.next.world, s.world);
}

fn negotiating_session() -> Session {
    let mut s = Session::new(
        Settings {
            majors: 3,
            city_states: 0,
            ..Settings::default()
        },
        "offers".into(),
        vec![0, 1, 2],
    )
    .unwrap();
    s.world.diplomacy.meet(CivId(0), CivId(1));
    for civ in &mut s.world.civs[..3] {
        civ.gold = 100;
    }
    s.refresh(&[], None).unwrap();
    s
}

fn exchange(target: u8, give: i32, get: i32) -> Action {
    Action::OfferTrade {
        target,
        give: if give > 0 {
            vec![TradeItem::Gold { amount: give }]
        } else {
            vec![]
        },
        get: if get > 0 {
            vec![TradeItem::Gold { amount: get }]
        } else {
            vec![]
        },
    }
}

#[test]
fn human_trade_requires_recipient_consent_is_private_and_spends_once_after_restore() {
    let s = negotiating_session();
    let bystander = serde_json::to_value(s.current_update(2).unwrap()).unwrap();
    let proposal = s
        .stage_command(0, request(&s, 0, "proposal", exchange(1, 10, 20)))
        .unwrap();
    assert_eq!(proposal.receipt.result, CommandStatus::Applied);
    assert_eq!(
        proposal.next.world, s.world,
        "a proposal cannot spend the recipient's money"
    );
    assert_eq!(
        proposal
            .updates
            .iter()
            .map(|(seat, _)| *seat)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    let s = Session::restore(&proposal.next.save().unwrap()).unwrap();
    let offer = s.current_update(1).unwrap().offers[0].id.clone();
    assert_eq!(
        serde_json::to_value(s.current_update(2).unwrap()).unwrap(),
        bystander
    );
    for wrong_seat in [0, 2] {
        let rejected = s
            .stage_command(
                wrong_seat,
                request(
                    &s,
                    wrong_seat,
                    "forged",
                    Action::AnswerOffer {
                        offer: offer.clone(),
                        accept: true,
                    },
                ),
            )
            .unwrap();
        assert_eq!(
            rejected.receipt.result,
            CommandStatus::Rejected {
                code: RejectionCode::IllegalAction
            }
        );
        assert_eq!(rejected.next.world, s.world);
        assert_eq!(rejected.next.offers, s.offers);
    }
    let req = request(
        &s,
        1,
        "accept",
        Action::AnswerOffer {
            offer,
            accept: true,
        },
    );
    let accepted = s.stage_command(1, req.clone()).unwrap();
    assert_eq!(accepted.receipt.result, CommandStatus::Applied);
    assert_eq!(
        s.world.civs[0].gold, 100,
        "live state stays unchanged before durable commit"
    );
    assert_eq!(accepted.next.world.civs[0].gold, 110);
    assert_eq!(accepted.next.world.civs[1].gold, 90);
    assert!(accepted.next.current_update(0).unwrap().offers.is_empty());
    let restored = Session::restore(&accepted.next.save().unwrap()).unwrap();
    let retry = restored.stage_command(1, req).unwrap();
    assert!(retry.duplicate);
    assert_eq!(retry.receipt, accepted.receipt);
    assert_eq!(retry.next.world, restored.world);
}

#[test]
fn offers_are_immutable_withdrawable_and_rechecked_when_accepted() {
    let s = apply(negotiating_session(), 0, "old", exchange(1, 10, 20));
    let old = s.current_update(1).unwrap().offers[0].id.clone();
    let s = apply(s, 0, "new", exchange(1, 80, 90));
    let current = s.current_update(1).unwrap().offers[0].id.clone();
    let rejected = s
        .stage_command(
            1,
            request(
                &s,
                1,
                "old-accept",
                Action::AnswerOffer {
                    offer: old,
                    accept: true,
                },
            ),
        )
        .unwrap();
    assert_eq!(
        rejected.receipt.result,
        CommandStatus::Rejected {
            code: RejectionCode::IllegalAction
        }
    );
    let mut poor = s.clone();
    poor.world.civs[0].gold = 1;
    poor.refresh(&[], None).unwrap();
    let restored = Session::restore(&poor.save().unwrap()).unwrap();
    let rejected = restored
        .stage_command(
            1,
            request(
                &restored,
                1,
                "poor-accept",
                Action::AnswerOffer {
                    offer: current.clone(),
                    accept: true,
                },
            ),
        )
        .unwrap();
    assert_eq!(
        rejected.receipt.result,
        CommandStatus::Rejected {
            code: RejectionCode::IllegalAction
        }
    );
    assert_eq!(rejected.next.world, restored.world);
    assert_eq!(rejected.next.offers, restored.offers);
    let s = apply(
        s,
        0,
        "withdraw",
        Action::WithdrawOffer {
            offer: current.clone(),
        },
    );
    assert!(s.current_update(1).unwrap().offers.is_empty());
    let rejected = s
        .stage_command(
            1,
            request(
                &s,
                1,
                "late-accept",
                Action::AnswerOffer {
                    offer: current,
                    accept: true,
                },
            ),
        )
        .unwrap();
    assert_eq!(
        rejected.receipt.result,
        CommandStatus::Rejected {
            code: RejectionCode::IllegalAction
        }
    );
}

#[test]
fn recipient_private_wealth_and_technology_do_not_change_proposal_results() {
    let base = negotiating_session();
    for wealth in [0, 1, i32::MAX] {
        let mut s = base.clone();
        s.world.civs[1].gold = wealth;
        s.world.civs[1].techs.extend(s.rules.technologies.ids());
        s.world.civs[1]
            .uniques
            .insert(civ_core::Unique::parse("Enables Open Borders agreements"));
        let transition = s
            .stage_command(
                0,
                request(
                    &s,
                    0,
                    "ask",
                    Action::OfferTrade {
                        target: 1,
                        give: vec![TradeItem::Gold { amount: 1 }, TradeItem::OpenBorders],
                        get: vec![TradeItem::Gold { amount: 50 }],
                    },
                ),
            )
            .unwrap();
        assert_eq!(transition.receipt.result, CommandStatus::Applied);
        assert_eq!(transition.next.world, s.world);
    }
    let s = apply(base, 0, "ask-too-much", exchange(1, 10, 200));
    let offer = s.current_update(1).unwrap().offers[0].id.clone();
    let rejected = s
        .stage_command(
            1,
            request(
                &s,
                1,
                "cannot-pay",
                Action::AnswerOffer {
                    offer,
                    accept: true,
                },
            ),
        )
        .unwrap();
    assert_eq!(
        rejected.receipt.result,
        CommandStatus::Rejected {
            code: RejectionCode::IllegalAction
        }
    );
    assert_eq!(rejected.next.world, s.world);
}

#[test]
fn offers_expire_on_round_close_and_war_invalidates_trade() {
    let s = apply(negotiating_session(), 0, "proposal", exchange(1, 10, 20));
    let next = s.stage_timeout(s.turn()).unwrap().unwrap().next;
    assert!(next.current_update(0).unwrap().offers.is_empty());
    assert!(Session::restore(&next.save().unwrap()).is_ok());
    let s = apply(s, 1, "war", Action::DeclareWar { target: 0 });
    assert!(s.current_update(0).unwrap().offers.is_empty());
    let s = apply(s, 0, "peace", Action::OfferPeace { target: 1 });
    assert!(s.world.diplomacy.at_war(CivId(0), CivId(1)));
    let offer = s.current_update(1).unwrap().offers[0].id.clone();
    let s = apply(
        s,
        1,
        "decline",
        Action::AnswerOffer {
            offer,
            accept: false,
        },
    );
    assert!(s.world.diplomacy.at_war(CivId(0), CivId(1)));
    let s = apply(s, 0, "peace-again", Action::OfferPeace { target: 1 });
    let offer = s.current_update(1).unwrap().offers[0].id.clone();
    // Finishing one's turn leaves already-proposed consent valid until the boundary.
    let s = apply(s, 0, "ready", Action::FinishTurn);
    let s = apply(
        s,
        1,
        "peace-accept",
        Action::AnswerOffer {
            offer,
            accept: true,
        },
    );
    assert!(!s.world.diplomacy.at_war(CivId(0), CivId(1)));
}

#[test]
fn altered_saved_offer_terms_are_not_restored_as_consent() {
    let s = apply(negotiating_session(), 0, "proposal", exchange(1, 10, 20));
    for amount in [-10, 0, 50] {
        let mut value = serde_json::to_value(&s.offers).unwrap();
        value[0]["terms"]["give"][0]["amount"] = amount.into();
        let mut altered = s.clone();
        altered.offers = serde_json::from_value(value).unwrap();
        altered.refresh(&[], None).unwrap();
        assert!(Session::restore(&altered.save().unwrap()).is_err());
    }
}

#[test]
fn withdrawn_replaced_declined_or_consumed_consent_cannot_be_restored() {
    let original = apply(negotiating_session(), 0, "proposal", exchange(1, 10, 20));
    let offer = original.current_update(1).unwrap().offers[0].id.clone();
    for (seat, action) in [
        (
            0,
            Action::WithdrawOffer {
                offer: offer.clone(),
            },
        ),
        (0, exchange(1, 20, 30)),
        (
            1,
            Action::AnswerOffer {
                offer: offer.clone(),
                accept: false,
            },
        ),
        (
            1,
            Action::AnswerOffer {
                offer: offer.clone(),
                accept: true,
            },
        ),
    ] {
        let mut revoked = apply(original.clone(), seat, "revoke", action);
        assert!(Session::restore(&revoked.save().unwrap()).is_ok());
        revoked.offers = original.offers.clone();
        revoked.refresh(&[], None).unwrap();
        assert!(Session::restore(&revoked.save().unwrap()).is_err());
    }
}

#[test]
fn peace_offers_replay_from_only_seed_and_player_commands() {
    // Compact starts make contact during normal action settlement; no world edits.
    let mut s = Session::new(
        Settings {
            width: 16,
            height: 10,
            majors: 8,
            city_states: 0,
            ..Settings::default()
        },
        "replay-offers".into(),
        (0..8).collect(),
    )
    .unwrap();
    let unit = own_unit(&s, 0, false);
    s = apply(s, 0, "observe", Action::SkipUnit { unit });
    for round in 0..10 {
        if (0..8).any(|a| (a + 1..8).any(|b| s.world.diplomacy.have_met(CivId(a), CivId(b)))) {
            break;
        }
        for seat in 0..8 {
            let unit = own_unit(&s, seat, false);
            let closest = |tile| {
                s.world
                    .units
                    .iter()
                    .filter(|(_, other)| other.owner != CivId(seat) && other.owner.0 < 8)
                    .map(|(_, other)| s.world.grid.distance(tile, other.position))
                    .min()
                    .unwrap()
            };
            if let Some(to) = reachable(&s.world, &s.rules, CivId(seat), unit.id, unit.gen)
                .into_iter()
                .min_by_key(|reach| closest(TileId(reach.tile)))
                .map(|reach| reach.tile)
            {
                let transition = s
                    .stage_command(
                        seat,
                        request(
                            &s,
                            seat,
                            &format!("explore-{round}-{seat}"),
                            Action::MoveUnit { unit, to },
                        ),
                    )
                    .unwrap();
                s = transition.next;
            }
        }
        s = s.stage_timeout(s.turn()).unwrap().unwrap().next;
    }
    let (from, to) = (0..8)
        .flat_map(|a| (a + 1..8).map(move |b| (a, b)))
        .find(|(a, b)| s.world.diplomacy.have_met(CivId(*a), CivId(*b)))
        .expect("nearby starts meet");
    s = apply(s, from, "declare", Action::DeclareWar { target: to });
    s = apply(s, from, "offer", Action::OfferPeace { target: to });
    s = Session::restore(&s.save().unwrap()).unwrap();
    s.verify_replay().unwrap();
    let offer = s.current_update(to).unwrap().offers[0].id.clone();
    s = apply(
        s,
        to,
        "consent",
        Action::AnswerOffer {
            offer,
            accept: true,
        },
    );
    Session::restore(&s.save().unwrap())
        .unwrap()
        .verify_replay()
        .unwrap();
}

#[test]
fn an_unseen_recipient_elimination_does_not_remove_or_reject_offers() {
    let original = negotiating_session();
    let mut eliminated = original.clone();
    for unit in eliminated.world.civs[1].units.clone() {
        eliminated.world.remove_unit(unit);
    }
    assert_eq!(eliminated.world.resolve_defeats(), vec![CivId(1)]);
    for mut s in [original, eliminated] {
        s = apply(s, 0, "still-private", exchange(1, 10, 20));
        assert_eq!(s.current_update(0).unwrap().offers.len(), 1);
        let before = s.offers.clone();
        s.offers.prune(&s.world);
        assert_eq!(s.offers, before);
        assert!(Session::restore(&s.save().unwrap()).is_ok());
    }
}

#[test]
fn human_production_switches_preserve_work_across_rounds_save_and_replay() {
    let s = Session::new(
        Settings {
            nation: Some("America".into()),
            majors: 2,
            city_states: 1,
            ..Settings::default()
        },
        "test-epoch".into(),
        vec![0, 1],
    )
    .unwrap();
    let settler = own_unit(&s, 0, true);
    let mut s = apply(
        s,
        0,
        "found",
        Action::FoundCity {
            unit: settler,
            name: "Capital".into(),
        },
    );
    let city_id = s.world.civs[0].cities[0];
    let city = EntityRef {
        id: city_id.index(),
        gen: city_id.generation(),
    };
    let agriculture = s.rules.technologies.id("Agriculture").unwrap();
    assert!(
        s.world.civs[0].has_tech(agriculture),
        "starting technology is granted before play"
    );
    let a = ConstructionRef::Unit {
        id: s.rules.units.id("Warrior").unwrap().index(),
    };
    let b = ConstructionRef::Building {
        id: s.rules.buildings.id("Monument").unwrap().index(),
    };
    s = apply(
        s,
        0,
        "produce-a",
        Action::SetProduction {
            city,
            queue: vec![a.clone()],
        },
    );
    s = apply(s, 0, "round1-a", Action::FinishTurn);
    s = apply(s, 1, "round1-b", Action::FinishTurn);
    assert!(
        !s.finished(),
        "the surviving unfounded rival still has a potential capital"
    );
    let invested = s.world.cities.get(city_id).unwrap().current_production();
    assert!(invested > 0);
    s = apply(
        s,
        0,
        "produce-b",
        Action::SetProduction {
            city,
            queue: vec![b],
        },
    );
    assert_eq!(s.world.cities.get(city_id).unwrap().current_production(), 0);
    s = apply(s, 0, "round2-a", Action::FinishTurn);
    s = apply(s, 1, "round2-b", Action::FinishTurn);
    let before = s
        .world
        .cities
        .get(city_id)
        .unwrap()
        .production_progress
        .clone();
    assert_eq!(before.len(), 2);
    s = Session::restore(&s.save().unwrap()).unwrap();
    s = apply(
        s,
        0,
        "resume-a",
        Action::SetProduction {
            city,
            queue: vec![a],
        },
    );
    assert_eq!(
        s.world.cities.get(city_id).unwrap().current_production(),
        invested
    );
    assert_eq!(
        s.world.cities.get(city_id).unwrap().production_progress,
        before
    );
    s.verify_replay().unwrap();
}

#[test]
fn taking_the_last_city_removes_defenders_and_releases_the_round() {
    let mut s = session();
    for seat in [0, 1] {
        let settler = own_unit(&s, seat, true);
        s = apply(
            s,
            seat,
            &format!("found-{seat}"),
            Action::FoundCity {
                unit: settler,
                name: format!("Capital {seat}"),
            },
        );
    }
    // Arrange a siege so this tests the session's immediate elimination and
    // turn access, independently of map layout and a long military campaign.
    let target = s.world.civs[1].cities[0];
    let centre = s.world.cities.get(target).unwrap().centre;
    s.world.cities.get_mut(target).unwrap().health = 1;
    let attacker = s
        .world
        .units
        .iter()
        .find(|(_, u)| u.owner == CivId(0) && u.is_military(&s.rules))
        .unwrap()
        .0;
    let adjacent = s.world.grid.neighbours(centre).next().unwrap();
    s.world.units.get_mut(attacker).unwrap().position = adjacent;
    s.world.grid.terrain[adjacent.0 as usize] = s.rules.terrain("Grassland").unwrap();
    s.world.diplomacy.declare_war(CivId(0), CivId(1));
    civ_core::world::turn::update_visibility(&mut s.world);
    civ_core::world::fog::capture_observations(&mut s.world, &s.rules);
    s.refresh(&[], None).unwrap();
    assert!(!s.world.civs[1].units.is_empty());

    s = apply(
        s,
        0,
        "capture",
        Action::Attack {
            unit: EntityRef {
                id: attacker.index(),
                gen: attacker.generation(),
            },
            target: AttackTarget::City {
                city: EntityRef {
                    id: target.index(),
                    gen: target.generation(),
                },
            },
        },
    );
    assert_eq!(s.world.cities.get(target).unwrap().owner, CivId(0));
    assert!(!s.world.civs[1].is_alive());
    assert!(s.world.civs[1].units.is_empty());
    assert!(
        !s.current_update(1)
            .unwrap()
            .view
            .civs
            .iter()
            .find(|c| c.id == 1)
            .unwrap()
            .alive
    );
    let rejected = s
        .stage_command(1, request(&s, 1, "defeated-finish", Action::FinishTurn))
        .unwrap();
    assert_eq!(
        rejected.receipt.result,
        CommandStatus::Rejected {
            code: RejectionCode::NotControlled
        }
    );
    s = apply(s, 0, "survivor-finish", Action::FinishTurn);
    assert_eq!(
        s.turn(),
        1,
        "the defeated seat must not hold the round open"
    );
    assert!(s.finished());
    assert_eq!(s.world.winner.unwrap().0, CivId(0));
    let restored = Session::restore(&s.save().unwrap()).unwrap();
    assert_eq!(restored.world, s.world);
}

#[test]
fn defensive_pact_requires_saved_bilateral_consent_and_hides_other_technology() {
    use civ_core::world::{
        agreements::{self, Terms},
        trade::{self, Item},
        City,
    };
    let mut s = negotiating_session();
    for civ in [CivId(0), CivId(1)] {
        let tile = s
            .world
            .units
            .get(s.world.civ(civ).unwrap().units[0])
            .unwrap()
            .position;
        s.world
            .found_city_in(&s.rules, City::found("Capital", civ, tile));
        s.world
            .civ_mut(civ)
            .unwrap()
            .techs
            .insert(s.rules.technologies.id("Writing").unwrap());
    }
    s.world
        .civ_mut(CivId(0))
        .unwrap()
        .techs
        .insert(s.rules.technologies.id("Chivalry").unwrap());
    for civ in [CivId(0), CivId(1)] {
        civ_core::world::policies::rebuild_uniques(&mut s.world, &s.rules, civ);
    }
    agreements::accept(
        &mut s.world,
        &s.rules,
        CivId(0),
        CivId(1),
        &Terms::Friendship,
        Default::default(),
    )
    .unwrap();
    trade::accept_with(
        &mut s.world,
        &s.rules,
        CivId(0),
        CivId(1),
        (&[Item::Embassy], &[Item::Embassy]),
        Default::default(),
    )
    .unwrap();
    s.refresh(&[], None).unwrap();
    let proposal = s
        .stage_command(
            0,
            request(&s, 0, "pact", Action::OfferDefensivePact { target: 1 }),
        )
        .unwrap();
    assert_eq!(
        proposal.receipt.result,
        CommandStatus::Applied,
        "proposal cannot reveal the recipient's private technology"
    );
    assert_eq!(proposal.next.world, s.world);
    assert!(proposal.next.current_update(2).unwrap().offers.is_empty());
    let mut s = Session::restore(&proposal.next.save().unwrap()).unwrap();
    let offer = s.current_update(1).unwrap().offers[0].id.clone();
    let denied = s
        .stage_command(
            1,
            request(
                &s,
                1,
                "early",
                Action::AnswerOffer {
                    offer: offer.clone(),
                    accept: true,
                },
            ),
        )
        .unwrap();
    assert!(matches!(
        denied.receipt.result,
        CommandStatus::Rejected { .. }
    ));
    assert_eq!(denied.next.world, s.world);
    s.world
        .civ_mut(CivId(1))
        .unwrap()
        .techs
        .insert(s.rules.technologies.id("Chivalry").unwrap());
    civ_core::world::policies::rebuild_uniques(&mut s.world, &s.rules, CivId(1));
    let accepted = s
        .stage_command(
            1,
            request(
                &s,
                1,
                "accept-pact",
                Action::AnswerOffer {
                    offer,
                    accept: true,
                },
            ),
        )
        .unwrap();
    assert_eq!(accepted.receipt.result, CommandStatus::Applied);
    assert_eq!(
        accepted
            .next
            .world
            .diplomacy
            .pact(CivId(0), CivId(1))
            .unwrap()
            .defensive_pact_turns,
        30
    );
    assert!(accepted.next.current_update(0).unwrap().offers.is_empty());
}

#[test]
fn compensated_peace_keeps_private_saved_consent_and_commits_atomically() {
    let mut s = negotiating_session();
    s = apply(s, 0, "war-compensation", Action::DeclareWar { target: 1 });
    let a = s.world.civ(CivId(0)).unwrap().gold;
    let b = s.world.civ(CivId(1)).unwrap().gold;
    s = apply(
        s,
        0,
        "peace-compensation",
        Action::OfferPeaceTreaty {
            target: 1,
            give: vec![TradeItem::Gold { amount: 10 }],
            get: vec![],
        },
    );
    assert!(s.world.diplomacy.at_war(CivId(0), CivId(1)));
    assert_eq!(s.world.civ(CivId(0)).unwrap().gold, a);
    s = Session::restore(&s.save().unwrap()).unwrap();
    let offer = s.current_update(1).unwrap().offers[0].id.clone();
    s = apply(
        s,
        1,
        "accept-compensation",
        Action::AnswerOffer {
            offer,
            accept: true,
        },
    );
    assert!(!s.world.diplomacy.at_war(CivId(0), CivId(1)));
    assert_eq!(s.world.civ(CivId(0)).unwrap().gold, a - 10);
    assert_eq!(s.world.civ(CivId(1)).unwrap().gold, b + 10);
    assert!(s.current_update(0).unwrap().offers.is_empty());
    assert!(Session::restore(&s.save().unwrap()).is_ok());
}

#[test]
fn combat_previews_need_an_own_attacker_and_a_visible_target() {
    let mut s = session();
    let mine = own_unit(&s, 0, false);
    let theirs = own_unit(&s, 1, false);
    let target = AttackTarget::Unit { unit: theirs };
    let their_tile = s.world.units.iter().find(|(id, _)| id.index() == theirs.id).unwrap().1.position;
    let ask = |s: &Session, seat: u8, unit: EntityRef| match s.query(seat, Query::CombatPreview { unit, target: target.clone() }).unwrap().result {
        QueryResult::CombatPreview { preview } => preview,
        other => panic!("unexpected {other:?}"),
    };
    s.world.visibility.clear_visible(CivId(0));
    assert_eq!(ask(&s, 0, mine), None, "an unseen target reveals nothing");
    s.world.visibility.observe(CivId(0), their_tile);
    let p = ask(&s, 0, mine).expect("a visible target");
    assert!(p.attacker.strength > 0 && p.defender.damage_min <= p.defender.damage_max, "{p:?}");
    assert_eq!(ask(&s, 1, mine), None, "only the attacker's owner may ask");
}

#[test]
fn city_options_list_workable_tiles_with_yields() {
    let s = session();
    let settler = own_unit(&s, 0, true);
    let s = apply(s, 0, "found", Action::FoundCity { unit: settler, name: "Pasargadae".into() });
    let (id, city) = s.world.cities.iter().find(|(_, c)| c.owner == CivId(0)).unwrap();
    let options = crate::queries::city_options(&s.world, &s.rules, CivId(0), id.index(), id.generation()).unwrap();
    assert!(!options.workable_tiles.is_empty());
    assert!(options.workable_tiles.iter().all(|t| t.tile != city.centre.0), "the centre works for free");
    let worked: Vec<u16> = options.workable_tiles.iter().filter(|t| t.worked).map(|t| t.tile).collect();
    assert_eq!(worked, city.worked.iter().map(|t| t.0).collect::<Vec<_>>());
    assert!(options.workable_tiles.iter().any(|t| t.food > 0));
    assert!(crate::queries::city_options(&s.world, &s.rules, CivId(1), id.index(), id.generation()).is_none());
}

/// Seat 0 is human; civ 1 is an AI that is not a seat. Seat 2 is a second
/// human who must never learn about the other pair's diplomacy.
fn ai_session() -> Session {
    let mut s = Session::new(
        Settings {
            majors: 3,
            city_states: 0,
            ..Settings::default()
        },
        "ai-offers".into(),
        vec![0, 2],
    )
    .unwrap();
    s.world.diplomacy.meet(CivId(0), CivId(1));
    s.refresh(&[], None).unwrap();
    s
}
fn settle_capital(s: Session, seat: u8) -> Session {
    let settler = own_unit(&s, seat, true);
    apply(
        s,
        seat,
        "settle",
        Action::FoundCity {
            unit: settler,
            name: "Capital".into(),
        },
    )
}
fn offers_from_ai(s: &Session, seat: u8) -> Vec<DiplomaticOffer> {
    s.current_update(seat)
        .unwrap()
        .offers
        .into_iter()
        .filter(|o| o.from == 1)
        .collect()
}
/// Rounds until the AI offers these terms, holding the scripted situation.
fn await_offer(
    mut s: Session,
    rounds: u32,
    hold: impl Fn(&mut Session),
    wanted: impl Fn(&AgreementTerms) -> bool,
) -> (Session, DiplomaticOffer) {
    for _ in 0..rounds {
        hold(&mut s);
        s = s.stage_timeout(s.turn()).unwrap().unwrap().next;
        if let Some(offer) = offers_from_ai(&s, 0).into_iter().find(|o| wanted(&o.terms)) {
            return (s, offer);
        }
    }
    panic!("no such offer in {rounds} rounds");
}
fn warm(s: &mut Session) {
    s.world.diplomacy.attitude_mut(CivId(1), CivId(0)).set(
        civ_core::world::diplomacy::Sentiment::YearsOfPeace,
        civ_core::stats::Fixed::whole(80),
    );
}
fn answer(s: Session, id: &str, offer: &DiplomaticOffer, accept: bool) -> Session {
    apply(
        s,
        0,
        id,
        Action::AnswerOffer {
            offer: offer.id.clone(),
            accept,
        },
    )
}

#[test]
fn an_ai_offers_a_human_friendship_and_waits_after_a_refusal() {
    let (s, offer) = await_offer(ai_session(), 60, warm, |t| *t == AgreementTerms::Friendship);
    assert_eq!((offer.from, offer.to, offer.turn), (1, 0, s.world.turn));
    // The other human sees nothing of it.
    assert!(s.current_update(2).unwrap().offers.is_empty());
    // A pending AI offer survives save and restore.
    let s = Session::restore(&s.save().unwrap()).unwrap();

    // Accepting signs it.
    let accepted = answer(s.clone(), "yes", &offer, true);
    assert!(accepted
        .world
        .diplomacy
        .pact(CivId(0), CivId(1))
        .is_some_and(|p| p.friendship_turns > 0));

    // Refusing is private, and the AI does not ask again for a while.
    let t = s
        .stage_command(0, request(&s, 0, "no", Action::AnswerOffer { offer: offer.id.clone(), accept: false }))
        .unwrap();
    assert_eq!(t.receipt.result, CommandStatus::Applied);
    assert!(t.updates.iter().all(|(seat, _)| *seat != 2), "the other human learns nothing");
    let mut s = t.next;
    assert!(offers_from_ai(&s, 0).is_empty());
    for _ in 0..15 {
        warm(&mut s);
        s = s.stage_timeout(s.turn()).unwrap().unwrap().next;
        assert!(
            offers_from_ai(&s, 0).iter().all(|o| o.terms != AgreementTerms::Friendship),
            "asked again on turn {}",
            s.world.turn
        );
    }
}

#[test]
fn an_ai_losing_a_war_to_a_human_offers_peace() {
    let mut s = ai_session();
    s.world.diplomacy.declare_war(CivId(1), CivId(0));
    // Disarm the AI so it is losing; it sues once the war is ten turns old.
    let disarm = |s: &mut Session| {
        let soldiers: Vec<_> = s
            .world
            .units
            .iter()
            .filter(|(_, u)| u.owner == CivId(1) && u.is_military(&s.rules))
            .map(|(id, _)| id)
            .collect();
        for id in soldiers {
            s.world.remove_unit(id);
        }
    };
    let (s, offer) = await_offer(s, 30, disarm, |t| *t == AgreementTerms::Peace);
    assert!(s.world.turn >= 10);
    // Refusing keeps the war; accepting ends it.
    let refused = answer(s.clone(), "no", &offer, false);
    assert!(refused.world.diplomacy.at_war(CivId(0), CivId(1)));
    let accepted = answer(s, "yes", &offer, true);
    assert!(!accepted.world.diplomacy.at_war(CivId(0), CivId(1)));
}

#[test]
fn an_ai_offers_a_human_a_trade_of_embassies() {
    let mut s = settle_capital(ai_session(), 0);
    let writing = s.rules.technologies.id("Writing").unwrap();
    for civ in [CivId(0), CivId(1)] {
        s.world.civ_mut(civ).unwrap().techs.insert(writing);
        civ_core::world::effects::on_technology_researched(
            &mut s.world,
            &s.rules,
            civ,
            writing,
            civ_core::stats::Fixed::ONE,
        );
        civ_core::world::policies::rebuild_uniques(&mut s.world, &s.rules, civ);
    }
    let (s, offer) = await_offer(s, 40, |_| {}, |t| {
        matches!(t, AgreementTerms::Trade { give, get } if give == &[TradeItem::Embassy] && get == &[TradeItem::Embassy])
    });
    let refused = answer(s.clone(), "no", &offer, false);
    assert!(!refused.world.diplomacy.has_embassy(CivId(1), CivId(0)));
    let accepted = answer(s, "yes", &offer, true);
    assert!(accepted.world.diplomacy.has_embassy(CivId(1), CivId(0)));
    assert!(accepted.world.diplomacy.has_embassy(CivId(0), CivId(1)));
}

#[test]
fn a_human_cannot_forge_an_ai_offer_id() {
    let s = negotiating_session();
    let t = s
        .stage_command(0, request(&s, 0, "ai:7", Action::OfferFriendship { target: 1 }))
        .unwrap();
    assert!(matches!(t.receipt.result, CommandStatus::Rejected { .. }));
}
