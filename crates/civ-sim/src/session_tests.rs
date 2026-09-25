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
            nation: Some("United States".into()),
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
        .insert(s.rules.technologies.id("Stirrups").unwrap());
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
        .insert(s.rules.technologies.id("Stirrups").unwrap());
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

fn found_capital(s: Session, seat: u8) -> Session {
    let settler = own_unit(&s, seat, true);
    apply(
        s,
        seat,
        &format!("found-{seat}"),
        Action::FoundCity {
            unit: settler,
            name: format!("Capital {seat}"),
        },
    )
}
fn add_unit(s: &mut Session, seat: u8, name: &str) -> EntityRef {
    let base = s.rules.units.id(name).unwrap();
    let city = s.world.civ(CivId(seat)).unwrap().cities[0];
    let centre = s.world.cities.get(city).unwrap().centre;
    let unit = civ_core::world::construction::prepare_unit_near(
        &s.world,
        &s.rules,
        CivId(seat),
        base,
        centre,
        Some(city),
    )
    .unwrap();
    let id = s.world.place_unit(unit);
    EntityRef {
        id: id.index(),
        gen: id.generation(),
    }
}
fn clear_barbarians(s: &mut Session) {
    let barbarians = s.world.civs.iter().find(|c| c.is_barbarian).unwrap().id;
    s.world.barbarian_camps.clear();
    let theirs: Vec<_> = s
        .world
        .units
        .iter()
        .filter(|(_, u)| u.owner == barbarians)
        .map(|(id, _)| id)
        .collect();
    for id in theirs {
        s.world.remove_unit(id);
    }
}
fn next_round(s: Session) -> Session {
    s.stage_timeout(s.turn()).unwrap().unwrap().next
}
fn automation_of(s: &Session, seat: u8, unit: EntityRef) -> Option<crate::view::UnitAutomation> {
    s.current_update(seat)
        .unwrap()
        .view
        .units
        .iter()
        .find(|u| u.id == unit.id && u.gen == unit.gen)
        .and_then(|u| u.own.as_ref()?.automation)
}
fn unit_actions(s: &Session, seat: u8, unit: EntityRef) -> Vec<crate::queries::UnitActionKind> {
    let reply = s.query(seat, Query::UnitOptions { unit, nuclear_target: None }).unwrap();
    let QueryResult::UnitOptions { options } = reply.result else {
        panic!("unit options")
    };
    options.map_or_else(Vec::new, |o| o.actions.into_iter().map(|a| a.action).collect())
}
fn improved_tiles(s: &Session, seat: u8) -> usize {
    s.world
        .grid
        .ids()
        .filter(|t| {
            s.world.grid.owner[t.0 as usize] == Some(CivId(seat))
                && s.world.grid.improvement[t.0 as usize].is_some()
        })
        .count()
}

#[test]
fn an_automated_worker_improves_tiles_over_twenty_rounds() {
    let mut s = found_capital(session(), 0);
    let worker = add_unit(&mut s, 0, "Worker");
    use crate::queries::UnitActionKind as K;
    let offered = unit_actions(&s, 0, worker);
    assert!(offered.contains(&K::Automate) && !offered.contains(&K::Explore), "{offered:?}");
    let before = improved_tiles(&s, 0);
    s = apply(s, 0, "automate", Action::AutomateUnit { unit: worker });
    assert_eq!(automation_of(&s, 0, worker), Some(crate::view::UnitAutomation::Work));
    assert!(!unit_actions(&s, 0, worker).contains(&K::Automate), "already automated");
    // Barbarians wandering up would rightly stop the worker; keep them away.
    for _ in 0..20 {
        clear_barbarians(&mut s);
        s = next_round(s);
    }
    assert!(improved_tiles(&s, 0) >= before + 2, "{} tiles improved in 20 rounds", improved_tiles(&s, 0) - before);
    assert_eq!(automation_of(&s, 0, worker), Some(crate::view::UnitAutomation::Work));
    // Other seats never learn about the standing order.
    let theirs = s.current_update(1).unwrap().view;
    assert!(theirs.units.iter().filter(|u| u.owner == 0).all(|u| u.own.is_none()));
}

#[test]
fn an_explorer_reveals_tiles_and_a_manual_order_cancels_it() {
    let s = session();
    let warrior = own_unit(&s, 0, false);
    use crate::queries::UnitActionKind as K;
    let offered = unit_actions(&s, 0, warrior);
    assert!(offered.contains(&K::Explore) && !offered.contains(&K::Automate), "{offered:?}");
    let explored = |s: &Session| {
        s.world
            .grid
            .ids()
            .filter(|t| s.world.visibility.has_explored(CivId(0), *t))
            .count()
    };
    let before = explored(&s);
    let mut s = apply(s, 0, "explore", Action::ExploreUnit { unit: warrior });
    let moved = explored(&s);
    assert!(moved > before, "the order acts as soon as it is given");
    for _ in 0..5 {
        clear_barbarians(&mut s);
        s = next_round(s);
    }
    assert!(explored(&s) > moved, "exploring continued over the next rounds");
    assert_eq!(automation_of(&s, 0, warrior), Some(crate::view::UnitAutomation::Explore));

    // A save keeps the order.
    let restored = Session::restore(&s.save().unwrap()).unwrap();
    assert_eq!(automation_of(&restored, 0, warrior), Some(crate::view::UnitAutomation::Explore));

    // Any manual order ends it.
    let s = apply(restored, 0, "skip", Action::SkipUnit { unit: warrior });
    assert_eq!(automation_of(&s, 0, warrior), None);
}

#[test]
fn a_visible_enemy_next_to_an_automated_unit_stops_it() {
    let mut s = found_capital(session(), 0);
    let worker = add_unit(&mut s, 0, "Worker");
    s = apply(s, 0, "automate", Action::AutomateUnit { unit: worker });
    let id = s
        .world
        .units
        .iter()
        .find(|(id, _)| id.index() == worker.id)
        .unwrap()
        .0;
    let at = s.world.units.get(id).unwrap().position;
    let barbarians = s.world.civs.iter().find(|c| c.is_barbarian).unwrap().id;
    let brute = s.rules.units.id("Brute").or_else(|| s.rules.units.id("Warrior")).unwrap();
    let spot = s
        .world
        .grid
        .neighbours(at)
        .find(|t| {
            s.world.visibility.can_see(CivId(0), *t)
                && s.world.units.iter().all(|(_, u)| u.position != *t)
                && civ_core::world::movement::is_passable(&s.world.grid, &s.rules, *t)
                && !s.world.grid.is_water(&s.rules, *t)
        })
        .unwrap();
    s.world
        .place_unit(civ_core::world::entities::Unit::new(brute, barbarians, spot, 1));
    s = next_round(s);
    assert_eq!(automation_of(&s, 0, worker), None);
}

