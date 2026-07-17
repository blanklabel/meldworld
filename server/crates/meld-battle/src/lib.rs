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

/// One active Psyker Manifestation occupying a Focus slot. `stacks` (1–2) is the
/// reinforcement level; each of the Psyker's turns the Focus fires `stacks` strong.
/// `target_id` is the enemy an offensive Manifestation is aimed at (chosen when it is
/// cast/reinforced); `None`, or a target that has died, falls back to the first living
/// enemy at tick time (and the fallback is written back so it sticks).
#[derive(Debug, Clone, PartialEq)]
pub struct Focus {
    pub kind: String,
    pub stacks: u8,
    pub target_id: Option<Id>,
}

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
    /// The four attributes (Str/Mnd/Dex/Wll). Populated for player heroes from the
    /// class × level growth curve (see `meld-run`); zero for monsters. Derived
    /// stats (`atk`/`max_hp`/`speed_stat`/`spell_power`/`dodge`) already fold these
    /// in — the raw values are carried only to surface them to the client.
    pub str_: i32,
    pub mnd: i32,
    pub dex: i32,
    pub wll: i32,
    /// Mnd-derived power for manifestations/spells (Psyker Foci deal
    /// `spell_power × mult`, not `atk × mult`). Defaults to `atk`.
    pub spell_power: i32,
    /// Dex-derived chance (0.0–1.0) to completely avoid an incoming *physical*
    /// attack (Attack / Power Strike / creature attacks). Psychic manifestations
    /// are unavoidable. Zero unless Dex is above the dodge floor.
    pub dodge: f64,
    pub gauge: f64,
    pub statuses: Vec<String>,
    /// Content key of the fighter's class (`squire`/`psyker`/`resonant`/…), surfaced
    /// to the client so it shows the right per-hero command menu. Empty for monsters.
    pub class_key: String,
    /// Barrier (temp HP): absorbs damage before HP, and decays each of this
    /// fighter's turns. Granted by wards (Psyker Kinetic Aegis, Resonant Ward).
    pub barrier: i32,
    /// Regen: HP restored at the start of each of this fighter's turns (Resonant
    /// innate, or granted by Regen Boon).
    pub regen: i32,
    /// Battle faction — `"player"` for heroes, else the creature's faction. Drives
    /// AI targeting: a fighter attacks the nearest fighter hostile to its faction
    /// (see `meld_proto::factions::battle_hostile`).
    pub faction: String,
    /// Whether this (creature) fighter flees a losing battle.
    pub flees: bool,
    /// Max simultaneous Foci (0 = not a Psyker; Psykers channel instead of the
    /// normal attack/skill kit — see [`Battle::resolve_psyker`]).
    pub focus_max: usize,
    /// Active Manifestations occupying Focus slots (Psyker only).
    pub foci: Vec<Focus>,
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
            str_: 0,
            mnd: 0,
            dex: 0,
            wll: 0,
            spell_power: atk,
            dodge: 0.0,
            gauge: 0.0,
            statuses: Vec::new(),
            class_key: String::new(),
            barrier: 0,
            regen: 0,
            faction: if kind == CombatantKind::Player {
                meld_proto::factions::PLAYER.to_string()
            } else {
                String::new()
            },
            flees: false,
            focus_max: 0,
            foci: Vec::new(),
            defending: false,
            awaiting: false,
            ready_tick: 0,
            alive: hp > 0,
        }
    }

    /// Wire status list — the channel the client reads per-combatant extras from:
    /// `class:<key>` (drives the per-hero command menu), `faction:<f>` (creature
    /// side), `barrier:<n>`, `regen:<n>`, and (Psyker) `focus_slots:<n>` +
    /// `focus:<kind>:<stacks>` per Manifestation.
    fn wire_statuses(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !self.class_key.is_empty() {
            v.push(format!("class:{}", self.class_key));
        }
        if self.kind != CombatantKind::Player && !self.faction.is_empty() {
            v.push(format!("faction:{}", self.faction));
        }
        if self.barrier > 0 {
            v.push(format!("barrier:{}", self.barrier));
        }
        if self.regen > 0 {
            v.push(format!("regen:{}", self.regen));
        }
        if self.focus_max > 0 {
            v.push(format!("focus_slots:{}", self.focus_max));
            for f in &self.foci {
                v.push(format!("focus:{}:{}", f.kind, f.stacks));
            }
        }
        // Attributes for the hero inspect (only heroes carry them; monsters keep 0).
        if self.str_ != 0 || self.mnd != 0 || self.dex != 0 || self.wll != 0 {
            v.push(format!("str:{}", self.str_));
            v.push(format!("mnd:{}", self.mnd));
            v.push(format!("dex:{}", self.dex));
            v.push(format!("wll:{}", self.wll));
        }
        v.extend(self.statuses.iter().cloned());
        v
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
            statuses: self.wire_statuses(),
        }
    }
}

/// Prepend `pre` effects to a resolution so start-of-turn upkeep (Regen/Barrier)
/// is reported before the action's own effects.
fn prepend_effects(res: &mut Resolution, pre: Vec<ResolvedEffect>) {
    if pre.is_empty() {
        return;
    }
    let mut merged = pre;
    merged.extend(std::mem::take(&mut res.effects));
    res.effects = merged;
}

