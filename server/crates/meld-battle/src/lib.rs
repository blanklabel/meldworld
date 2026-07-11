//! Server-authoritative ATB engine (CANON.md §B, behaviors/combat-atb.md).
//!
//! One [`Battle`] is a pure state machine: [`Battle::tick`] advances gauges on
//! the 100 ms cadence and resolves monster/timeout actions; [`Battle::submit`]
//! resolves a player action. Both return engine [`Event`]s that `meld-server`
//! maps onto `battle.*` wire messages. No wall-clock, no RNG globals, no I/O —
//! so it is fully deterministic and unit-testable (BUILD-PLAN M2.3/M2.4).

use meld_balance::Balance;
use meld_proto::common::Combatant as WireCombatant;
use meld_proto::enums::{
    BattleActionKind, BattleOutcome, CombatantKind, EffectKind, EncounterClass,
};
use meld_proto::Id;

/// A combatant inside a battle. `atk`/`def`/`max_hp` are already world-scaled
/// (stat_mult applied at spawn — no mid-fight rescale, combat-atb.md invariant 4).
#[derive(Debug, Clone)]
pub struct Fighter {
    pub combatant_id: Id,
    pub kind: CombatantKind,
    pub player_id: Option<Id>,
    pub monster_kind: Option<String>,
    pub level: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub atk: i32,
    pub def: i32,
    pub speed_stat: i32,
    pub gauge: f64,
    pub statuses: Vec<String>,
    /// True while a `defend` stance is active (until this fighter next acts).
    pub defending: bool,
    /// True once the gauge is full and we are waiting on this player's input.
    awaiting: bool,
    /// Engine tick at which the turn became ready (for the 15 s timeout).
    ready_tick: u64,
    alive: bool,
}

impl Fighter {
    /// Build a fresh fighter (gauge 0, alive iff `hp > 0`). Stats are already
    /// world-scaled by the caller (no mid-fight rescale).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        combatant_id: Id,
        kind: CombatantKind,
        player_id: Option<Id>,
        monster_kind: Option<String>,
        level: i32,
        hp: i32,
        atk: i32,
        def: i32,
        speed_stat: i32,
    ) -> Self {
        Fighter {
            combatant_id,
            kind,
            player_id,
            monster_kind,
            level,
            hp,
            max_hp: hp,
            atk,
            def,
            speed_stat,
            gauge: 0.0,
            statuses: Vec::new(),
            defending: false,
            awaiting: false,
            ready_tick: 0,
            alive: hp > 0,
        }
    }

    fn to_wire(&self) -> WireCombatant {
        WireCombatant {
            combatant_id: self.combatant_id.clone(),
            kind: self.kind,
            player_id: self.player_id.clone(),
            monster_kind: self.monster_kind.clone(),
            level: self.level,
            hp: self.hp,
            max_hp: self.max_hp,
            gauge: self.gauge,
            statuses: self.statuses.clone(),
        }
    }
}

/// One resolved effect on a target (maps to `battle.action_resolved.effects[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEffect {
    pub target_id: Id,
    pub kind: EffectKind,
    pub amount: Option<i32>,
    pub status: Option<String>,
    pub hp_after: i32,
}

/// The outcome of resolving a single action (maps to `battle.action_resolved`).
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub action_id: Option<Id>,
    pub actor_id: Id,
    pub action: BattleActionKind,
    pub auto: bool,
    pub flee_success: Option<bool>,
    pub effects: Vec<ResolvedEffect>,
}

/// Engine events emitted by `tick`/`submit`, in resolution order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A player combatant's gauge filled; their action window opens.
    TurnReady { combatant_id: Id },
    /// An action resolved (player, monster AI, or auto-defend).
    Resolved(Resolution),
    /// The battle reached a terminal state (spike: single party vs enemies).
    Ended { outcome: BattleOutcome },
}