#[test]
fn a_battle_between_two_ais_in_view_is_replayed_in_order() {
    let mut s = Session::new(
        Settings {
            majors: 3,
            city_states: 0,
            ..Settings::default()
        },
        "replay".into(),
        vec![0],
    )
    .unwrap();
    for (a, b) in [(0, 1), (0, 2), (1, 2)] {
        s.world.diplomacy.meet(CivId(a), CivId(b));
    }
    s.world.diplomacy.declare_war(CivId(1), CivId(2));
    let here = s
        .world
        .units
        .iter()
        .find(|(_, u)| u.owner == CivId(0))
        .unwrap()
        .1
        .position;
    let land: Vec<_> = s
        .world
        .grid
        .within(here, 2)
        .into_iter()
        .filter(|t| {
            *t != here
                && s.world.visibility.can_see(CivId(0), *t)
                && !s.world.grid.is_water(&s.rules, *t)
                && civ_core::world::movement::is_passable(&s.world.grid, &s.rules, *t)
                && s.world.units.iter().all(|(_, u)| u.position != *t)
        })
        .collect();
    let (x, y) = land
        .iter()
        .flat_map(|x| land.iter().map(move |y| (*x, *y)))
        .find(|(x, y)| s.world.grid.distance(*x, *y) == 1)
        .expect("two adjacent visible land tiles");
    let unit = |name: &str| s.rules.units.id(name).unwrap();
    let strong = s
        .world
        .place_unit(civ_core::world::Unit::new(unit("Swordsman"), CivId(1), x, 2));
    let weak = s
        .world
        .place_unit(civ_core::world::Unit::new(unit("Warrior"), CivId(2), y, 2));
    s.refresh(&[], None).unwrap();

    let t = s.stage_timeout(s.turn()).unwrap().unwrap();
    let (_, update) = t.updates.iter().find(|(seat, _)| *seat == 0).unwrap();
    let replay = update.replay.as_ref().expect("the round had something to replay");
    let actors: Vec<u8> = replay.iter().map(|b| b.civ).collect();
    let mut sorted = actors.clone();
    sorted.sort();
    assert_eq!(actors, sorted, "batches follow the order civs acted in");
    assert!(actors.iter().all(|c| *c != 0), "never our own turn");
    let fight = replay
        .iter()
        .flat_map(|b| b.events.iter().map(move |e| (b.civ, e)))
        .find(|(_, e)| {
            matches!(&e.event, crate::view::PresentationEvent::Attacked { attacker: crate::protocol::AttackTarget::Unit { unit }, .. }
                if (unit.id, unit.gen) == (strong.index(), strong.generation())
                    || (unit.id, unit.gen) == (weak.index(), weak.generation()))
        })
        .expect("the fight in view is in the replay");
    assert!(fight.0 == 1 || fight.0 == 2);
    let crate::view::PresentationEvent::Attacked { from, at, viewer_attacked, .. } = &fight.1.event else {
        unreachable!()
    };
    assert!(!viewer_attacked);
    assert!([x.0, y.0].contains(from) && [x.0, y.0].contains(at));

    // Old clients: the field is optional on the wire.
    let mut json = serde_json::to_value(update).unwrap();
    json.as_object_mut().unwrap().remove("replay");
    let old: GameUpdate = serde_json::from_value(json).unwrap();
    assert!(old.replay.is_none());
    let quiet = t.next.current_update(0).unwrap();
    assert!(serde_json::to_value(&quiet).unwrap().get("replay").is_none());
}

#[test]
fn replays_over_a_real_game_stay_inside_what_the_human_explored() {
    let mut s = Session::new(
        Settings {
            // A seed whose deal brings another nation into view within thirty rounds.
            seed: 4,
            majors: 4,
            city_states: 2,
            ..Settings::default()
        },
        "replay-game".into(),
        vec![0],
    )
    .unwrap();
    let mut shown = 0;
    for _ in 0..30 {
        let t = s.stage_timeout(s.turn()).unwrap().unwrap();
        s = t.next;
        for (seat, update) in &t.updates {
            for batch in update.replay.iter().flatten() {
                assert_ne!(batch.civ, *seat);
                for e in &batch.events {
                    let tiles: Vec<u16> = match &e.event {
                        crate::view::PresentationEvent::UnitMoved { from, to, .. } => vec![*from, *to],
                        crate::view::PresentationEvent::Attacked { from, at, .. } => vec![*from, *at],
                        _ => Vec::new(),
                    };
                    for t in tiles {
                        assert!(
                            s.world.visibility.has_explored(CivId(*seat), civ_core::world::TileId(t)),
                            "replayed tile {t} was never seen"
                        );
                    }
                    shown += 1;
                }
            }
        }
    }
    assert!(shown > 0, "thirty rounds with nothing to replay");
}

fn warrior_of(s: &Session, seat: u8) -> (Id<civ_core::world::Unit>, EntityRef) {
    let (id, _) = s
        .world
        .units
        .iter()
        .find(|(_, u)| u.owner == CivId(seat) && u.is_military(&s.rules))
        .unwrap();
    (id, EntityRef { id: id.index(), gen: id.generation() })
}
/// A neighbouring land tile whose view is already ours: a friendly civilian
/// stands there, so a move onto it shows nothing new.
fn covered_step(s: &mut Session, seat: u8, unit: Id<civ_core::world::Unit>) -> u16 {
    let at = s.world.units.get(unit).unwrap().position;
    let to = s
        .world
        .grid
        .neighbours(at)
        .find(|t| {
            !s.world.grid.is_water(&s.rules, *t)
                && civ_core::world::movement::is_passable(&s.world.grid, &s.rules, *t)
                && s.world.units.iter().all(|(_, u)| u.position != *t)
                && s.world.grid.features[t.0 as usize].is_empty()
        })
        .unwrap();
    let settler = s.rules.units.id("Settler").unwrap();
    s.world
        .place_unit(civ_core::world::Unit::new(settler, CivId(seat), to, 2));
    civ_core::world::turn::update_visibility_in(&mut s.world, &s.rules);
    civ_core::world::fog::capture_observations(&mut s.world, &s.rules);
    s.refresh(&[], None).unwrap();
    to.0
}
fn rejected(s: &Session, seat: u8, id: &str, action: Action) -> UndoRefusal {
    let t = s.stage_command(seat, request(s, seat, id, action)).unwrap();
    assert_eq!(t.receipt.result, CommandStatus::Rejected { code: RejectionCode::Unavailable }, "{id}");
    t.receipt.undo_refusal.expect("an undo refusal says why")
}

