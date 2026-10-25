// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! City-state information deliberately exposed to the requesting major.
use civ_core::{
    ruleset::Ruleset,
    world::{city_states, quests, CivId, World},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CityStateRelationsView {
    pub civ: u8,
    pub kind: String,
    pub personality: String,
    pub influence: String,
    pub relationship: String,
    /// The ally is identified only once this player has met it.
    pub known_ally: Option<u8>,
    pub has_ally: bool,
    pub allied_turns: u32,
    pub unique_luxury: Option<u32>,
    pub unique_unit: Option<u32>,
    /// A marriage price includes units outside the player's sight. It is
    /// confirmed by the action boundary, never quoted from hidden state.
    pub may_request_marriage: bool,
    pub quests: Vec<QuestView>,
    pub war_assistance: Vec<WarAssistanceView>,
    /// `pledgeProtection` would be accepted now: at peace, influence of at
    /// least zero, and not within twenty turns of withdrawing a pledge.
    #[serde(default)]
    pub may_pledge: bool,
    /// `withdrawProtection` would be accepted now: protecting it, and not
    /// within ten turns of pledging. Withdrawing costs 20 influence.
    #[serde(default)]
    pub may_withdraw: bool,
    /// It would pay tribute in gold now (`demandTribute`, `worker: false`),
    /// for `tributeAmount` gold and 15 influence.
    #[serde(default)]
    pub tribute_gold: bool,
    /// It would hand over a worker now, for 50 influence.
    #[serde(default)]
    pub tribute_worker: bool,
    /// The gold a tribute would bring.
    #[serde(default)]
    pub tribute_amount: i32,
    /// `attemptCoup` would be accepted now: a spy of ours is established in
    /// its capital and we are not its ally.
    #[serde(default)]
    pub may_coup: bool,
    /// Percent chance a coup succeeds, not counting a spy that may be
    /// defending the capital unseen. Present when `mayCoup`.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coup_chance: Option<i32>,
    /// How far influence moves next turn, toward `restingPoint` (a decimal
    /// string; negative while it falls).
    #[serde(default)]
    pub influence_per_turn: String,
    /// Where influence settles if left alone (a decimal string).
    #[serde(default)]
    pub resting_point: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WarAssistanceView {
    pub attacker: u8,
    pub needed: u32,
    pub own_kills: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestView {
    pub name: String,
    pub description: String,
    pub assigned_on: u32,
    pub expires_on: Option<u32>,
    pub influence_reward: i32,
    pub target: QuestTargetView,
    pub own_contest_score: Option<i32>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QuestTargetView {
    CityState,
    Resource { id: u32 },
    Unit { id: u32 },
    Building { id: u32 },
    Civilization { id: u8 },
    Tile { tile: u16 },
    Contest,
    Percent { value: i32 },
}
pub fn for_player(w: &World, r: &Ruleset, me: CivId) -> Vec<CityStateRelationsView> {
    w.civs.iter().filter(|c|c.is_city_state()&&c.is_alive()&&w.diplomacy.have_met(me,c.id)).map(|c| {
        let ally=city_states::ally_of(w,c.id);
        let allied_turns=if c.city_state.tracked_ally==Some(me){c.city_state.allied_turns}else{0};
        let costs=civ_core::world::TurnModifiers::for_world(w,r).costs;
        let may_coup=city_states::can_coup(w,r,me,c.id);
        CityStateRelationsView {
            may_pledge:civ_core::world::administration::may_protect(w,me,c.id,true),
            may_withdraw:civ_core::world::administration::may_protect(w,me,c.id,false),
            tribute_gold:civ_core::world::tribute::may_demand(w,r,c.id,me,false),
            tribute_worker:civ_core::world::tribute::may_demand(w,r,c.id,me,true),
            tribute_amount:civ_core::world::tribute::gold_amount(w,costs.gold_gift_modifier,costs.tribute_interval),
            may_coup,
            coup_chance:may_coup.then(||city_states::visible_coup_chance(w,r,me,c.id)),
            influence_per_turn:city_states::drift_in(w,r,c.id,me).to_string(),
            resting_point:city_states::resting_point_in(w,r,c.id,me).to_string(),
            civ:c.id.0,kind:c.city_state_type.map(|id|r.city_state_types[id].name.clone()).unwrap_or_default(),
            personality:format!("{:?}",c.city_state.personality),influence:w.diplomacy.influence(c.id,me).to_string(),
            relationship:format!("{:?}",city_states::relationship(w,c.id,me)),
            known_ally:ally.filter(|id|*id==me||w.diplomacy.have_met(me,*id)).map(|id|id.0),has_ally:ally.is_some(),allied_turns,
            unique_luxury:c.city_state.unique_luxury.map(|id|id.index()),unique_unit:c.city_state.unique_unit.map(|id|id.index()),
            may_request_marriage:!w.diplomacy.at_war(me,c.id)&&ally==Some(me)&&w.civ(me).is_some_and(|us|us.uniques.matching(civ_core::UniqueType::CityStateCanBeBoughtForGold).any(|u|u.params.first().and_then(|p|p.parse::<u32>().ok()).is_some_and(|n|allied_turns>=n))),
            war_assistance:c.quests.assistance.iter().filter(|(attacker,_)|w.diplomacy.have_met(me,**attacker)).map(|(attacker,assist)|WarAssistanceView {attacker:attacker.0,needed:assist.needed,own_kills:assist.kills.get(&me).copied().unwrap_or(0)}).collect(),
            quests:c.quests.active.iter().filter(|q|q.assignee==me).filter_map(|q| {
                let rule=r.quests.get(q.quest)?;
                let score=q.contest.and_then(|contest|w.civ(me).map(|c|match contest {quests::Contest::Culture=>c.culture_for_contests,quests::Contest::Faith=>c.faith_for_contests,quests::Contest::Technologies=>c.techs.len() as i32}-q.baseline));
                Some(QuestView {name:rule.name.clone(),description:rule.description.clone(),assigned_on:q.assigned_on,expires_on:(q.expires_on!=0).then_some(q.expires_on),influence_reward:rule.influence.unwrap_or(quests::DEFAULT_INFLUENCE),own_contest_score:score,
                    target:match q.target {quests::Target::Self_=>QuestTargetView::CityState,quests::Target::Resource(id)=>QuestTargetView::Resource{id:id.index()},quests::Target::Unit(id)=>QuestTargetView::Unit{id:id.index()},quests::Target::Building(id)=>QuestTargetView::Building{id:id.index()},quests::Target::Civ(id)=>QuestTargetView::Civilization{id:id.0},quests::Target::Tile(tile)=>QuestTargetView::Tile{tile:tile.0},quests::Target::Contest=>QuestTargetView::Contest,quests::Target::Percent(value)=>QuestTargetView::Percent{value}}
                })
            }).collect(),
        }
    }).collect()
}