/// Why a `submit` was rejected (server maps to a `session.error` code).
#[derive(Debug, Clone, PartialEq)]
pub enum Reject {
    NotFound,
    DuplicateAction,
    InvalidState(&'static str),
    ValidationError(&'static str),
}

pub struct Battle {
    pub battle_id: Id,
    pub encounter_class: EncounterClass,
    fighters: Vec<Fighter>,
    tick_count: u64,
    ended: bool,
    // Tunables snapshot (structural formulas in code; coefficients from balance).
    gauge_divisor: f64,
    timeout_ticks: u64,
    defend_reduction: f64,
    min_damage: i32,
    flee_base: f64,
    flee_penalty_per_tier: f64,
    flee_floor: f64,
    seen_actions: Vec<Id>,
    /// Tiny deterministic LCG for flee rolls (no global RNG — determinism).
    rng: u64,
}

impl Battle {
    /// Build a battle from ally + enemy fighters. `seed` drives flee rolls.
    pub fn new(
        battle_id: Id,
        encounter_class: EncounterClass,
        allies: Vec<Fighter>,
        enemies: Vec<Fighter>,
        balance: &Balance,
        seed: u64,
    ) -> Self {
        let tick_ms = balance.battle.tick_ms.max(1);
        let mut fighters = allies;
        fighters.extend(enemies);
        for f in &mut fighters {
            f.alive = f.hp > 0;
        }
        Battle {
            battle_id,
            encounter_class,
            fighters,
            tick_count: 0,
            ended: false,
            gauge_divisor: balance.battle.gauge_fill_divisor,
            timeout_ticks: (balance.battle.turn_timeout_ms / tick_ms).max(1),
            defend_reduction: balance.battle.defend_damage_reduction,
            min_damage: balance.combat_math.min_damage,
            flee_base: balance.battle.flee_base,
            flee_penalty_per_tier: balance.battle.flee_penalty_per_tier,
            flee_floor: balance.battle.flee_floor,
            seen_actions: Vec::new(),
            rng: seed | 1,
        }
    }

    pub fn is_over(&self) -> bool {
        self.ended
    }

    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    /// Snapshot of all fighters as wire combatants (for `battle.started`).
    pub fn wire_combatants(&self) -> (Vec<WireCombatant>, Vec<WireCombatant>) {
        let allies = self
            .fighters
            .iter()
            .filter(|f| f.kind == CombatantKind::Player)
            .map(Fighter::to_wire)
            .collect();
        let enemies = self
            .fighters
            .iter()
            .filter(|f| f.kind != CombatantKind::Player)
            .map(Fighter::to_wire)
            .collect();
        (allies, enemies)
    }

    /// Per-combatant gauge/HP state (for `battle.gauge_update`).
    pub fn gauge_state(&self) -> Vec<(Id, f64, i32, Vec<String>)> {
        self.fighters
            .iter()
            .map(|f| (f.combatant_id.clone(), f.gauge, f.hp, f.statuses.clone()))
            .collect()
    }

    pub fn living_player_ids(&self) -> Vec<Id> {
        self.fighters
            .iter()
            .filter(|f| f.alive && f.kind == CombatantKind::Player)
            .filter_map(|f| f.player_id.clone())
            .collect()
    }

    fn idx(&self, combatant_id: &str) -> Option<usize> {
        self.fighters
            .iter()
            .position(|f| f.combatant_id == combatant_id)
    }

    fn any_enemy_alive(&self) -> bool {
        self.fighters
            .iter()
            .any(|f| f.alive && f.kind != CombatantKind::Player)
    }

    fn any_player_alive(&self) -> bool {
        self.fighters
            .iter()
            .any(|f| f.alive && f.kind == CombatantKind::Player)
    }

    fn first_living_player(&self) -> Option<usize> {
        self.fighters
            .iter()
            .position(|f| f.alive && f.kind == CombatantKind::Player)
    }

    /// Advance the battle one 100 ms tick. Fills gauges, fires monster turns and
    /// 15 s auto-defends, and reports the terminal outcome once reached.
    pub fn tick(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if self.ended {
            return events;
        }
        self.tick_count += 1;

        // 1. Fill gauges for living fighters not already awaiting input.
        let n = self.fighters.len();
        for i in 0..n {
            let f = &mut self.fighters[i];
            if !f.alive || f.awaiting || f.gauge >= 1.0 {
                continue;
            }
            f.gauge = (f.gauge + f.speed_stat as f64 / self.gauge_divisor).min(1.0);
        }

        // 2. Resolve full gauges. Monsters act immediately; players get a window.
        for i in 0..n {
            if self.ended {
                break;
            }
            let (alive, full, awaiting, is_player) = {
                let f = &self.fighters[i];
                (
                    f.alive,
                    f.gauge >= 1.0,
                    f.awaiting,
                    f.kind == CombatantKind::Player,
                )
            };
            if !alive || !full {
                continue;
            }
            if is_player {
                if !awaiting {
                    self.fighters[i].awaiting = true;
                    self.fighters[i].ready_tick = self.tick_count;
                    events.push(Event::TurnReady {
                        combatant_id: self.fighters[i].combatant_id.clone(),
                    });
                } else if self.tick_count.saturating_sub(self.fighters[i].ready_tick)
                    >= self.timeout_ticks
                {
                    // 15 s elapsed with no action → auto-defend.
                    let res = self.resolve_defend(i, None, true);
                    events.push(Event::Resolved(res));
                    self.check_terminal(&mut events);
                }
            } else {
                // Monster AI: attack the first living player.
                if let Some(res) = self.resolve_monster_turn(i) {
                    events.push(Event::Resolved(res));
                    self.check_terminal(&mut events);
                }
            }
        }
        events
    }

    /// Resolve a player-submitted action. Returns the events or a rejection.
    pub fn submit(
        &mut self,
        actor_combatant_id: &str,
        action_id: Id,
        action: BattleActionKind,
        target_ids: Option<Vec<Id>>,
    ) -> Result<Vec<Event>, Reject> {
        if self.ended {
            return Err(Reject::InvalidState("Battle has ended."));
        }
        let i = self.idx(actor_combatant_id).ok_or(Reject::NotFound)?;
        if self.fighters[i].kind != CombatantKind::Player || !self.fighters[i].alive {
            return Err(Reject::NotFound);
        }
        if self.seen_actions.iter().any(|a| a == &action_id) {
            return Err(Reject::DuplicateAction);
        }
        if !self.fighters[i].awaiting || self.fighters[i].gauge < 1.0 {
            return Err(Reject::InvalidState("Actor gauge is not full."));
        }
        if action == BattleActionKind::Flee && self.encounter_class == EncounterClass::Gatekeeper {
            return Err(Reject::InvalidState(
                "Flee is disabled against Gatekeepers.",
            ));
        }
        self.seen_actions.push(action_id.clone());

        let mut events = Vec::new();
        let res = match action {
            BattleActionKind::Attack => {
                let target = target_ids
                    .as_ref()
                    .and_then(|t| t.first())
                    .ok_or(Reject::ValidationError("attack requires target_ids"))?;
                self.resolve_attack(i, target, Some(action_id))?
            }
            BattleActionKind::Defend => self.resolve_defend(i, Some(action_id), false),
            BattleActionKind::Flee => self.resolve_flee(i, Some(action_id)),
            BattleActionKind::Skill | BattleActionKind::Item => {
                // Skills/items are content-driven — out of the today-slice.
                return Err(Reject::ValidationError(
                    "skill/item not implemented in v0.1 slice",
                ));
            }
        };
        let fled = res.flee_success == Some(true);
        events.push(Event::Resolved(res));
        if fled {
            self.ended = true;
            events.push(Event::Ended {
                outcome: BattleOutcome::Fled,
            });
        } else {
            self.check_terminal(&mut events);
        }
        Ok(events)
    }

    // --- resolution helpers -------------------------------------------------

    /// Damage after defense and an optional defend stance. Structural formula;
    /// coefficients (`min_damage`, `defend_reduction`) are tunables.
    fn damage(&self, atk: i32, def: i32, target_defending: bool) -> i32 {
        let mut raw = (atk - def) as f64;
        if target_defending {
            raw *= 1.0 - self.defend_reduction;
        }
        (raw.round() as i32).max(self.min_damage)
    }

    fn resolve_attack(
        &mut self,
        actor_i: usize,
        target_id: &str,
        action_id: Option<Id>,
    ) -> Result<Resolution, Reject> {
        let target_i = match self.idx(target_id) {
            Some(t) if self.fighters[t].alive => t,
            // Target died between submit and resolve → retarget to next enemy
            // for a player, or drop. Spike: retarget to first living enemy.
            _ => self
                .fighters
                .iter()
                .position(|f| f.alive && f.kind != CombatantKind::Player)
                .ok_or(Reject::NotFound)?,
        };
        let atk = self.fighters[actor_i].atk;
        let def = self.fighters[target_i].def;
        let defending = self.fighters[target_i].defending;
        let dmg = self.damage(atk, def, defending);

        let effects = self.apply_damage(target_i, dmg);
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        Ok(Resolution {
            action_id,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Attack,
            auto: false,
            flee_success: None,
            effects,
        })
    }

    fn resolve_defend(&mut self, actor_i: usize, action_id: Option<Id>, auto: bool) -> Resolution {
        self.fighters[actor_i].defending = true;
        self.reset_gauge(actor_i);
        Resolution {
            action_id,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Defend,
            auto,
            flee_success: None,
            effects: Vec::new(),
        }
    }

    fn resolve_flee(&mut self, actor_i: usize, action_id: Option<Id>) -> Resolution {
        // combat-atb.md flee formula. Spike: single Center-Hub-Forest party, so
        // the encounter-above-party tier gap is 0; the full multi-tier gap lands
        // with deeper encounters.
        let tier_gap = 0;
        let chance = self.flee_chance(tier_gap);
        let roll = self.next_rand_unit();
        let success = roll < chance;
        self.reset_gauge(actor_i);
        if success {
            for f in &mut self.fighters {
                if f.kind == CombatantKind::Player {
                    f.alive = false; // leaves the battle
                }
            }
        }
        Resolution {
            action_id,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Flee,
            auto: false,
            flee_success: Some(success),
            effects: Vec::new(),
        }
    }

    fn resolve_monster_turn(&mut self, actor_i: usize) -> Option<Resolution> {
        let target_i = self.first_living_player()?;
        let atk = self.fighters[actor_i].atk;
        let def = self.fighters[target_i].def;
        let defending = self.fighters[target_i].defending;
        let dmg = self.damage(atk, def, defending);
        let effects = self.apply_damage(target_i, dmg);
        self.reset_gauge(actor_i);
        Some(Resolution {
            action_id: None,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Attack,
            auto: false,
            flee_success: None,
            effects,
        })
    }

    fn apply_damage(&mut self, target_i: usize, dmg: i32) -> Vec<ResolvedEffect> {
        let t = &mut self.fighters[target_i];
        t.hp = (t.hp - dmg).max(0);
        let dead = t.hp == 0;
        if dead {
            t.alive = false;
        }
        let mut effects = vec![ResolvedEffect {
            target_id: t.combatant_id.clone(),
            kind: EffectKind::Damage,
            amount: Some(dmg),
            status: None,
            hp_after: t.hp,
        }];
        if dead {
            effects.push(ResolvedEffect {
                target_id: self.fighters[target_i].combatant_id.clone(),
                kind: EffectKind::Ko,
                amount: None,
                status: None,
                hp_after: 0,
            });
        }
        effects
    }

    fn reset_gauge(&mut self, i: usize) {
        self.fighters[i].gauge = 0.0;
        self.fighters[i].awaiting = false;
    }

    fn check_terminal(&mut self, events: &mut Vec<Event>) {
        if self.ended {
            return;
        }
        if !self.any_enemy_alive() {
            self.ended = true;
            events.push(Event::Ended {
                outcome: BattleOutcome::Victory,
            });
        } else if !self.any_player_alive() {
            self.ended = true;
            events.push(Event::Ended {
                outcome: BattleOutcome::Defeat,
            });
        }
    }

    /// Flee chance (combat-atb.md): `base − penalty·max(0, tier_gap)`, floored.
    /// Structure in code; coefficients from balance.
    fn flee_chance(&self, tier_gap: i32) -> f64 {
        let raw = self.flee_base - self.flee_penalty_per_tier * tier_gap.max(0) as f64;
        raw.max(self.flee_floor)
    }

    fn next_rand_unit(&mut self) -> f64 {
        // Numerical Recipes LCG — deterministic per seed.
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.rng >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balance() -> Balance {
        Balance::load_default().unwrap()
    }

    fn player(id: &str, speed: i32) -> Fighter {
        Fighter {
            combatant_id: id.to_string(),
            kind: CombatantKind::Player,
            player_id: Some(format!("p-{id}")),
            monster_kind: None,
            level: 1,
            hp: 40,
            max_hp: 40,
            atk: 12,
            def: 3,
            speed_stat: speed,
            gauge: 0.0,
            statuses: vec![],
            defending: false,
            awaiting: false,
            ready_tick: 0,
            alive: true,
        }
    }

    fn monster(id: &str, hp: i32, speed: i32) -> Fighter {
        Fighter {
            combatant_id: id.to_string(),
            kind: CombatantKind::Monster,
            player_id: None,
            monster_kind: Some("forest_bloom_stalker".into()),
            level: 1,
            hp,
            max_hp: hp,
            atk: 14,
            def: 4,
            speed_stat: speed,
            gauge: 0.0,
            statuses: vec![],
            defending: false,
            awaiting: false,
            ready_tick: 0,
            alive: true,
        }
    }

    #[test]
    fn player_gauge_fills_and_turn_becomes_ready() {
        let b = balance();
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("a", 110)],
            vec![monster("m", 1000, 1)], // slow monster so it doesn't act
            &b,
            7,
        );
        // speed 110 / 400 = 0.275 per tick → full at tick 4.
        let mut ready = false;
        for _ in 0..5 {
            for ev in battle.tick() {
                if let Event::TurnReady { combatant_id } = ev {
                    assert_eq!(combatant_id, "a");
                    ready = true;
                }
            }
        }
        assert!(ready, "player turn should become ready within 5 ticks");
    }

    #[test]
    fn attack_damages_and_kills_monster() {
        let b = balance();
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("a", 400)], // fills in one tick
            vec![monster("m", 10, 1)],
            &b,
            7,
        );
        // Drive: tick to ready, then attack until dead.
        let mut outcome = None;
        for _ in 0..50 {
            for ev in battle.tick() {
                if let Event::TurnReady { combatant_id } = ev {
                    let evs = battle
                        .submit(
                            &combatant_id,
                            format!("act-{}", battle.tick_count()),
                            BattleActionKind::Attack,
                            Some(vec!["m".into()]),
                        )
                        .unwrap();
                    for e in evs {
                        if let Event::Ended { outcome: o } = e {
                            outcome = Some(o);
                        }
                    }
                }
            }
            if battle.is_over() {
                break;
            }
        }
        assert_eq!(outcome, Some(BattleOutcome::Victory));
    }

    #[test]
    fn timeout_triggers_auto_defend() {
        let b = balance();
        // timeout_ticks = 15000/100 = 150.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("a", 400)],
            vec![monster("m", 100000, 1)],
            &b,
            7,
        );
        let mut auto_defend_seen = false;
        for _ in 0..200 {
            for ev in battle.tick() {
                if let Event::Resolved(r) = ev {
                    if r.auto && r.action == BattleActionKind::Defend && r.actor_id == "a" {
                        auto_defend_seen = true;
                    }
                }
            }
            if auto_defend_seen {
                break;
            }
        }
        assert!(auto_defend_seen, "AFK player should auto-defend after 15s");
    }

    #[test]
    fn duplicate_action_id_rejected() {
        let b = balance();
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("a", 400)],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        battle.tick(); // fill + ready
        let first = battle.submit(
            "a",
            "dup".into(),
            BattleActionKind::Attack,
            Some(vec!["m".into()]),
        );
        assert!(first.is_ok());
        // Re-ready and resubmit the same action_id.
        for _ in 0..3 {
            battle.tick();
        }
        let second = battle.submit(
            "a",
            "dup".into(),
            BattleActionKind::Attack,
            Some(vec!["m".into()]),
        );
        assert_eq!(second, Err(Reject::DuplicateAction));
    }
}