#[test]
fn a_move_that_showed_nothing_new_can_be_taken_back() {
    let mut s = Session::new(Settings { majors: 2, city_states: 0, ..Settings::default() }, "undo".into(), vec![0]).unwrap();
    assert_eq!(rejected(&s, 0, "early", Action::Undo), UndoRefusal::NothingToUndo);
    let (id, warrior) = warrior_of(&s, 0);
    let to = covered_step(&mut s, 0, id);
    let before = s.world.clone();
    let s = apply(s, 0, "step", Action::MoveUnit { unit: warrior, to });
    assert_eq!(s.world.units.get(id).unwrap().position.0, to);
    let t = s.stage_command(0, request(&s, 0, "undo", Action::Undo)).unwrap();
    assert_eq!(t.receipt.result, CommandStatus::Applied);
    assert_eq!(t.next.world, before, "the world is exactly as it was");
    let (_, update) = t.updates.iter().find(|(seat, _)| *seat == 0).unwrap();
    assert!(update.view.units.iter().any(|u| u.id == warrior.id && u.tile == before.units.get(id).unwrap().position.0));
    // Only the last command, and only once.
    assert_eq!(rejected(&t.next, 0, "again", Action::Undo), UndoRefusal::NothingToUndo);
    // Solo, a production or research choice can be taken back too.
    let s = t.next;
    let tech = s.rules.technologies.iter().find(|(id, _)| !s.world.civ(CivId(0)).unwrap().has_tech(*id)).unwrap().0;
    let s = apply(s, 0, "research", Action::SetResearch { technology: tech.index() });
    let s = apply(s, 0, "unresearch", Action::Undo);
    assert_ne!(s.world.civ(CivId(0)).unwrap().researching, Some(tech));

    // Replaying the log, undo included, reaches the same world.
    let s = Session::new(Settings { majors: 2, city_states: 0, ..Settings::default() }, "undo-replay".into(), vec![0]).unwrap();
    let s = apply(s, 0, "research", Action::SetResearch { technology: tech.index() });
    let s = apply(s, 0, "unresearch", Action::Undo);
    s.verify_replay().unwrap();
}

#[test]
fn an_undo_cannot_be_used_to_peek_at_hidden_tiles() {
    let s = Session::new(Settings { majors: 2, city_states: 0, ..Settings::default() }, "peek".into(), vec![0]).unwrap();
    let (id, warrior) = warrior_of(&s, 0);
    let at = s.world.units.get(id).unwrap().position;
    let explored = |s: &Session| s.world.grid.ids().filter(|t| s.world.visibility.has_explored(CivId(0), *t)).count();
    let seen_before = explored(&s);
    // Step outward: the move shows new ground.
    let out = s
        .world
        .grid
        .neighbours(at)
        .filter(|t| !s.world.grid.is_water(&s.rules, *t) && civ_core::world::movement::is_passable(&s.world.grid, &s.rules, *t))
        .find(|t| {
            let mut probe = s.world.clone();
            civ_core::world::commands::execute(&mut probe, &s.rules, CivId(0), civ_core::world::commands::Command::MoveUnit { unit: id, to: *t })
                .is_ok()
                && probe.grid.ids().filter(|x| probe.visibility.has_explored(CivId(0), *x)).count() > seen_before
        })
        .unwrap();
    let s = apply(s, 0, "scout", Action::MoveUnit { unit: warrior, to: out.0 });
    let after = s.world.clone();
    assert!(explored(&s) > seen_before);
    assert_eq!(rejected(&s, 0, "take-back", Action::Undo), UndoRefusal::Revealed);
    // Refused means refused: the unit stays where the look cost it.
    let t = s.stage_command(0, request(&s, 0, "take-back-2", Action::Undo)).unwrap();
    assert_eq!(t.next.world, after);
}

#[test]
fn rooms_undo_only_moves_and_only_before_anything_else_happens() {
    let mut s = Session::new(Settings { majors: 2, city_states: 0, ..Settings::default() }, "room".into(), vec![0, 1]).unwrap();
    let tech = s.rules.technologies.iter().find(|(id, _)| !s.world.civ(CivId(0)).unwrap().has_tech(*id)).unwrap().0;
    s = apply(s, 0, "research", Action::SetResearch { technology: tech.index() });
    assert_eq!(rejected(&s, 0, "u1", Action::Undo), UndoRefusal::NotAllowed);

    let (id, warrior) = warrior_of(&s, 0);
    let to = covered_step(&mut s, 0, id);
    s = apply(s, 0, "step", Action::MoveUnit { unit: warrior, to });
    // The other player acts in the meantime: the world is not ours to rewind.
    let other = own_unit(&s, 1, false);
    let later = apply(s.clone(), 1, "their-skip", Action::SkipUnit { unit: other });
    assert_eq!(rejected(&later, 0, "u2", Action::Undo), UndoRefusal::OthersActed);
    // Without that, the move comes back.
    let s = apply(s, 0, "u3", Action::Undo);
    assert_ne!(s.world.units.get(id).unwrap().position.0, to);
}