/// The level at which a Manifestation becomes castable (content; structural). A
/// Psyker unlocks more manifestations as it levels.
pub fn manifest_unlock_level(kind: &str) -> Option<i32> {
    match kind {
        // The unlock numbers live in one place (meld_proto::skills); this just
        // gates "is a real manifestation" so unknown kinds return None.
        "gravity_well" | "kinetic_aegis" | "mind_spike" | "temporal_anchor" => {
            Some(meld_proto::skills::unlock_level(kind))
        }
        _ => None,
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
    skill_power_mult: f64,
    skill_heal_fraction: f64,
    item_heal_fraction: f64,
    psyker_gravity_tick_mult: f64,
    psyker_spike_tick_mult: f64,
    psyker_aegis_tick_fraction: f64,
    psyker_anchor_gauge_drain: f64,
    barrier_decay_per_turn: i32,
    resonant_transfuse_heal_fraction: f64,
    resonant_transfuse_cost_fraction: f64,
    resonant_boon_regen: i32,
    resonant_ward_barrier_fraction: f64,
    min_damage: i32,
    creature_flee_hp_fraction: f64,
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
            skill_power_mult: balance.battle.skill_power_mult,
            skill_heal_fraction: balance.battle.skill_heal_fraction,
            item_heal_fraction: balance.battle.item_heal_fraction,
            psyker_gravity_tick_mult: balance.battle.psyker_gravity_tick_mult,
            psyker_spike_tick_mult: balance.battle.psyker_spike_tick_mult,
            psyker_aegis_tick_fraction: balance.battle.psyker_aegis_tick_fraction,
            psyker_anchor_gauge_drain: balance.battle.psyker_anchor_gauge_drain,
            barrier_decay_per_turn: balance.battle.barrier_decay_per_turn,
            resonant_transfuse_heal_fraction: balance.battle.resonant_transfuse_heal_fraction,
            resonant_transfuse_cost_fraction: balance.battle.resonant_transfuse_cost_fraction,
            resonant_boon_regen: balance.battle.resonant_boon_regen,
            resonant_ward_barrier_fraction: balance.battle.resonant_ward_barrier_fraction,
            min_damage: balance.combat_math.min_damage,
            creature_flee_hp_fraction: balance.ai.flee_hp_fraction,
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

    /// Merge a joining party's fighters into the battle at gauge 0 (raid merge;
    /// enemy stats never rescale — combat-atb.md). Returns their wire views.
    pub fn join(&mut self, mut new: Vec<Fighter>) -> Vec<WireCombatant> {
        for f in &mut new {
            f.gauge = 0.0;
            f.awaiting = false;
            f.alive = f.hp > 0;
        }
        let views = new.iter().map(Fighter::to_wire).collect();
        self.fighters.extend(new);
        views
    }

    /// Number of distinct player combatants currently in the battle.
    pub fn player_count(&self) -> usize {
        self.fighters
            .iter()
            .filter(|f| f.kind == CombatantKind::Player)
            .count()
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
            .map(|f| (f.combatant_id.clone(), f.gauge, f.hp, f.wire_statuses()))
            .collect()
    }

    /// Current HP of a combatant by id (for carrying wounds across a run's
    /// encounters — persistent HP lives on the server between battles).
    pub fn combatant_hp(&self, combatant_id: &str) -> Option<i32> {
        self.fighters
            .iter()
            .find(|f| f.combatant_id == combatant_id)
            .map(|f| f.hp)
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
                    // 15 s elapsed with no action. A Psyker keeps channeling (its
                    // Foci tick, no new op); everyone else auto-defends.
                    let upkeep = self.start_of_turn(i);
                    let mut res = if self.fighters[i].focus_max > 0 {
                        // Auto-channel keeps each Focus firing at its own stored target.
                        self.resolve_psyker(i, None, None, None, true)
                    } else {
                        self.resolve_defend(i, None, true)
                    };
                    prepend_effects(&mut res, upkeep);
                    events.push(Event::Resolved(res));
                    self.check_terminal(&mut events);
                }
            } else {
                // Monster AI: attack the first living player.
                let upkeep = self.start_of_turn(i);
                if let Some(mut res) = self.resolve_monster_turn(i) {
                    prepend_effects(&mut res, upkeep);
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
        skill_kind: Option<String>,
        item_id: Option<Id>,
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
        // Start-of-turn upkeep (Regen heal, Barrier decay) fires before the action.
        let upkeep = self.start_of_turn(i);
        // A Psyker channels: every turn its active Foci fire, then it casts/
        // reinforces/revokes one (encoded in skill_kind). Flee still works normally.
        let target = target_ids.as_ref().and_then(|t| t.first()).map(|s| s.as_str());
        let is_psyker = self.fighters[i].focus_max > 0;
        let mut res = if is_psyker && action != BattleActionKind::Flee {
            self.resolve_psyker(i, skill_kind.as_deref(), target, Some(action_id), false)
        } else {
            match action {
                BattleActionKind::Attack => {
                    let target =
                        target.ok_or(Reject::ValidationError("attack requires target_ids"))?;
                    self.resolve_attack(i, target, Some(action_id))?
                }
                BattleActionKind::Defend => self.resolve_defend(i, Some(action_id), false),
                BattleActionKind::Flee => self.resolve_flee(i, Some(action_id)),
                BattleActionKind::Skill => {
                    self.resolve_skill(i, target, skill_kind.as_deref(), Some(action_id))?
                }
                // Slice items are always available (no inventory depletion yet).
                BattleActionKind::Item => {
                    self.resolve_item(i, item_id.as_deref(), target, Some(action_id))
                }
            }
        };
        // Prepend the upkeep effects so the client sees Regen/Barrier before the action.
        prepend_effects(&mut res, upkeep);
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
        let effects = match self.roll_dodge(target_i) {
            Some(dodge) => dodge,
            None => self.apply_damage(target_i, self.damage(atk, def, defending)),
        };
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

    /// Class skills (slice content). `power_strike` is a heavier attack
    /// (`atk * skill_power_mult - def`); `second_wind` (and the default) is a
    /// self-heal for `skill_heal_fraction` of max HP. (The Psyker does not use
    /// this path — it channels Foci via [`Battle::resolve_psyker`].)
    fn resolve_skill(
        &mut self,
        actor_i: usize,
        target_id: Option<&str>,
        skill_kind: Option<&str>,
        action_id: Option<Id>,
    ) -> Result<Resolution, Reject> {
        // A skill the hero hasn't leveled into yet is rejected server-side (the
        // client also greys it out; this is the authoritative backstop).
        if let Some(k) = skill_kind {
            if !meld_proto::skills::is_unlocked(k, self.fighters[actor_i].level) {
                return Err(Reject::ValidationError("skill not unlocked at this level"));
            }
        }
        if skill_kind == Some("second_wind") {
            let raw = ((self.fighters[actor_i].max_hp as f64) * self.skill_heal_fraction).round()
                as i32;
            let effects = self.apply_heal(actor_i, raw);
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Resonant healer skills. Aim at the chosen living ally if the player picked
        // one, else auto-target the most-wounded living ally (the classic default).
        if matches!(skill_kind, Some("transfuse") | Some("regen_boon") | Some("ward")) {
            let target_i = self
                .ally_target(target_id)
                .unwrap_or_else(|| self.most_wounded_ally(actor_i));
            let effects = self.resolve_resonant(actor_i, skill_kind.unwrap(), target_i);
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Power Strike: retarget to the first living enemy if the chosen one died.
        let target = target_id.ok_or(Reject::ValidationError("skill requires a target"))?;
        let target_i = match self.idx(target) {
            Some(t) if self.fighters[t].alive => t,
            _ => self
                .fighters
                .iter()
                .position(|f| f.alive && f.kind != CombatantKind::Player)
                .ok_or(Reject::NotFound)?,
        };
        let scaled_atk =
            (self.fighters[actor_i].atk as f64 * self.skill_power_mult).round() as i32;
        let def = self.fighters[target_i].def;
        let defending = self.fighters[target_i].defending;
        let effects = match self.roll_dodge(target_i) {
            Some(dodge) => dodge,
            None => self.apply_damage(target_i, self.damage(scaled_atk, def, defending)),
        };
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects))
    }

    /// Resolve a Psyker's turn. First every active Focus fires (offense manifestations
    /// crush the enemy ignoring armour, wards heal the Psyker, control drains the
    /// enemy's ATB gauge); then the chosen op — encoded in `skill_kind` — runs:
    ///
    /// - `cast:<kind>`      occupy a free slot with a new Manifestation (fires at once)
    /// - `reinforce:<kind>` stack an active Manifestation (max 2), firing the added stack
    /// - `revoke:<kind>`    end a Manifestation, freeing its slot
    /// - `hold` / absent    just let the Foci tick
    fn resolve_psyker(
        &mut self,
        actor_i: usize,
        op: Option<&str>,
        target: Option<&str>,
        action_id: Option<Id>,
        auto: bool,
    ) -> Resolution {
        let mut effects = Vec::new();
        // 1. Tick every active Focus (snapshot to avoid aliasing the Vec). Each
        // offensive Focus fires at its own stored target (retargeting on death).
        let active: Vec<(String, u8, Option<Id>)> = self.fighters[actor_i]
            .foci
            .iter()
            .map(|f| (f.kind.clone(), f.stacks, f.target_id.clone()))
            .collect();
        for (kind, stacks, target_id) in &active {
            effects.extend(self.tick_manifest(actor_i, kind, *stacks, target_id.as_deref()));
            if !self.any_enemy_alive() {
                break;
            }
        }

        // 2. Apply the management op. Offensive Manifestations remember the enemy the
        // player aimed them at; casting/reinforcing the same kind on a new enemy just
        // redirects it (see [`Focus::target_id`]).
        let op = op.unwrap_or("hold");
        let mut parts = op.splitn(2, ':');
        let verb = parts.next().unwrap_or("hold");
        let arg = parts.next().unwrap_or("");
        let aim = target.map(str::to_string);
        match verb {
            "cast" => {
                let level = self.fighters[actor_i].level;
                let unlocked = manifest_unlock_level(arg).is_some_and(|lv| level >= lv);
                let slot_free = self.fighters[actor_i].foci.len() < self.fighters[actor_i].focus_max;
                let already = self.fighters[actor_i].foci.iter().any(|f| f.kind == arg);
                if unlocked && slot_free && !already {
                    self.fighters[actor_i].foci.push(Focus {
                        kind: arg.to_string(),
                        stacks: 1,
                        target_id: aim.clone(),
                    });
                    effects.extend(self.tick_manifest(actor_i, arg, 1, aim.as_deref())); // fires immediately
                }
            }
            "reinforce" => {
                let mut bumped = false;
                if let Some(f) = self.fighters[actor_i].foci.iter_mut().find(|f| f.kind == arg) {
                    if aim.is_some() {
                        f.target_id = aim.clone(); // redirect to the freshly-aimed enemy
                    }
                    if f.stacks < 2 {
                        f.stacks += 1;
                        bumped = true;
                    }
                }
                if bumped {
                    effects.extend(self.tick_manifest(actor_i, arg, 1, aim.as_deref())); // the added stack fires
                }
            }
            "revoke" => {
                self.fighters[actor_i].foci.retain(|f| f.kind != arg);
            }
            _ => {} // hold
        }

        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        Resolution {
            action_id,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Skill,
            auto,
            flee_success: None,
            effects,
        }
    }

    /// Apply one tick of a Manifestation at `stacks` strength, aimed at `target_id`
    /// (the enemy the offensive Foci hit; ignored by the self-warding Kinetic Aegis).
    fn tick_manifest(
        &mut self,
        psyker_i: usize,
        kind: &str,
        stacks: u8,
        target_id: Option<&str>,
    ) -> Vec<ResolvedEffect> {
        match kind {
            "gravity_well" => {
                self.tick_offense(psyker_i, kind, self.psyker_gravity_tick_mult, stacks, target_id)
            }
            "mind_spike" => {
                self.tick_offense(psyker_i, kind, self.psyker_spike_tick_mult, stacks, target_id)
            }
            "kinetic_aegis" => {
                // The ward projects Barrier (temp HP), not a heal.
                let raw = (self.fighters[psyker_i].max_hp as f64
                    * self.psyker_aegis_tick_fraction
                    * stacks as f64)
                    .round() as i32;
                self.grant_barrier(psyker_i, raw)
            }
            "temporal_anchor" => self.tick_control(psyker_i, kind, stacks, target_id),
            _ => Vec::new(),
        }
    }

    /// The enemy index an offensive Focus hits this tick: its stored target if that
    /// enemy is alive, else the first living enemy — written back onto the Focus so the
    /// aim sticks after a retarget. `None` when no enemy is alive.
    fn focus_enemy_target(
        &mut self,
        psyker_i: usize,
        kind: &str,
        target_id: Option<&str>,
    ) -> Option<usize> {
        let aimed = target_id.and_then(|id| self.idx(id)).filter(|&t| {
            self.fighters[t].alive && self.fighters[t].kind != CombatantKind::Player
        });
        if let Some(t) = aimed {
            return Some(t);
        }
        let fallback = self
            .fighters
            .iter()
            .position(|f| f.alive && f.kind != CombatantKind::Player)?;
        let new_id = self.fighters[fallback].combatant_id.clone();
        if let Some(f) = self.fighters[psyker_i].foci.iter_mut().find(|f| f.kind == kind) {
            f.target_id = Some(new_id);
        }
        Some(fallback)
    }

    /// Grant `amount` Barrier (temp HP) to a fighter, reported as a status effect.
    fn grant_barrier(&mut self, i: usize, amount: i32) -> Vec<ResolvedEffect> {
        if amount <= 0 {
            return Vec::new();
        }
        self.fighters[i].barrier += amount;
        vec![ResolvedEffect {
            target_id: self.fighters[i].combatant_id.clone(),
            kind: EffectKind::StatusApplied,
            amount: Some(amount),
            status: Some("barrier".to_string()),
            hp_after: self.fighters[i].hp,
        }]
    }

    /// Index of a player-chosen ally target, if `target_id` names a **living player
    /// ally** — the guard that keeps aimed heals/items from ever healing an enemy (or
    /// a corpse). `None` means "no valid manual pick", so callers fall back to their
    /// default (most-wounded ally for heals, the actor for items).
    fn ally_target(&self, target_id: Option<&str>) -> Option<usize> {
        let id = target_id?;
        self.idx(id)
            .filter(|&t| self.fighters[t].alive && self.fighters[t].kind == CombatantKind::Player)
    }

    /// Index of the most-wounded living ally (lowest HP fraction), falling back to
    /// the caster if no other ally is hurt. Used to auto-target Resonant skills.
    fn most_wounded_ally(&self, caster_i: usize) -> usize {
        self.fighters
            .iter()
            .enumerate()
            .filter(|(_, f)| f.alive && f.kind == CombatantKind::Player)
            .min_by(|(_, a), (_, b)| {
                let fa = a.hp as f64 / a.max_hp.max(1) as f64;
                let fb = b.hp as f64 / b.max_hp.max(1) as f64;
                fa.total_cmp(&fb)
            })
            .map(|(i, _)| i)
            .unwrap_or(caster_i)
    }

    /// Resonant healer skills, applied to `target_i` (a resolved living ally — either
    /// the player's pick or the most-wounded default; see [`Battle::resolve_skill`]):
    /// - `transfuse`  — heal the ally, paying part of the heal from the Resonant's HP.
    /// - `regen_boon` — grant the ally the Regen status.
    /// - `ward`       — grant the ally Barrier.
    fn resolve_resonant(&mut self, caster_i: usize, skill: &str, target_i: usize) -> Vec<ResolvedEffect> {
        match skill {
            "transfuse" => {
                let heal = ((self.fighters[caster_i].max_hp as f64)
                    * self.resonant_transfuse_heal_fraction)
                    .round() as i32;
                let cost = ((heal as f64) * self.resonant_transfuse_cost_fraction).round() as i32;
                let mut effects = self.apply_heal(target_i, heal);
                // The Resonant pays its own HP (never below 1 — it doesn't suicide).
                let before = self.fighters[caster_i].hp;
                let after = (before - cost).max(1);
                self.fighters[caster_i].hp = after;
                effects.push(ResolvedEffect {
                    target_id: self.fighters[caster_i].combatant_id.clone(),
                    kind: EffectKind::Damage,
                    amount: Some(before - after),
                    status: Some("transfuse".to_string()),
                    hp_after: after,
                });
                effects
            }
            "regen_boon" => {
                self.fighters[target_i].regen += self.resonant_boon_regen;
                vec![ResolvedEffect {
                    target_id: self.fighters[target_i].combatant_id.clone(),
                    kind: EffectKind::StatusApplied,
                    amount: Some(self.fighters[target_i].regen),
                    status: Some("regen".to_string()),
                    hp_after: self.fighters[target_i].hp,
                }]
            }
            _ => {
                // ward
                let amount = ((self.fighters[caster_i].max_hp as f64)
                    * self.resonant_ward_barrier_fraction)
                    .round() as i32;
                self.grant_barrier(target_i, amount)
            }
        }
    }

    /// Start-of-turn upkeep for a fighter: apply Regen (heal) then decay Barrier.
    /// Returned effects are prepended to the turn's resolution.
    fn start_of_turn(&mut self, i: usize) -> Vec<ResolvedEffect> {
        let mut effects = Vec::new();
        if self.fighters[i].alive && self.fighters[i].regen > 0 {
            let raw = self.fighters[i].regen;
            effects.extend(self.apply_heal(i, raw));
        }
        if self.fighters[i].barrier > 0 {
            self.fighters[i].barrier =
                (self.fighters[i].barrier - self.barrier_decay_per_turn).max(0);
        }
        effects
    }

    /// Offensive Manifestation tick: `spell_power * mult * stacks` psychic damage
    /// to the Focus's aimed enemy, **ignoring defence** (def treated as 0). Scales
    /// with the Psyker's Mnd (which feeds `spell_power`), not its physical atk.
    fn tick_offense(
        &mut self,
        psyker_i: usize,
        kind: &str,
        mult: f64,
        stacks: u8,
        target_id: Option<&str>,
    ) -> Vec<ResolvedEffect> {
        let Some(t) = self.focus_enemy_target(psyker_i, kind, target_id) else {
            return Vec::new();
        };
        let power = self.fighters[psyker_i].spell_power;
        let dmg = ((power as f64) * mult * stacks as f64).round() as i32;
        self.apply_damage(t, dmg.max(self.min_damage))
    }

    /// Control Manifestation tick: drain the aimed enemy's ATB gauge, delaying its turns.
    fn tick_control(
        &mut self,
        psyker_i: usize,
        kind: &str,
        stacks: u8,
        target_id: Option<&str>,
    ) -> Vec<ResolvedEffect> {
        let Some(t) = self.focus_enemy_target(psyker_i, kind, target_id) else {
            return Vec::new();
        };
        let drain = self.psyker_anchor_gauge_drain * stacks as f64;
        self.fighters[t].gauge = (self.fighters[t].gauge - drain).max(0.0);
        vec![ResolvedEffect {
            target_id: self.fighters[t].combatant_id.clone(),
            kind: EffectKind::StatusApplied,
            amount: None,
            status: Some("slowed".to_string()),
            hp_after: self.fighters[t].hp,
        }]
    }

    /// Items (slice content). `elixir` fully heals; `salve` (and the default) heals
    /// `item_heal_fraction` of max HP. Applied to the chosen living ally if the player
    /// picked one, else the actor (the classic self-use default).
    fn resolve_item(
        &mut self,
        actor_i: usize,
        item_id: Option<&str>,
        target_id: Option<&str>,
        action_id: Option<Id>,
    ) -> Resolution {
        let heal_i = self.ally_target(target_id).unwrap_or(actor_i);
        let max_hp = self.fighters[heal_i].max_hp;
        let raw = if item_id == Some("elixir") {
            max_hp // full heal
        } else {
            ((max_hp as f64) * self.item_heal_fraction).round() as i32
        };
        let effects = self.apply_heal(heal_i, raw);
        // The action still belongs to the actor (its gauge/stance reset), even when
        // the heal lands on an ally.
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        self.resolution(actor_i, BattleActionKind::Item, action_id, effects)
    }

    /// Heal the actor by `raw` (min 1), capped at max HP; report the actual gain.
    fn apply_heal(&mut self, actor_i: usize, raw: i32) -> Vec<ResolvedEffect> {
        let before = self.fighters[actor_i].hp;
        let max_hp = self.fighters[actor_i].max_hp;
        let after = (before + raw.max(1)).min(max_hp);
        self.fighters[actor_i].hp = after;
        vec![ResolvedEffect {
            target_id: self.fighters[actor_i].combatant_id.clone(),
            kind: EffectKind::Heal,
            amount: Some(after - before),
            status: None,
            hp_after: after,
        }]
    }

    /// Assemble a non-flee, non-auto player [`Resolution`].
    fn resolution(
        &self,
        actor_i: usize,
        action: BattleActionKind,
        action_id: Option<Id>,
        effects: Vec<ResolvedEffect>,
    ) -> Resolution {
        Resolution {
            action_id,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action,
            auto: false,
            flee_success: None,
            effects,
        }
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

    /// A creature's turn. It targets the first living fighter *hostile to its
    /// faction* — a player, or a rival-faction creature — so a mixed-faction
    /// encounter has creatures fighting each other as well as the party. A
    /// `flees` creature bolts (leaves the battle) once its HP is low.
    fn resolve_monster_turn(&mut self, actor_i: usize) -> Option<Resolution> {
        let actor_faction = self.fighters[actor_i].faction.clone();

        // Skittish creatures flee a losing battle instead of attacking.
        if self.fighters[actor_i].flees {
            let f = &self.fighters[actor_i];
            let low = (f.hp as f64) < (f.max_hp as f64) * self.creature_flee_hp_fraction;
            if low && f.max_hp > 0 {
                self.fighters[actor_i].alive = false; // leaves the field
                self.reset_gauge(actor_i);
                return Some(Resolution {
                    action_id: None,
                    actor_id: self.fighters[actor_i].combatant_id.clone(),
                    action: BattleActionKind::Flee,
                    auto: true,
                    flee_success: Some(true),
                    effects: vec![ResolvedEffect {
                        target_id: self.fighters[actor_i].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("fled".to_string()),
                        hp_after: self.fighters[actor_i].hp,
                    }],
                });
            }
        }

        // Attack the *weakest* living fighter hostile to this creature's faction —
        // a player, or a rival-faction creature. Going for the lowest HP means a
        // wounded rival draws a creature away from the party, so a mixed-faction
        // encounter naturally has creatures turning on each other.
        let actor_id = self.fighters[actor_i].combatant_id.clone();
        let target_i = self
            .fighters
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                f.alive
                    && f.combatant_id != actor_id
                    && meld_proto::factions::battle_hostile(&actor_faction, &f.faction)
            })
            .min_by_key(|(_, f)| f.hp)
            .map(|(i, _)| i)?;
        let atk = self.fighters[actor_i].atk;
        let def = self.fighters[target_i].def;
        let defending = self.fighters[target_i].defending;
        let effects = match self.roll_dodge(target_i) {
            Some(dodge) => dodge,
            None => self.apply_damage(target_i, self.damage(atk, def, defending)),
        };
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

    /// Roll the target's Dex-derived dodge against a *physical* attack. On a
    /// dodge returns the whiff effect (0 HP change, `dodge` status) so the caller
    /// deals no damage; otherwise `None`. The RNG only advances when the target
    /// actually has dodge, so combatants with no Dex bonus don't perturb the
    /// deterministic stream (existing tests/replays are unaffected).
    fn roll_dodge(&mut self, target_i: usize) -> Option<Vec<ResolvedEffect>> {
        let chance = self.fighters[target_i].dodge;
        if chance > 0.0 && self.next_rand_unit() < chance {
            let t = &self.fighters[target_i];
            Some(vec![ResolvedEffect {
                target_id: t.combatant_id.clone(),
                kind: EffectKind::StatusApplied,
                amount: None,
                status: Some("dodge".to_string()),
                hp_after: t.hp,
            }])
        } else {
            None
        }
    }

    fn apply_damage(&mut self, target_i: usize, dmg: i32) -> Vec<ResolvedEffect> {
        let t = &mut self.fighters[target_i];
        // Barrier (temp HP) soaks damage before HP does.
        let absorbed = t.barrier.min(dmg.max(0));
        t.barrier -= absorbed;
        let hp_loss = (dmg - absorbed).max(0);
        t.hp = (t.hp - hp_loss).max(0);
        let dead = t.hp == 0;
        if dead {
            t.alive = false;
        }
        // Report the HP actually lost (barrier absorption shows via the barrier bar).
        let mut effects = vec![ResolvedEffect {
            target_id: t.combatant_id.clone(),
            kind: EffectKind::Damage,
            amount: Some(hp_loss),
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
        Fighter::new(
            id.to_string(),
            CombatantKind::Player,
            Some(format!("p-{id}")),
            None,
            1,
            40,
            12,
            3,
            speed,
        )
    }

    fn monster(id: &str, hp: i32, speed: i32) -> Fighter {
        let mut f = Fighter::new(
            id.to_string(),
            CombatantKind::Monster,
            None,
            Some("forest_bloom_stalker".into()),
            1,
            hp,
            14,
            4,
            speed,
        );
        f.faction = "beast".to_string();
        f
    }

    /// A creature of a specific faction.
    fn creature(id: &str, hp: i32, speed: i32, faction: &str) -> Fighter {
        let mut m = monster(id, hp, speed);
        m.faction = faction.to_string();
        m
    }

    #[test]
    fn creatures_turn_on_a_wounded_rival() {
        let b = balance();
        // A fast fiend, a near-dead beast (rival faction), and a healthy idle
        // player. The fiend goes for the weakest hostile — the beast — not the
        // player, so the two creatures brawl.
        let mut beast = creature("beast", 5, 1, "beast");
        beast.max_hp = 60;
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("p", 1)], // idle player
            vec![beast, creature("fiend", 1000, 400, "fiend")],
            &b,
            7,
        );
        // Let the fiend take a turn.
        for _ in 0..20 {
            battle.tick();
        }
        assert_eq!(player_hp(&battle, "beast"), 0, "the fiend struck the wounded beast");
        assert_eq!(player_hp(&battle, "p"), 40, "the player was left alone");
    }

    #[test]
    fn a_skittish_creature_flees_when_low() {
        let b = balance();
        // A lone `flees` creature at low HP bolts on its turn → victory (no enemy
        // left) without the player lifting a finger.
        let mut sh = creature("shade", 60, 400, "shade");
        sh.hp = 5; // below flee_hp_fraction * 60
        sh.flees = true;
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("p", 1)],
            vec![sh],
            &b,
            7,
        );
        let mut fled = false;
        let mut outcome = None;
        for _ in 0..20 {
            for ev in battle.tick() {
                match ev {
                    Event::Resolved(r) if r.action == BattleActionKind::Flee && r.actor_id == "shade" => {
                        fled = true;
                    }
                    Event::Ended { outcome: o } => outcome = Some(o),
                    _ => {}
                }
            }
        }
        assert!(fled, "the skittish creature should flee");
        assert_eq!(outcome, Some(BattleOutcome::Victory));
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
        // speed 110 / 4400 = 0.025 per tick → full at tick 40 (~4s FF5 cadence).
        let mut ready_tick = None;
        for t in 1..=60 {
            for ev in battle.tick() {
                if let Event::TurnReady { combatant_id } = ev {
                    assert_eq!(combatant_id, "a");
                    ready_tick.get_or_insert(t);
                }
            }
        }
        assert_eq!(ready_tick, Some(40), "speed-110 turn should ready at tick 40");
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
                            None,
                            None,
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

    /// Ticks until the given player combatant's turn is ready (cap guards runaway).
    fn tick_to_ready(battle: &mut Battle, cid: &str) {
        for _ in 0..500 {
            let ready = battle
                .tick()
                .into_iter()
                .any(|e| matches!(e, Event::TurnReady { combatant_id } if combatant_id == cid));
            if ready {
                return;
            }
        }
        panic!("turn never became ready for {cid}");
    }

    fn monster_def(id: &str, hp: i32, speed: i32, def: i32) -> Fighter {
        let mut m = monster(id, hp, speed);
        m.def = def;
        m
    }

    /// A Psyker fighter: focus_max slots, given level, no innate attack use.
    fn psyker(id: &str, speed: i32, level: i32, focus_max: usize) -> Fighter {
        let mut f = player(id, speed);
        f.level = level;
        f.focus_max = focus_max;
        f
    }

    fn foci_of(battle: &Battle, cid: &str) -> Vec<String> {
        battle
            .gauge_state()
            .into_iter()
            .find(|(id, _, _, _)| id == cid)
            .map(|(_, _, _, st)| st)
            .unwrap_or_default()
    }

    #[test]
    fn psyker_casts_and_reinforces_a_focus_that_ignores_defence() {
        let b = balance();
        // Psyker atk 12. Gravity Well tick = round(12 * 0.55 * stacks), def ignored.
        // Against a def-20 wall a plain hit floors to min_damage; the Focus lands full.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![psyker("p", 110, 1, 2)],
            vec![monster_def("m", 1000, 1, 20)],
            &b,
            7,
        );
        // Cast Gravity Well — fires immediately for round(12*0.55*1)=7.
        tick_to_ready(&mut battle, "p");
        let evs = battle
            .submit(
                "p",
                "c1".into(),
                BattleActionKind::Skill,
                None,
                Some("cast:gravity_well".into()),
                None,
            )
            .expect("cast resolves");
        let dmg: i32 = evs
            .iter()
            .filter_map(|e| match e {
                Event::Resolved(r) => Some(r.effects.iter().filter_map(|x| x.amount).sum::<i32>()),
                _ => None,
            })
            .sum();
        assert_eq!(dmg, 7, "gravity well fires on cast, ignoring def");
        assert!(foci_of(&battle, "p").iter().any(|s| s == "focus:gravity_well:1"));

        // Next turn: the Focus ticks again (7) AND we reinforce (adds a stack that
        // also fires for 7) → 14 this turn, and the slot now reads stacks 2.
        tick_to_ready(&mut battle, "p");
        let evs = battle
            .submit(
                "p",
                "r1".into(),
                BattleActionKind::Skill,
                None,
                Some("reinforce:gravity_well".into()),
                None,
            )
            .expect("reinforce resolves");
        let dmg: i32 = evs
            .iter()
            .filter_map(|e| match e {
                Event::Resolved(r) => Some(r.effects.iter().filter_map(|x| x.amount).sum::<i32>()),
                _ => None,
            })
            .sum();
        assert_eq!(dmg, 14, "existing tick (7) + reinforced stack tick (7)");
        assert!(foci_of(&battle, "p").iter().any(|s| s == "focus:gravity_well:2"));
    }

    #[test]
    fn psyker_focus_cap_and_unlocks_are_enforced() {
        let b = balance();
        // Level-1 Psyker: mind_spike (unlock L3) can't be cast; two L1 slots fill.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![psyker("p", 400, 1, 2)],
            vec![monster("m", 100000, 1)],
            &b,
            7,
        );
        let cast = |battle: &mut Battle, n: u32, kind: &str| {
            tick_to_ready(battle, "p");
            battle
                .submit(
                    "p",
                    format!("op{n}"),
                    BattleActionKind::Skill,
                    None,
                    Some(format!("cast:{kind}")),
                    None,
                )
                .expect("op resolves");
        };
        cast(&mut battle, 1, "mind_spike"); // locked at L1 → ignored
        assert!(foci_of(&battle, "p").iter().all(|s| !s.contains("mind_spike")));
        cast(&mut battle, 2, "gravity_well");
        cast(&mut battle, 3, "kinetic_aegis");
        cast(&mut battle, 4, "temporal_anchor"); // slots full (2) → ignored
        let foci: Vec<String> = foci_of(&battle, "p")
            .into_iter()
            .filter(|s| s.starts_with("focus:"))
            .collect();
        assert_eq!(foci.len(), 2, "focus_max is respected");
    }

    #[test]
    fn kinetic_aegis_grants_barrier_each_turn() {
        let b = balance();
        // aegis tick = round(40 * 0.1 * 1) = 4 Barrier (temp HP), not a heal.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![psyker("p", 110, 1, 2)],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "p");
        let evs = battle
            .submit(
                "p",
                "a".into(),
                BattleActionKind::Skill,
                None,
                Some("cast:kinetic_aegis".into()),
                None,
            )
            .expect("aegis resolves");
        let barrier = evs.iter().find_map(|e| match e {
            Event::Resolved(r) => r
                .effects
                .iter()
                .find(|x| x.status.as_deref() == Some("barrier"))
                .and_then(|x| x.amount),
            _ => None,
        });
        assert_eq!(barrier, Some(4), "kinetic aegis grants Barrier on cast");
        assert!(foci_of(&battle, "p").iter().any(|s| s == "barrier:4"));
    }

    #[test]
    fn barrier_absorbs_damage_before_hp() {
        let b = balance();
        // Player with 10 Barrier takes an 11-dmg monster hit: Barrier eats 10,
        // only 1 comes off HP (40 → 39).
        let mut p = player("a", 110);
        p.barrier = 10;
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![p],
            vec![monster("m", 1000, 200)], // acts ~tick 22, before the player's tick-40 turn
            &b,
            7,
        );
        tick_times(&mut battle, 30);
        assert_eq!(player_hp(&battle, "a"), 39, "barrier soaks 10 of the 11 hit");
        assert!(
            !foci_of(&battle, "a").iter().any(|s| s.starts_with("barrier:")),
            "barrier fully spent"
        );
    }

    #[test]
    fn regen_heals_at_start_of_turn() {
        let b = balance();
        // A wounded fighter with Regen 5 heals 5 the moment it acts.
        let mut p = player("a", 400);
        p.hp = 30;
        p.regen = 5;
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![p],
            vec![monster("m", 1000, 1)], // idle
            &b,
            7,
        );
        tick_to_ready(&mut battle, "a");
        let evs = battle
            .submit("a", "d".into(), BattleActionKind::Defend, None, None, None)
            .expect("defend resolves");
        let heal = evs.iter().find_map(|e| match e {
            Event::Resolved(r) => r
                .effects
                .iter()
                .find(|x| x.kind == EffectKind::Heal)
                .map(|x| (x.amount, x.hp_after)),
            _ => None,
        });
        assert_eq!(heal, Some((Some(5), 35)), "regen heals 5 at start of turn");
    }

    #[test]
    fn resonant_transfuse_heals_ally_at_own_hp_cost() {
        let b = balance();
        // Transfuse: heal = round(46 * 0.4) = 18 to the wounded ally; cost =
        // round(18 * 0.5) = 9 off the Resonant's own HP.
        let mut caster = player("c", 400);
        caster.hp = 46;
        caster.max_hp = 46;
        let mut ally = player("a", 1); // slow: never acts, stays wounded
        ally.hp = 10;
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![caster, ally],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "c");
        battle
            .submit(
                "c",
                "t".into(),
                BattleActionKind::Skill,
                None,
                Some("transfuse".into()),
                None,
            )
            .expect("transfuse resolves");
        assert_eq!(player_hp(&battle, "a"), 28, "ally healed 18 (10 → 28)");
        assert_eq!(player_hp(&battle, "c"), 37, "resonant paid 9 (46 → 37)");
    }

    #[test]
    fn aimed_heal_targets_the_chosen_ally_not_the_most_wounded() {
        let b = balance();
        // Two hurt allies: a1 is the most wounded (the auto-target), a2 is the one the
        // player aims at. Passing target_ids=[a2] must heal a2, leaving a1 untouched.
        let mut caster = player("c", 400);
        caster.hp = 46;
        caster.max_hp = 46; // → transfuse heal = round(46*0.4) = 18
        let mut a1 = player("a1", 1);
        a1.hp = 10; // most wounded
        let mut a2 = player("a2", 1);
        a2.hp = 20; // the chosen target
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![caster, a1, a2],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "c");
        battle
            .submit(
                "c",
                "t".into(),
                BattleActionKind::Skill,
                Some(vec!["a2".into()]),
                Some("transfuse".into()),
                None,
            )
            .expect("transfuse resolves");
        assert_eq!(player_hp(&battle, "a2"), 38, "chosen ally healed 18 (20 → 38)");
        assert_eq!(player_hp(&battle, "a1"), 10, "most-wounded ally left untouched");
    }

    #[test]
    fn item_can_be_used_on_a_chosen_ally() {
        let b = balance();
        // Salve heals round(40*0.4)=16. The actor uses it on an ally, not itself.
        let mut actor = player("c", 400);
        actor.hp = 20;
        let mut ally = player("a", 1);
        ally.hp = 5;
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![actor, ally],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "c");
        battle
            .submit(
                "c",
                "i".into(),
                BattleActionKind::Item,
                Some(vec!["a".into()]),
                None,
                Some("salve".into()),
            )
            .expect("item resolves");
        assert_eq!(player_hp(&battle, "a"), 21, "ally healed by the salve (5 → 21)");
        assert_eq!(player_hp(&battle, "c"), 20, "actor spent its turn, kept its own HP");
    }

    #[test]
    fn psyker_focus_hits_the_aimed_enemy_and_reinforce_redirects() {
        let b = balance();
        // Two enemies. Aim Gravity Well at m2 (not the first enemy): only m2 takes the
        // round(12*0.55)=7 tick. m1 is left alone, proving per-focus targeting.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![psyker("p", 110, 1, 2)],
            vec![monster("m1", 1000, 1), monster("m2", 1000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "p");
        battle
            .submit(
                "p",
                "c1".into(),
                BattleActionKind::Skill,
                Some(vec!["m2".into()]),
                Some("cast:gravity_well".into()),
                None,
            )
            .expect("cast resolves");
        assert_eq!(player_hp(&battle, "m1"), 1000, "first enemy untouched");
        assert_eq!(player_hp(&battle, "m2"), 993, "aimed enemy took the 7 tick");

        // Reinforce aimed at m1 redirects the focus. Ticks fire at the start of the
        // turn (before the op), so the still-aimed-at-m2 stack lands its 7 on m2, then
        // the redirect applies and the freshly-added stack fires its 7 on m1.
        tick_to_ready(&mut battle, "p");
        battle
            .submit(
                "p",
                "r1".into(),
                BattleActionKind::Skill,
                Some(vec!["m1".into()]),
                Some("reinforce:gravity_well".into()),
                None,
            )
            .expect("reinforce resolves");
        assert_eq!(player_hp(&battle, "m2"), 986, "old target took this turn's existing tick");
        assert_eq!(player_hp(&battle, "m1"), 993, "redirected stack landed on m1");

        // A plain hold turn proves the redirect stuck: both stacks (round(12*0.55*2)=13)
        // now fire on m1, and m2 is no longer touched.
        tick_to_ready(&mut battle, "p");
        battle
            .submit(
                "p",
                "h1".into(),
                BattleActionKind::Skill,
                None,
                Some("hold".into()),
                None,
            )
            .expect("hold resolves");
        assert_eq!(player_hp(&battle, "m1"), 980, "both stacks now hit m1 (took 13)");
        assert_eq!(player_hp(&battle, "m2"), 986, "m2 untouched after the redirect stuck");
    }

    #[test]
    fn skill_hits_harder_than_a_plain_attack() {
        let b = balance();
        // atk 12, def 4 → attack = 8; skill = round(12*1.75) - 4 = 21 - 4 = 17.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("a", 110)],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "a");
        let evs = battle
            .submit(
                "a",
                "s1".into(),
                BattleActionKind::Skill,
                Some(vec!["m".into()]),
                Some("power_strike".into()),
                None,
            )
            .expect("skill resolves");
        let dmg = evs.iter().find_map(|e| match e {
            Event::Resolved(r) if r.action == BattleActionKind::Skill => r.effects[0].amount,
            _ => None,
        });
        assert_eq!(dmg, Some(17), "power strike should use the 1.75x multiplier");
    }

    #[test]
    fn item_heals_the_wounded_actor_without_overhealing() {
        let b = balance();
        // A brisk monster (speed 200 → acts ~every 22 ticks) wounds the speed-110
        // player (ready at tick 40) exactly once (14 atk − 3 def = 11) before the
        // player's first turn: 40 → 29 hp.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("a", 110)], // 40 max hp, def 3
            vec![monster("m", 1000, 200)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "a");
        let hp_before = battle
            .gauge_state()
            .into_iter()
            .find(|(id, _, _, _)| id == "a")
            .unwrap()
            .2;
        assert_eq!(hp_before, 29, "monster should have landed one 11-dmg hit");
        let evs = battle
            .submit(
                "a",
                "i1".into(),
                BattleActionKind::Item,
                None,
                None,
                Some("salve".into()),
            )
            .expect("item resolves");
        let eff = evs
            .iter()
            .find_map(|e| match e {
                Event::Resolved(r) if r.action == BattleActionKind::Item => Some(r.effects[0].clone()),
                _ => None,
            })
            .expect("item resolution present");
        assert_eq!(eff.kind, EffectKind::Heal);
        // Salve rolls 0.4*40 = 16, but only 11 is missing → heal 11, capped at max.
        assert_eq!(eff.amount, Some(11));
        assert_eq!(eff.hp_after, 40);
    }

    fn tick_times(battle: &mut Battle, n: usize) {
        for _ in 0..n {
            battle.tick();
        }
    }

    fn player_hp(battle: &Battle, cid: &str) -> i32 {
        battle
            .gauge_state()
            .into_iter()
            .find(|(id, _, _, _)| id == cid)
            .unwrap()
            .2
    }

    /// Sets up a speed-110 player wounded to 18 hp by two 11-dmg monster hits
    /// (monster speed 200 acts at ticks 22 & 44; player is awaiting from tick 40)
    /// and returns the heal effect of `submit`ing the given skill/item.
    fn wounded_heal(skill: Option<&str>, item: Option<&str>) -> ResolvedEffect {
        let b = balance();
        // Level 2 so Second Wind (unlocks at 2) is usable; Item is level-agnostic.
        let mut caster = player("a", 110);
        caster.level = 2;
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![caster],
            vec![monster("m", 1000, 200)],
            &b,
            7,
        );
        tick_times(&mut battle, 50);
        assert_eq!(player_hp(&battle, "a"), 18, "two 11-dmg hits land by tick 50");
        let action = if skill.is_some() {
            BattleActionKind::Skill
        } else {
            BattleActionKind::Item
        };
        let evs = battle
            .submit(
                "a",
                "h".into(),
                action,
                Some(vec!["m".into()]),
                skill.map(String::from),
                item.map(String::from),
            )
            .expect("heal resolves");
        evs.into_iter()
            .find_map(|e| match e {
                Event::Resolved(r) if r.action == action => Some(r.effects[0].clone()),
                _ => None,
            })
            .expect("heal resolution present")
    }

    #[test]
    fn second_wind_skill_heals_a_fraction_of_max_hp() {
        // 0.3 * 40 = 12; wounded to 18 → 30.
        let eff = wounded_heal(Some("second_wind"), None);
        assert_eq!(eff.kind, EffectKind::Heal);
        assert_eq!(eff.amount, Some(12));
        assert_eq!(eff.hp_after, 30);
    }

    #[test]
    fn locked_skill_is_rejected_until_leveled() {
        let b = balance();
        // A level-1 Squire cannot use Second Wind (unlocks at 2).
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("a", 110)], // level 1
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        // Fill the gauge so the action is otherwise legal.
        tick_times(&mut battle, 20);
        let res = battle.submit(
            "a",
            "h".into(),
            BattleActionKind::Skill,
            None,
            Some("second_wind".to_string()),
            None,
        );
        assert!(res.is_err(), "level-1 Second Wind must be rejected");
    }

    #[test]
    fn high_dodge_target_avoids_some_hits() {
        let b = balance();
        // A fast monster hammers a dodgy, high-HP player; over many swings the
        // player's 35% dodge whiffs some of them (a `dodge` status, 0 HP loss).
        let mut dodgy = player("a", 1); // slow so it never acts; just soaks hits
        dodgy.dodge = 0.35;
        dodgy.hp = 100_000;
        dodgy.max_hp = 100_000;
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![dodgy],
            vec![monster("m", 1000, 400)], // fast attacker
            &b,
            7,
        );
        let mut dodges = 0;
        for _ in 0..300 {
            for ev in battle.tick() {
                if let Event::Resolved(r) = ev {
                    if r.effects.iter().any(|e| e.status.as_deref() == Some("dodge")) {
                        dodges += 1;
                    }
                }
            }
        }
        assert!(dodges > 0, "a 35%-dodge target should whiff at least one attack");
    }

    #[test]
    fn elixir_item_fully_heals() {
        // Full heal from 18 → 40 (gain 22).
        let eff = wounded_heal(None, Some("elixir"));
        assert_eq!(eff.kind, EffectKind::Heal);
        assert_eq!(eff.amount, Some(22));
        assert_eq!(eff.hp_after, 40);
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
        // speed 400 / 4400 ≈ 0.09 per tick → full by tick 12.
        for _ in 0..12 {
            battle.tick();
        }
        let first = battle.submit(
            "a",
            "dup".into(),
            BattleActionKind::Attack,
            Some(vec!["m".into()]),
            None,
            None,
        );
        assert!(first.is_ok(), "first submit should resolve: {first:?}");
        // Re-ready and resubmit the same action_id (dup is rejected before the
        // gauge check, so it fails regardless of gauge state).
        for _ in 0..12 {
            battle.tick();
        }
        let second = battle.submit(
            "a",
            "dup".into(),
            BattleActionKind::Attack,
            Some(vec!["m".into()]),
            None,
            None,
        );
        assert_eq!(second, Err(Reject::DuplicateAction));
    }
}