#[test]
fn a_turn_end_or_a_fight_makes_a_move_final() {
    let mut s = Session::new(Settings { majors: 2, city_states: 0, ..Settings::default() }, "final".into(), vec![0]).unwrap();
    let (id, warrior) = warrior_of(&s, 0);
    let to = covered_step(&mut s, 0, id);
    s = apply(s, 0, "step", Action::MoveUnit { unit: warrior, to });
    let s = s.stage_timeout(s.turn()).unwrap().unwrap().next;
    assert_eq!(rejected(&s, 0, "late", Action::Undo), UndoRefusal::TurnEnded);

    // An attack is never taken back.
    let mut s = Session::new(Settings { majors: 2, city_states: 0, ..Settings::default() }, "fight".into(), vec![0]).unwrap();
    let (id, warrior) = warrior_of(&s, 0);
    let at = s.world.units.get(id).unwrap().position;
    let spot = s
        .world
        .grid
        .neighbours(at)
        .find(|t| {
            s.world.visibility.can_see(CivId(0), *t)
                && !s.world.grid.is_water(&s.rules, *t)
                && civ_core::world::movement::is_passable(&s.world.grid, &s.rules, *t)
                && s.world.units.iter().all(|(_, u)| u.position != *t)
        })
        .unwrap();
    let barbarians = s.world.civs.iter().find(|c| c.is_barbarian).unwrap().id;
    let brute = s.world.place_unit(civ_core::world::Unit::new(s.rules.units.id("Brute").unwrap(), barbarians, spot, 1));
    s.refresh(&[], None).unwrap();
    s = apply(
        s,
        0,
        "hit",
        Action::Attack { unit: warrior, target: AttackTarget::Unit { unit: EntityRef { id: brute.index(), gen: brute.generation() } } },
    );
    assert_eq!(rejected(&s, 0, "unhit", Action::Undo), UndoRefusal::Combat);
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

#[test]
fn a_human_adopts_a_state_faith_by_action() {
    let s = session();
    assert!(s.rules.faith_mode(), "the shipped rules raise faiths by era");
    let settler = own_unit(&s, 0, true);
    let s = apply(
        s,
        0,
        "found",
        Action::FoundCity {
            unit: settler,
            name: "First city".into(),
        },
    );
    let mut s = s;
    let city = s.world.civs[0].cities[0];
    let hinduism = civ_core::world::faiths::key(0);
    let turn = s.world.turn;
    s.world
        .faiths
        .arisen
        .insert(0, civ_core::world::faiths::Arisen { turn, city });
    let c = s.world.cities.get_mut(city).unwrap();
    c.faith.add_pressure(Some(hinduism), 5000);
    c.faith.recount(c.population);

    let options = crate::queries::civ_options(&s.world, &s.rules, CivId(0)).state_faith.unwrap();
    assert_eq!(options.current, None);
    assert_eq!(options.adoptable, [hinduism.0]);

    let other = request(&s, 0, "wrong", Action::AdoptStateFaith { religion: 129 });
    let t = s.stage_command(0, other).unwrap();
    assert!(matches!(t.receipt.result, CommandStatus::Rejected { .. }), "Judaism has not arisen");

    let s = apply(s, 0, "adopt", Action::AdoptStateFaith { religion: hinduism.0 });
    assert_eq!(s.world.civs[0].faith.state, Some(hinduism));
    let options = crate::queries::civ_options(&s.world, &s.rules, CivId(0)).state_faith.unwrap();
    assert_eq!(options.current, Some(hinduism.0));
    assert_eq!(options.locked_turns, civ_core::world::faiths::STATE_FAITH_LOCK);
    let view = crate::view::player_view(&s.world, &s.rules, CivId(0));
    assert_eq!(view.civs.iter().find(|c| c.id == 0).unwrap().state_faith, Some(hinduism.0));
}

#[test]
fn choose_site_reaches_the_engine_and_is_refused_for_a_building_that_stands_nowhere() {
    let s = session();
    let settler = own_unit(&s, 0, true);
    let s = apply(
        s,
        0,
        "found",
        Action::FoundCity {
            unit: settler,
            name: "First city".into(),
        },
    );
    let city = s.world.civs[0].cities[0];
    let monument = s.rules.buildings.id("Monument").unwrap();
    let tile = s.world.cities.get(city).unwrap().tiles[0];
    let req = request(
        &s,
        0,
        "site",
        Action::ChooseSite {
            city: EntityRef {
                id: city.index(),
                gen: city.generation(),
            },
            building: monument.index(),
            tile: tile.0,
        },
    );
    let t = s.stage_command(0, req).unwrap();
    assert!(matches!(t.receipt.result, CommandStatus::Rejected { .. }));
}

#[test]
fn a_player_chooses_where_a_wonder_stands_and_it_is_built_there() {
    let s = session();
    let settler = own_unit(&s, 0, true);
    let mut s = apply(
        s,
        0,
        "found",
        Action::FoundCity {
            unit: settler,
            name: "First city".into(),
        },
    );
    let city = s.world.civs[0].cities[0];
    let centre = s.world.cities.get(city).unwrap().centre;
    for t in s.world.grid.within(centre, 2) {
        s.world.claim_tile(city, t);
    }
    let temple = s.rules.buildings.id("Temple of Artemis").unwrap();
    let techs: Vec<_> = s.rules.technologies.ids().collect();
    s.world.civs[0].techs.extend(techs);
    s.world.civs[0].gold = 100_000;
    let city_ref = EntityRef {
        id: city.index(),
        gen: city.generation(),
    };
    let options = crate::queries::city_options(&s.world, &s.rules, CivId(0), city.index(), city.generation()).unwrap();
    let choice = options
        .choices
        .iter()
        .find(|c| c.construction == ConstructionRef::Building { id: temple.index() })
        .expect("the Temple of Artemis is offered");
    let sites = choice.sites.as_ref().expect("a wonder stands on a tile");
    assert!(sites.takes_tile && sites.tiles.len() > 1);
    let best = sites.selected.unwrap();
    let other = *sites.tiles.iter().find(|t| **t != best).unwrap();

    let s = apply(
        s,
        0,
        "site",
        Action::ChooseSite {
            city: city_ref,
            building: temple.index(),
            tile: other,
        },
    );
    let options = crate::queries::city_options(&s.world, &s.rules, CivId(0), city.index(), city.generation()).unwrap();
    let choice = options
        .choices
        .iter()
        .find(|c| c.construction == ConstructionRef::Building { id: temple.index() })
        .unwrap();
    assert_eq!(choice.sites.as_ref().unwrap().selected, Some(other), "the choice is remembered");
    let mut s = apply(
        s,
        0,
        "queue",
        Action::SetProduction {
            city: city_ref,
            queue: vec![ConstructionRef::Building { id: temple.index() }],
        },
    );
    let unfinished = crate::view::player_view(&s.world, &s.rules, CivId(0));
    let work = unfinished.cities.iter().find(|c| c.id == city.index()).unwrap().wonder_construction.as_ref().unwrap();
    assert_eq!((work.building, work.tile, work.fraction), (temple.index(), other, 0.0));
    let foreign = crate::view::player_view(&s.world, &s.rules, CivId(1));
    assert!(foreign.cities.iter().all(|c| c.wonder_construction.is_none()));
    let costs = TurnModifiers::for_world(&s.world, &s.rules).costs;
    civ_core::world::construction::apply_production_in(&mut s.world, &s.rules, city, 10, costs).unwrap();
    let progressed = crate::view::player_view(&s.world, &s.rules, CivId(0));
    let work = progressed.cities.iter().find(|c| c.id == city.index()).unwrap().wonder_construction.as_ref().unwrap();
    assert_eq!(work.tile, other);
    assert!(work.fraction > 0.0 && work.fraction < 1.0);
    // Its production arrives all at once.
    let costs = TurnModifiers::for_world(&s.world, &s.rules).costs;
    civ_core::world::construction::apply_production_in(&mut s.world, &s.rules, city, 1_000_000, costs).unwrap();
    assert!(s.world.cities.get(city).unwrap().built.contains(&temple));
    // As the end of a round does, before the next update is drawn.
    civ_core::world::fog::capture_observations(&mut s.world, &s.rules);
    assert_eq!(civ_core::world::placement::standing(&s.world, city, temple), Some(TileId(other)));
    let view = crate::view::player_view(&s.world, &s.rules, CivId(0));
    assert!(view.cities.iter().find(|c| c.id == city.index()).unwrap().wonder_construction.is_none());
    let placed = view.tiles.iter().find(|t| t.id == other).and_then(|t| t.placed.as_ref());
    assert!(placed.is_some_and(|p| p.building == temple.index() && p.wonder), "the view shows it: {placed:?}");
}

fn city_ref(id: Id<civ_core::world::City>) -> EntityRef {
    EntityRef { id: id.index(), gen: id.generation() }
}
fn city_view(s: &Session, seat: u8, id: Id<civ_core::world::City>) -> Option<crate::view::CityView> {
    s.current_update(seat).unwrap().view.cities.into_iter().find(|c| c.id == id.index() && c.gen == id.generation())
}

#[test]
fn a_city_focus_and_pins_are_actions_and_part_of_the_own_city_view() {
    use crate::protocol::CityFocus as Focus;
    let solo = Session::new(Settings { majors: 2, city_states: 0, ..Settings::default() }, "focus".into(), vec![0]);
    let s = found_capital(solo.unwrap(), 0);
    let city = s.world.civs[0].cities[0];
    let c = city_view(&s, 0, city).unwrap();
    assert_eq!(c.queue_refs, Some(Vec::new()));
    let own = c.own.unwrap();
    assert!(own.capital && own.original_capital && !own.may_raze);
    assert_eq!(own.raze_refusal, Some(crate::queries::Refusal::OriginalCapital));
    assert_eq!((own.founder, own.previous_owner), (0, None));
    assert_eq!(own.focus, Focus::Balanced);
    assert!(own.locked_tiles.is_empty());
    let theirs = crate::view::player_view(&s.world, &s.rules, CivId(1));
    let seen = theirs.cities.iter().find(|c| c.id == city.index());
    assert!(seen.is_none_or(|c| c.own.is_none() && c.queue_refs.is_none()), "not for foreign eyes");

    let s = apply(s, 0, "focus", Action::SetCityFocus { city: city_ref(city), focus: Focus::Production });
    assert_eq!(city_view(&s, 0, city).unwrap().own.unwrap().focus, Focus::Production);
    assert_eq!(s.world.cities.get(city).unwrap().focus, Focus::Production.engine());
    let options = crate::queries::city_options(&s.world, &s.rules, CivId(0), city.index(), city.generation()).unwrap();
    let tile = options.workable_tiles.iter().find(|t| !t.worked && !t.taken).unwrap().tile;
    let s = apply(s, 0, "pin", Action::LockTile { city: city_ref(city), tile, locked: true });
    let own = city_view(&s, 0, city).unwrap().own.unwrap();
    assert_eq!(own.locked_tiles, vec![tile]);
    assert!(own.worked.contains(&tile));
    // Solo, a pin comes off with an undo, like any citizen change.
    let s = apply(s, 0, "unpin", Action::Undo);
    assert!(city_view(&s, 0, city).unwrap().own.unwrap().locked_tiles.is_empty());

    // The queue is typed as well as named.
    let unit = options
        .choices
        .iter()
        .find(|c| matches!(c.construction, ConstructionRef::Unit { .. }) && c.refusal.is_none())
        .unwrap()
        .construction
        .clone();
    let s = apply(s, 0, "build", Action::SetProduction { city: city_ref(city), queue: vec![unit.clone()] });
    let c = city_view(&s, 0, city).unwrap();
    assert_eq!(c.queue_refs, Some(vec![unit]));
    assert_eq!(c.queue.map(|q| q.len()), Some(1));
    s.verify_replay().unwrap();
}

#[test]
fn a_captured_city_waits_for_its_new_owners_answer() {
    use crate::view::{CaptureChoice, PendingCaptureView};
    let mut s = session();
    for seat in [0, 1] {
        s = found_capital(s, seat);
    }
    clear_barbarians(&mut s);
    // A second city of theirs, far from both capitals, with a free tile beside it.
    let free = |s: &Session, t: TileId| {
        s.world.grid.owner[t.0 as usize].is_none()
            && s.world.units.iter().all(|(_, u)| u.position != t)
            && s.world.cities.iter().all(|(_, c)| s.world.grid.distance(c.centre, t) > 3)
    };
    let (site, stand) = s
        .world
        .grid
        .ids()
        .filter(|t| free(&s, *t))
        .find_map(|t| s.world.grid.neighbours(t).find(|n| free(&s, *n)).map(|n| (t, n)))
        .unwrap();
    for t in [site, stand] {
        s.world.grid.terrain[t.0 as usize] = s.rules.terrain("Grassland").unwrap();
        s.world.grid.features[t.0 as usize].clear();
    }
    let veii = s.world.found_city_in(&s.rules, civ_core::world::City::found("Veii", CivId(1), site));
    s.world.claim_tile(veii, site);
    let c = s.world.cities.get_mut(veii).unwrap();
    c.health = 1;
    c.population = 3;
    let (attacker, warrior) = warrior_of(&s, 0);
    s.world.units.get_mut(attacker).unwrap().position = stand;
    s.world.diplomacy.declare_war(CivId(0), CivId(1));
    civ_core::world::turn::update_visibility(&mut s.world);
    civ_core::world::fog::capture_observations(&mut s.world, &s.rules);
    s.refresh(&[], None).unwrap();
    assert!(s.current_update(0).unwrap().view.pending_captures.is_none());

    let s = apply(s, 0, "take", Action::Attack { unit: warrior, target: AttackTarget::City { city: city_ref(veii) } });
    assert_eq!(s.world.cities.get(veii).unwrap().owner, CivId(0));
    assert_eq!(
        s.current_update(0).unwrap().view.pending_captures,
        Some(vec![PendingCaptureView {
            city: city_ref(veii),
            choices: vec![CaptureChoice::Annex, CaptureChoice::Keep, CaptureChoice::Raze],
        }])
    );
    let own = city_view(&s, 0, veii).unwrap().own.unwrap();
    assert!(own.puppet && own.may_raze && !own.capital && !own.original_capital);
    assert_eq!((own.founder, own.previous_owner, own.raze_refusal), (1, Some(1), None));
    assert!(s.current_update(1).unwrap().view.pending_captures.is_none(), "only the conqueror decides");

    let s = apply(s, 0, "keep", Action::KeepPuppet { city: city_ref(veii) });
    assert!(s.current_update(0).unwrap().view.pending_captures.is_none());
    assert!(s.world.cities.get(veii).unwrap().puppet);
    let again = s.stage_command(0, request(&s, 0, "keep-again", Action::KeepPuppet { city: city_ref(veii) })).unwrap();
    assert!(matches!(again.receipt.result, CommandStatus::Rejected { .. }));
}

#[test]
fn city_options_say_why_they_refuse_and_price_every_tile_the_city_could_buy() {
    use crate::queries::Refusal;
    let mut s = found_capital(session(), 0);
    let city = s.world.civs[0].cities[0];
    let centre = s.world.cities.get(city).unwrap().centre;
    let options =
        |s: &Session| crate::queries::city_options(&s.world, &s.rules, CivId(0), city.index(), city.generation()).unwrap();

    s.world.civs[0].gold = 0;
    let poor = options(&s);
    assert!(!poor.purchasable_tiles.is_empty(), "tiles are listed even when the treasury is empty");
    assert!(poor.purchasable_tiles.iter().all(|t| !t.affordable));
    for quote in poor.choices.iter().filter_map(|c| c.gold.as_ref()).filter(|q| q.amount.is_some()) {
        assert!(matches!(quote.refusal, Some(Refusal::CannotAfford | Refusal::Occupied)), "{quote:?}");
    }
    s.world.civs[0].gold = 100_000;
    let rich = options(&s);
    let tiles = |o: &crate::queries::CityOptions| {
        o.purchasable_tiles.iter().map(|t| (t.tile, t.gold.clone())).collect::<Vec<_>>()
    };
    assert_eq!(tiles(&rich), tiles(&poor));
    assert!(rich.purchasable_tiles.iter().all(|t| t.affordable));

    // A unit of the kind already standing in the city cannot be bought.
    let unit = rich
        .choices
        .iter()
        .find(|c| {
            matches!(c.construction, ConstructionRef::Unit { .. }) && c.gold.as_ref().is_some_and(|q| q.amount.is_some())
        })
        .unwrap();
    let ConstructionRef::Unit { id } = unit.construction else { unreachable!() };
    let base = s.rules.units.ids().find(|b| b.index() == id).unwrap();
    s.world.place_unit(civ_core::world::Unit::new(base, CivId(0), centre, 2));
    let quote = options(&s).choices.into_iter().find(|c| c.construction == unit.construction).unwrap().gold.unwrap();
    assert_eq!(quote.refusal, Some(Refusal::Occupied));

    // A specialist slot says what its specialist yields here.
    let (building, _) = s.rules.buildings.iter().find(|(_, b)| b.specialist_slots.contains_key("Scientist")).unwrap();
    let c = s.world.cities.get_mut(city).unwrap();
    c.built.insert(building);
    civ_core::world::city::rebuild_city_uniques(&s.rules, c);
    let scientist = s.rules.specialists.id("Scientist").unwrap();
    let slot = options(&s).specialist_slots.into_iter().find(|slot| slot.specialist == scientist.index()).unwrap();
    assert_eq!(slot.yields.science, 3);

    // A puppet's governor decides both what is built and what is bought.
    s.world.cities.get_mut(city).unwrap().puppet = true;
    let puppet = options(&s);
    assert!(puppet.choices.iter().all(|c| c.refusal == Some(Refusal::Puppet)));
    assert!(puppet
        .choices
        .iter()
        .filter_map(|c| c.gold.as_ref())
        .all(|q| q.amount.is_none() || q.refusal == Some(Refusal::Puppet)));
    assert!(puppet.purchasable_tiles.is_empty());
}

#[test]
fn a_wonder_site_keeps_the_engines_own_pick_beside_the_players() {
    let mut s = found_capital(session(), 0);
    let city = s.world.civs[0].cities[0];
    let centre = s.world.cities.get(city).unwrap().centre;
    for t in s.world.grid.within(centre, 2) {
        s.world.claim_tile(city, t);
    }
    let techs: Vec<_> = s.rules.technologies.ids().collect();
    s.world.civs[0].techs.extend(techs);
    let temple = s.rules.buildings.id("Temple of Artemis").unwrap();
    let sites = |s: &Session| {
        crate::queries::city_options(&s.world, &s.rules, CivId(0), city.index(), city.generation())
            .unwrap()
            .choices
            .into_iter()
            .find(|c| c.construction == ConstructionRef::Building { id: temple.index() })
            .unwrap()
            .sites
            .unwrap()
    };
    let before = sites(&s);
    assert!(before.best.is_some());
    assert_eq!(before.best, before.selected, "unchosen, the engine's pick stands");
    let other = *before.tiles.iter().find(|t| Some(**t) != before.best).unwrap();
    let s = apply(s, 0, "site", Action::ChooseSite { city: city_ref(city), building: temple.index(), tile: other });
    let after = sites(&s);
    assert_eq!(after.selected, Some(other));
    assert_eq!(after.best, before.best, "the suggestion stays the engine's");
}

fn events_for(t: &CommandTransition, seat: u8) -> Vec<crate::view::PresentationEvent> {
    t.updates
        .iter()
        .filter(|(s, _)| *s == seat)
        .flat_map(|(_, u)| u.events.iter().map(|e| e.event.event.clone()))
        .collect()
}
fn solo() -> Session {
    Session::new(Settings { majors: 2, city_states: 1, ..Settings::default() }, "solo".into(), vec![0]).unwrap()
}
fn reveal_to(s: &mut Session, seat: u8) {
    for t in s.world.grid.ids().collect::<Vec<_>>() {
        s.world.visibility.observe(CivId(seat), t);
    }
    civ_core::world::fog::capture_observations(&mut s.world, &s.rules);
    s.refresh(&[], None).unwrap();
}

#[test]
fn a_demand_waits_in_the_view_until_answered_and_the_promise_follows() {
    use civ_core::world::diplomacy::Demand as Core;
    let mut s = settle_capital(ai_session(), 0);
    civ_core::world::promises::demand(&mut s.world, CivId(1), CivId(0), Core::DoNotSettleNearUs).unwrap();
    s.refresh(&[], None).unwrap();
    let turn = s.turn();
    let view = s.current_update(0).unwrap().view;
    assert_eq!(
        view.demands,
        Some(vec![crate::view::DemandView {
            by: 1,
            demand: Demand::DoNotSettleNearUs,
            promise_until: turn + civ_core::world::promises::PROMISE_TURNS - 1,
            refusal_declares_war: false,
            may_refuse: true,
        }])
    );
    assert!(view.promises.is_none());
    assert!(s.current_update(2).unwrap().view.demands.is_none(), "only the one asked sees it");
    let t = s
        .stage_command(
            0,
            request(&s, 0, "agree", Action::AnswerDemand { by: 1, demand: Demand::DoNotSettleNearUs, agree: true }),
        )
        .unwrap();
    assert_eq!(t.receipt.result, CommandStatus::Applied);
    assert!(events_for(&t, 0).contains(&crate::view::PresentationEvent::DemandAnswered {
        by: 0,
        to: 1,
        demand: Demand::DoNotSettleNearUs,
        agreed: true,
    }));
    let view = t.next.current_update(0).unwrap().view;
    assert!(view.demands.is_none());
    assert_eq!(
        view.promises,
        Some(vec![crate::view::PromiseView {
            by: 0,
            to: 1,
            demand: Demand::DoNotSettleNearUs,
            until: turn + civ_core::world::promises::PROMISE_TURNS - 1,
        }])
    );

    // An AI's demand arrives as an event for the one asked.
    let made = crate::view::report_for(
        &s.world,
        &s.world,
        &s.rules,
        CivId(0),
        &[civ_core::world::TurnEvent::DemandMade { by: CivId(1), of: CivId(0), demand: Core::DontSpyOnUs }],
    );
    assert_eq!(
        made.events.into_iter().map(|e| e.event).collect::<Vec<_>>(),
        vec![crate::view::PresentationEvent::DemandMade { by: 1, of: 0, demand: Demand::DontSpyOnUs }]
    );
    // A refusal that means war says so, and is refused while a treaty forbids war.
    civ_core::world::promises::demand(&mut s.world, CivId(1), CivId(0), Core::DoNotAttackUs).unwrap();
    let asked = crate::view::player_view(&s.world, &s.rules, CivId(0)).demands.unwrap();
    let war = asked.iter().find(|d| d.demand == Demand::DoNotAttackUs).unwrap();
    assert!(war.refusal_declares_war);
    assert_eq!(war.may_refuse, s.world.diplomacy.can_declare_war(CivId(0), CivId(1)));
}

#[test]
fn every_update_says_whether_the_last_command_can_be_taken_back() {
    let mut s = Session::new(Settings { majors: 2, city_states: 0, ..Settings::default() }, "undo-state".into(), vec![0])
        .unwrap();
    let state = |s: &Session| s.current_update(0).unwrap().undo.unwrap();
    assert_eq!(state(&s), UndoState { available: false, refusal: Some(UndoRefusal::NothingToUndo) });
    let (id, warrior) = warrior_of(&s, 0);
    let to = covered_step(&mut s, 0, id);
    let t = s.stage_command(0, request(&s, 0, "step", Action::MoveUnit { unit: warrior, to })).unwrap();
    let (_, update) = t.updates.iter().find(|(seat, _)| *seat == 0).unwrap();
    assert_eq!(update.undo, Some(UndoState { available: true, refusal: None }));
    let s = next_round(t.next);
    assert_eq!(state(&s), UndoState { available: false, refusal: Some(UndoRefusal::TurnEnded) });

    // In a room only a move may be taken back.
    let mut room = session();
    let tech = room.rules.technologies.iter().find(|(id, _)| !room.world.civ(CivId(0)).unwrap().has_tech(*id)).unwrap().0;
    room = apply(room, 0, "research", Action::SetResearch { technology: tech.index() });
    assert_eq!(state(&room), UndoState { available: false, refusal: Some(UndoRefusal::NotAllowed) });
}

#[test]
fn unit_options_list_mission_targets_and_a_strike_is_previewed_from_what_is_seen() {
    let mut s = settle_capital(solo(), 0);
    clear_barbarians(&mut s);
    reveal_to(&mut s, 0);
    let capital = s.world.civs[0].cities[0];
    let centre = s.world.cities.get(capital).unwrap().centre;
    let base = s.rules.units.id("Atomic Bomb").unwrap();
    let bomb = s.world.place_unit(civ_core::world::Unit::new(base, CivId(0), centre, 1));
    let bomb_ref = EntityRef { id: bomb.index(), gen: bomb.generation() };
    s.refresh(&[], None).unwrap();
    let targets = |s: &Session| {
        let reply = s.query(0, Query::UnitOptions { unit: bomb_ref, nuclear_target: None }).unwrap();
        let QueryResult::UnitOptions { options: Some(options) } = reply.result else { panic!("options") };
        options.targets.unwrap_or_default()
    };
    let strike = targets(&s).into_iter().find(|t| t.action == crate::queries::UnitActionKind::NuclearStrike).unwrap();
    assert!(!strike.tiles.is_empty());
    let range = civ_core::world::air::attack_range(&s.world, &s.rules, bomb) as u32;
    assert!(strike.tiles.iter().all(|t| s.world.grid.distance(centre, TileId(*t)) <= range && TileId(*t) != centre));

    // A rival's unit, seen near the capital but not yet met: striking it would
    // declare war on a nation this player does not know, which is refused.
    let (theirs, _) = s.world.units.iter().find(|(_, u)| u.owner == CivId(1)).unwrap();
    let near = s.world.grid.within(centre, 3).into_iter().find(|t| s.world.grid.distance(centre, *t) == 3).unwrap();
    s.world.units.get_mut(theirs).unwrap().position = near;
    assert!(!s.world.diplomacy.have_met(CivId(0), CivId(1)));
    let preview = |s: &Session| {
        let reply = s.query(0, Query::UnitOptions { unit: bomb_ref, nuclear_target: Some(near.0) }).unwrap();
        let QueryResult::UnitOptions { options: Some(options) } = reply.result else { panic!("options") };
        options.nuclear_preview.expect("preview")
    };
    let p = preview(&s);
    assert_eq!(p.victims, vec![1]);
    assert!(!p.legal);
    assert_eq!(p.intercept_chance, 0);
    assert_eq!(p.tiles.len(), s.world.grid.within(near, p.blast_radius).len());
    assert!(!targets(&s).iter().any(|t| t.tiles.contains(&near.0)));
    s.world.diplomacy.meet(CivId(0), CivId(1));
    assert!(preview(&s).legal);
    assert!(targets(&s).iter().any(|t| t.tiles.contains(&near.0)));
    // Nobody else's in the blast: nobody to declare war on.
    let own = s.world.grid.within(centre, 1).into_iter().find(|t| *t != centre).unwrap();
    let reply = s.query(0, Query::UnitOptions { unit: bomb_ref, nuclear_target: Some(own.0) }).unwrap();
    let QueryResult::UnitOptions { options: Some(options) } = reply.result else { panic!("options") };
    let quiet = options.nuclear_preview.expect("preview");
    assert!(quiet.victims.is_empty() || quiet.victims == vec![1], "{quiet:?}");
}

#[test]
fn a_city_state_says_what_the_player_may_do_there() {
    let mut s = settle_capital(solo(), 0);
    let cs = s.world.civs.iter().find(|c| c.is_city_state()).unwrap().id;
    s.world.diplomacy.meet(CivId(0), cs);
    s.refresh(&[], None).unwrap();
    let relation = |s: &Session| s.current_update(0).unwrap().view.city_states.into_iter().find(|c| c.civ == cs.0).unwrap();
    let before = relation(&s);
    assert!(before.may_pledge && !before.may_withdraw && !before.may_coup && before.coup_chance.is_none());
    assert_eq!(before.resting_point, civ_core::world::city_states::resting_point_in(&s.world, &s.rules, cs, CivId(0)).to_string());
    assert!(before.tribute_amount > 0);
    let s = apply(s, 0, "pledge", Action::PledgeProtection { target: cs.0 });
    let after = relation(&s);
    assert!(!after.may_pledge && !after.may_withdraw, "a pledge stands for ten turns");
    assert_ne!(after.resting_point, before.resting_point, "a protector's influence rests higher");
    let t = crate::tables(&s.rules).influence_thresholds.unwrap();
    assert_eq!((t.friend, t.ally, t.unforgivable), (30, 60, -30));
}

#[test]
fn a_spy_shows_what_the_theft_costs_and_promises_forbid_spying() {
    use civ_core::world::diplomacy::{Demand as Core, Flag};
    use civ_core::world::espionage::{Spy, SpyAction};
    let mut s = session();
    for seat in [0, 1] {
        s = found_capital(s, seat);
    }
    let theirs = s.world.civs[1].cities[0];
    let tech = s
        .rules
        .technologies
        .ids()
        .find(|t| civ_core::world::research::can_research(&s.rules, s.world.civ(CivId(0)).unwrap(), *t).is_ok())
        .unwrap();
    s.world.civ_mut(CivId(1)).unwrap().techs.insert(tech);
    s.world.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy {
        rank: 1,
        station: Some(theirs),
        action: SpyAction::StealingTech,
        turns_remaining: 4,
        progress: 12,
    });
    s.world.diplomacy.meet(CivId(0), CivId(1));
    s.world.diplomacy.set_flag(CivId(1), CivId(0), Flag::Agreed(Core::DontSpyOnUs), 30);
    s.refresh(&[], None).unwrap();
    let view = s.current_update(0).unwrap().view;
    let speed = TurnModifiers::for_world(&s.world, &s.rules).science.speed;
    let stealable = civ_core::world::espionage::stealable(&s.world, &s.rules, CivId(0), CivId(1));
    assert_eq!(view.spies[0].steal_cost, Some(civ_core::world::espionage::steal_cost(&s.rules, &stealable, speed)));
    assert_eq!(view.spying_forbidden, Some(vec![1]));
    let promise = view.promises.unwrap();
    assert_eq!(promise.len(), 1);
    assert_eq!((promise[0].by, promise[0].to, promise[0].until), (0, 1, s.turn() + 29));
}

#[test]
fn a_great_person_lists_every_ability_with_why_it_cannot_be_used_here() {
    use crate::queries::GreatPersonRefusal;
    let mut s = settle_capital(solo(), 0);
    let scholar = add_unit(&mut s, 0, "Scholar");
    s.refresh(&[], None).unwrap();
    let abilities = |s: &Session| {
        let reply = s.query(0, Query::UnitOptions { unit: scholar, nuclear_target: None }).unwrap();
        let QueryResult::UnitOptions { options: Some(options) } = reply.result else { panic!("options") };
        (options.great_person_abilities.unwrap(), options.great_person_actions)
    };
    let (list, legal) = abilities(&s);
    let hurry = list.iter().find(|a| a.action == GreatPersonAction::HurryResearch).unwrap();
    assert!(!hurry.available);
    assert_eq!(hurry.why, Some(GreatPersonRefusal::NothingResearched));
    assert!(list.iter().any(|a| matches!(a.action, GreatPersonAction::Construct { .. })));
    for a in &list {
        assert_eq!(a.available, legal.contains(&a.action), "{a:?}");
        assert_eq!(a.available, a.why.is_none());
    }
    let tech = s.rules.technologies.iter().find(|(id, _)| !s.world.civ(CivId(0)).unwrap().has_tech(*id)).unwrap().0;
    let s = apply(s, 0, "research", Action::SetResearch { technology: tech.index() });
    let (list, _) = abilities(&s);
    let hurry = list.iter().find(|a| a.action == GreatPersonAction::HurryResearch).unwrap();
    assert!(hurry.available && hurry.why.is_none() && hurry.amount.is_some());
}

#[test]
fn city_state_and_city_events_reach_the_player() {
    use crate::view::PresentationEvent as P;
    let mut s = settle_capital(solo(), 0);
    let cs = s.world.civs.iter().find(|c| c.is_city_state()).unwrap().id;
    s.world.diplomacy.meet(CivId(0), cs);
    s.world.civs[0].gold = 500;
    s.refresh(&[], None).unwrap();
    let t = s.stage_command(0, request(&s, 0, "gift", Action::GiftGold { target: cs.0, gold: 250 })).unwrap();
    assert!(events_for(&t, 0).iter().any(|e| matches!(e, P::GoldGifted { city_state, gold: 250, .. } if *city_state == cs.0)));
    let s = t.next;
    let capital = s.world.civs[0].cities[0];
    let mut after = s.world.clone();
    let base = s.rules.units.id("Scholar").unwrap();
    let centre = after.cities.get(capital).unwrap().centre;
    let born = after.place_unit(civ_core::world::Unit::new(base, CivId(0), centre, 2));
    let monument = s.rules.buildings.id("Monument").unwrap();
    let report = crate::view::report_for(
        &s.world,
        &after,
        &s.rules,
        CivId(0),
        &[
            civ_core::world::TurnEvent::GreatPersonBorn { civ: CivId(0), kind: "Scholar".into() },
            civ_core::world::TurnEvent::CityCompleted { city: capital, what: Construction::Building(monument) },
            civ_core::world::TurnEvent::TributePaid { city_state: cs, to: CivId(0), gold: 60, worker: false },
            civ_core::world::TurnEvent::RebelSpawned { civ: CivId(0), unit: born },
        ],
    );
    let events: Vec<_> = report.events.into_iter().map(|e| e.event).collect();
    assert!(events.contains(&P::GreatPersonBorn { name: "Scholar".into(), unit: Some(born.into()) }));
    assert!(events.contains(&P::CityCompleted {
        city: capital.into(),
        construction: ConstructionRef::Building { id: monument.index() }
    }));
    assert!(events.contains(&P::TributePaid { city_state: cs.0, gold: 60, worker: false }));
    assert!(events.contains(&P::RebelsSpawned { tile: Some(centre.0), unit: Some(born.into()) }));
    // Eras reached, from the first.
    assert_eq!(s.current_update(0).unwrap().view.era_starts, Some(vec![0]));
}

#[test]
fn a_room_sees_every_seats_readiness_and_connection() {
    let s = session();
    let seats = |s: &Session, seat: u8| s.current_update(seat).unwrap().seats.unwrap();
    assert_eq!(
        seats(&s, 0),
        vec![
            SeatStatusView { seat: 0, ready: false, controlled: true, connected: None, name: None, left: None },
            SeatStatusView { seat: 1, ready: false, controlled: true, connected: None, name: None, left: None },
        ]
    );
    let t = s.stage_command(1, request(&s, 1, "done", Action::FinishTurn)).unwrap();
    assert!(t.updates.iter().any(|(seat, u)| *seat == 0 && u.seats.as_ref().unwrap()[1].ready), "the other seat hears");
    let t = t.next.stage_presence(1, true).unwrap();
    let (_, update) = t.updates.iter().find(|(seat, _)| *seat == 0).unwrap();
    assert_eq!(update.seats.as_ref().unwrap()[1].connected, Some(true));
    let s = t.next;
    s.verify_replay().unwrap();
    let restored = Session::restore(&s.save().unwrap()).unwrap();
    assert_eq!(seats(&restored, 0), seats(&s, 0));
    // Solo play has no room to show.
    assert!(solo().current_update(0).unwrap().seats.is_none());
}
