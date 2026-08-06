//! Run & instance lifecycle for the spike (docs/behaviors/run-lifecycle.md subset).
//!
//! Provides: base-run-level derivation, per-player ephemeral run state
//! (backpack + run level/XP), the victory/defeat outcome transitions, and the
//! bridge that assembles a [`meld_battle::Battle`] from an arena monster and a
//! party. Extraction channels, death durability (HTTP/DB side), and abandon are
//! the next slices; the run/battle spine they hang off is here.

use std::collections::HashMap;

use meld_balance::Balance;
use meld_battle::{Battle, Fighter};
use meld_proto::common::{ItemStack, LootGear};
use meld_proto::enums::{CharacterClass, CombatantKind, EncounterClass, RunResult};
use meld_proto::Id;
use meld_world::MonsterSpawn;

/// `base_run_level(hub) = round(1 + hub.distance × per_distance)` (CANON.md §B).
pub fn base_run_level(distance: i32, balance: &Balance) -> i32 {
    (1.0 + distance as f64 * balance.runs.base_run_level_per_distance).round() as i32
}

/// XP needed to advance from level `L`: `xp_base × xp_growth_factor^(L-1)`
/// (CANON.md §B) — the classic "double the requirement each level" curve.
pub fn xp_to_next(level: i32, balance: &Balance) -> i64 {
    // The curve IS its design statement: level L takes (L + offset) fights against a
    // same-level encounter. Two fights clear level 1, three clear level 2, four clear
    // level 3. The XP number is derived from the encounter tables rather than tuned
    // separately, so creature XP and the ladder cannot drift apart.
    let fights = (level.max(1) + balance.runs.fights_per_level_offset).max(1) as f64;
    (fights * same_level_encounter_xp(level, balance) as f64).round().max(1.0) as i64
}

/// What one encounter AT a hero's own level pays. A same-level encounter sits at
/// `d = 12.5 × level` because `mlevel(d) = round(d / 12.5)` (CANON §B), so the
/// distance scaling gives its XP directly.
pub fn same_level_encounter_xp(level: i32, balance: &Balance) -> i64 {
    let r = &balance.runs;
    let sc = &balance.world_scaling;
    let distance = 12.5 * level.max(1) as f64;
    let mult = (1.0 + distance / sc.stat_mult_base_divisor).powf(sc.xp_distance_exp);
    (r.xp_reference_creature * mult * r.xp_reference_group)
        .round()
        .max(1.0) as i64
}

/// How many same-level fights level `level` costs — the design statement itself.
pub fn fights_per_level(level: i32, balance: &Balance) -> i32 {
    (level.max(1) + balance.runs.fights_per_level_offset).max(1)
}

/// Total XP to climb from level 1 to `level`.
pub fn xp_total_to_level(level: i32, balance: &Balance) -> i64 {
    (1..level.max(1)).map(|l| xp_to_next(l, balance)).sum()
}

/// One player's ephemeral run state.
#[derive(Debug, Clone)]
pub struct PlayerRun {
    pub run_id: Id,
    pub player_id: Id,
    pub username: String,
    pub character_class: CharacterClass,
    /// The player's headline level — the highest of their heroes'. Kept as one
    /// number because messages, the loot report and `base_run_level` all want one;
    /// the per-hero ladders in `hero_levels` are the source of truth.
    pub run_level: i32,
    /// Per-hero level inside this dive, aligned with the party's slots. Levels are
    /// dive-scoped (XP never persists); what survives is the account's best-per-class
    /// record.
    pub hero_levels: Vec<i32>,
    /// Per-hero banked XP toward each hero's own next level.
    pub hero_xp: Vec<i64>,
    pub xp: i64,
    pub backpack: Vec<ItemStack>,
    /// Chits found this run (economy.md S1). Lives in the backpack conceptually;
    /// banked into the Vault on extraction, deleted with the run on death.
    pub chits: i64,
    /// Red-chest gear found this run. Unowned until extraction converts it to
    /// owned Vault gear (gear-item-models.md); discarded on death.
    pub looted_gear: Vec<LootGear>,
    pub max_distance_reached: i32,
    pub result: Option<RunResult>,
    /// Which party (enter-maze group) this run belongs to. Battles merge across
    /// party ids (the Expandable Party raid mechanic).
    pub party_id: u32,
}

impl PlayerRun {
    pub fn is_terminal(&self) -> bool {
        self.result.is_some()
    }

    /// Apply victory XP, leveling up as thresholds are crossed. Returns the
    /// number of levels gained.
    /// Bank `xp` on ONE hero (by party slot) and settle the levels it buys. Only
    /// living heroes are ever passed here — a hero that fell earns nothing from the
    /// fight it did not finish. Returns the levels that hero gained.
    ///
    /// `party_size` grows the per-hero vectors on first use, seeded at the run's
    /// `base_run_level`, so a dive from a deeper hub starts every hero deeper.
    /// Credit one hero its share of an encounter's XP.
    ///
    /// The share is the encounter's XP DIVIDED by the party size: a lone hero absorbs
    /// the whole lesson, four split it. That single division is what gives the game
    /// its intended arc — the solo era levels fast on short fights, and a full party's
    /// runs are long — without needing two sets of encounters.
    pub fn award_hero_xp(
        &mut self,
        slot: usize,
        party_size: usize,
        xp: i64,
        balance: &Balance,
    ) -> i32 {
        let size = party_size.max(slot + 1);
        let xp = if balance.runs.xp_split_across_party {
            // Never round a real reward down to nothing.
            ((xp as f64) / size.max(1) as f64).round().max(1.0) as i64
        } else {
            xp
        };
        if self.hero_levels.len() < size {
            let base = self.run_level;
            self.hero_levels.resize(size, base);
            self.hero_xp.resize(size, 0);
        }
        if xp <= 0 {
            return 0;
        }
        let cap = balance.runs.max_hero_level;
        let mut gained = 0;
        self.hero_xp[slot] += xp;
        while self.hero_levels[slot] < cap {
            let need = xp_to_next(self.hero_levels[slot], balance);
            if need <= 0 || self.hero_xp[slot] < need {
                break;
            }
            self.hero_xp[slot] -= need;
            self.hero_levels[slot] += 1;
            gained += 1;
        }
        // The headline level follows the best hero, so every message that wants one
        // number keeps telling the truth.
        self.run_level = self.hero_levels.iter().copied().max().unwrap_or(self.run_level);
        gained
    }

    /// This hero's level inside the dive; the run's headline level until the hero has
    /// earned anything of its own.
    /// Try to put `item` in the backpack. Returns false when the pack is FULL, so the
    /// caller can tell the player rather than silently swallowing the drop.
    ///
    /// A dive's carrying capacity is the pressure that makes extraction a decision:
    /// with an unbounded pack you never choose between the heavy thing and the
    /// valuable thing, and you never turn back early because you are full.
    /// Same-kind stacks merge and do not consume a slot.
    /// Total slots this run can carry: the party's shared bag plus a pouch per hero.
    /// A bigger party carries more, but never enough to stop choosing.
    pub fn carry_capacity(&self, balance: &Balance) -> usize {
        let heroes = self.hero_levels.len().max(1);
        (balance.runs.backpack_slots.max(0) as usize)
            + heroes * (balance.runs.hero_pouch_slots.max(0) as usize)
    }

    pub fn try_carry(&mut self, item: ItemStack, balance: &Balance) -> bool {
        if let Some(stack) = self
            .backpack
            .iter_mut()
            .find(|s| s.item_kind == item.item_kind && s.insurance == item.insurance)
        {
            stack.quantity += item.quantity;
            return true;
        }
        if self.backpack.len() >= self.carry_capacity(balance).max(1) {
            return false;
        }
        self.backpack.push(item);
        true
    }

    /// Whether the pack has no room for a NEW kind of item.
    pub fn pack_full(&self, balance: &Balance) -> bool {
        self.backpack.len() >= self.carry_capacity(balance).max(1)
    }

    pub fn hero_level(&self, slot: usize) -> i32 {
        self.hero_levels.get(slot).copied().unwrap_or(self.run_level)
    }

    /// How many of the player's heroes are at or above `level` — what the party-slot
    /// unlock rules count ("two heroes at 20").
    pub fn heroes_at_level(&self, level: i32) -> usize {
        self.hero_levels.iter().filter(|l| **l >= level).count()
    }

    pub fn award_xp(&mut self, xp: i64, balance: &Balance) -> i32 {
        self.xp += xp;
        let mut gained = 0;
        while self.xp >= xp_to_next(self.run_level, balance) {
            self.xp -= xp_to_next(self.run_level, balance);
            self.run_level += 1;
            gained += 1;
        }
        gained
    }
}

/// The run set for one MazeInstance (spike: one instance, one monster).
pub struct InstanceRun {
    pub instance_id: Id,
    pub departure_hub_distance: i32,
    pub base_run_level: i32,
    pub runs: Vec<PlayerRun>,
    next_party_id: u32,
}

impl InstanceRun {
    pub fn new(instance_id: Id, departure_hub_distance: i32, balance: &Balance) -> Self {
        InstanceRun {
            instance_id,
            departure_hub_distance,
            base_run_level: base_run_level(departure_hub_distance, balance),
            runs: Vec::new(),
            next_party_id: 0,
        }
    }

    /// Add a party (one enter-maze group) and return its party id.
    pub fn add_party(
        &mut self,
        members: Vec<(Id, String, CharacterClass, Id)>, // (player_id, username, class, run_id)
    ) -> u32 {
        let party_id = self.next_party_id;
        self.next_party_id += 1;
        for (player_id, username, character_class, run_id) in members {
            self.runs.push(PlayerRun {
                run_id,
                player_id,
                username,
                character_class,
                run_level: self.base_run_level,
                hero_levels: Vec::new(),
                hero_xp: Vec::new(),
                xp: 0,
                backpack: Vec::new(),
                chits: 0,
                looted_gear: Vec::new(),
                max_distance_reached: 0,
                result: None,
                party_id,
            });
        }
        party_id
    }

    pub fn run_mut(&mut self, player_id: &str) -> Option<&mut PlayerRun> {
        self.runs.iter_mut().find(|r| r.player_id == player_id)
    }

    /// All members reached a terminal state → instance may close.
    pub fn all_terminal(&self) -> bool {
        self.runs.iter().all(PlayerRun::is_terminal)
    }
}

/// Map a `CharacterClass` to its balance content key.
pub fn class_key(class: CharacterClass) -> &'static str {
    match class {
        CharacterClass::Explorer => "explorer",
        CharacterClass::Hunter => "hunter",
        CharacterClass::Dragoon => "dragoon",
        CharacterClass::Sage => "sage",
        CharacterClass::Ranger => "ranger",
        CharacterClass::AlchemistKnight => "alchemist_knight",
        CharacterClass::Bard => "bard",
        CharacterClass::Psyker => "psyker",
        CharacterClass::Resonant => "resonant",
        CharacterClass::Shifter => "shifter",
        CharacterClass::PhoenixGuard => "phoenix_guard",
    }
}

/// Max HP for a class at a given level (CANON.md §B attribute growth: Wll →
/// HP). Shared by `party_fighters` (battle setup) and level-up handling (a
/// level-up heals to the new max, unlike mid-run wounds which persist).
pub fn max_hp_at_level(class: CharacterClass, level: i32, balance: &Balance) -> i32 {
    let stats = balance
        .player
        .get(class_key(class))
        .unwrap_or_else(|| balance.player.get("explorer").expect("explorer stats"));
    let (_, _, _, wll) = stats.attributes_at(level);
    let grow = |attr: i32, base: i32, coef: f64| ((attr - base) as f64 * coef).round() as i32;
    stats.base_hp + grow(wll, stats.wll, balance.attributes.wll_to_hp)
}

/// One hero's summed combat bonuses from their own equipped gear (per-hero
/// equip slots — each hero in a party can wear different gear).
#[derive(Debug, Clone, Default)]
pub struct GearBonus {
    pub atk: i32,
    pub def: i32,
    pub spd: i32,
    /// AD-1 ward affixes — what the hero starts each battle holding.
    pub barrier: i32,
    pub regen: i32,
    /// Evasion in percentage points.
    pub evasion: i32,
    /// AD-1 keyword affixes, already filtered to this hero's class.
    pub adrenaline: i32,
    pub focus_slots: i32,
    /// Unresolved synergy affixes: (ally class key, atk, def). Paid out here,
    /// where the party composition is known.
    pub synergies: Vec<(String, i32, i32)>,
    /// AD-3 brand: the element this hero's attacks deal.
    pub brand: Option<String>,
    /// AD-1 unique drawbacks — what this loadout costs.
    pub penalty_atk: i32,
    pub penalty_def: i32,
    pub penalty_spd: i32,
    pub penalty_max_hp: i32,
    /// AD-1 set pieces worn: (set key, count).
    pub set_pieces: Vec<(String, usize)>,
    /// Raw per-item elemental entries (DamageType wire key → multiplier) from
    /// every equipped piece; folded and clamped by [`fold_damage_modifiers`]
    /// at battle assembly (spec §5).
    pub modifiers: Vec<(String, f64)>,
}

/// Fold a hero's raw per-item elemental entries into one profile (spec §5
/// stat aggregation): per damage type, `1 + Σ(mᵢ − 1)` — so two quarter-
/// resists (0.75) stack to a half-resist (0.5) instead of multiplying into a
/// weakness — clamped to the spec's 0.0–2.0 bounds [TUNABLE bounds live in
/// the spec; structural here].
pub fn fold_damage_modifiers(
    entries: &[(String, f64)],
) -> std::collections::HashMap<meld_proto::enums::DamageType, f64> {
    let mut acc: HashMap<meld_proto::enums::DamageType, f64> = HashMap::new();
    for (key, m) in entries {
        // Keys are the wire form ("FIRE"); unknown keys are skipped.
        let Some(ty) = meld_proto::enums::DamageType::from_wire(key) else {
            continue;
        };
        *acc.entry(ty).or_insert(1.0) += m - 1.0;
    }
    for v in acc.values_mut() {
        *v = v.clamp(0.0, 2.0);
    }
    acc
}

/// Assemble a battle from a party and one arena monster. `party` gives, per
/// player, the (player_id, combatant_id, class); the server owns combatant ids.
/// Per-player combatant inputs for a battle: (player_id, combatant_id, class,
/// that hero's own equipped-gear bonus).
pub type PartyMember = (Id, Id, CharacterClass, GearBonus);

/// Build the ally `Fighter`s for a party (shared by battle start and raid merge).
/// `row_overrides` (aligned with `party`) lets the player's saved formation win over
/// the class-default front/back row: `Some(true)` = back, `Some(false)` = front,
/// `None`/absent = keep the class default.
pub fn party_fighters(
    party: &[PartyMember],
    runs: &InstanceRun,
    balance: &Balance,
    row_overrides: &[Option<bool>],
) -> Vec<Fighter> {
    // Index run level by player once so the per-member lookup is O(1) rather than
    // scanning every run per member (O(party × runs) — both grow with raid size).
    let run_by_player: HashMap<&str, &PlayerRun> = runs
        .runs
        .iter()
        .map(|r| (r.player_id.as_str(), r))
        .collect();
    let level_by_player: HashMap<&str, i32> = runs
        .runs
        .iter()
        .map(|r| (r.player_id.as_str(), r.run_level))
        .collect();
    let mut fighters = party
        .iter()
        .enumerate()
        .map(|(i, (player_id, combatant_id, class, bonus))| {
            let stats = balance
                .player
                .get(class_key(*class))
                .unwrap_or_else(|| balance.player.get("explorer").expect("explorer stats"));
            // Each hero fights at ITS OWN level (dive-scoped): the hero that has been
            // doing the killing is the hero that got stronger. Falls back to the
            // player's headline level for a hero that has not earned anything yet.
            let level = run_by_player
                .get(player_id.as_str())
                .map(|r| r.hero_level(i))
                .or_else(|| level_by_player.get(player_id.as_str()).copied())
                .unwrap_or(1);

            // Attributes at this level, and the combat stats derived from them.
            // Each derived stat = class base + (attribute − level-1 baseline) ×
            // coefficient, so a level-1 hero has exactly its class base stats and
            // every level's auto-gained attributes translate into growth. Str →
            // physical atk, Wll → HP + defence, Dex → ATB speed + dodge, Mnd →
            // manifestation/spell power. See balance `[attributes]`.
            let a = &balance.attributes;
            let (str_, mnd, dex, wll) = stats.attributes_at(level);
            let grow = |attr: i32, base: i32, coef: f64| ((attr - base) as f64 * coef).round() as i32;
            let max_hp = max_hp_at_level(*class, level, balance);
            // AD-1: a unique's drawback bites here, and is floored at 1 so a
            // build can be lopsided without becoming unplayable.
            let atk = (stats.base_atk + grow(str_, stats.str, a.str_to_atk) + bonus.atk
                - bonus.penalty_atk)
                .max(1);
            let def = (stats.base_def + grow(wll, stats.wll, a.wll_to_def) + bonus.def
                - bonus.penalty_def)
                .max(0);
            let speed = (stats.speed_stat + grow(dex, stats.dex, a.dex_to_speed) + bonus.spd
                - bonus.penalty_spd)
                .max(1);
            // Spell power keys off the class attack base (gear boosts physical, not
            // psychic) and scales with Mnd.
            let spell_power = stats.base_atk + grow(mnd, stats.mnd, a.mnd_to_power);
            let dodge =
                ((dex - a.dodge_dex_floor).max(0) as f64 * a.dodge_per_dex).clamp(0.0, a.dodge_cap);

            let mut f = Fighter::new(
                combatant_id.clone(),
                CombatantKind::Player,
                Some(player_id.clone()),
                None,
                level,
                max_hp,
                atk,
                def,
                speed,
            );
            f.str_ = str_;
            f.mnd = mnd;
            f.dex = dex;
            f.wll = wll;
            f.spell_power = spell_power;
            f.dodge = dodge;
            // Elemental wards from gear (spec §5): folded + clamped 0.0–2.0.
            f.damage_modifiers = fold_damage_modifiers(&bonus.modifiers);
            // AD-3: a branded weapon types the hero's basic swing, so a party can
            // finally exploit a creature's elemental weakness instead of only
            // resisting its attacks. Creature profiles already existed; heroes had
            // no way to answer them.
            if let Some(el) = bonus.brand.as_deref().and_then(meld_proto::enums::DamageType::from_wire) {
                f.basic_attack_type = el;
            }
            // AD-1 ward affixes: the hero walks into the fight already holding
            // these, which is what makes a ward roll a build rather than a stat.
            f.barrier += bonus.barrier;
            f.regen += bonus.regen;
            if bonus.evasion > 0 {
                f.evasion += bonus.evasion as f64 / 100.0;
            }
            // Synergy affixes pay out only if the ally they name is in THIS party —
            // resolved here because battle assembly is the only place that knows the
            // composition (AD-1 → party builds).
            for (ally, atk, def) in &bonus.synergies {
                if party
                    .iter()
                    .enumerate()
                    .any(|(j, (_, _, c, _))| j != i && class_key(*c) == ally.as_str())
                {
                    f.atk += atk;
                    f.def += def;
                }
            }
            // Surface the class to the client (drives the per-hero command menu).
            f.class_key = class_key(*class).to_string();
            match *class {
                // A Psyker channels Foci instead of the martial kit; its slot count
                // grows with level: base + 1 per `psyker_focus_per_level`, capped.
                // Casters hold the back row (squishy → protected).
                CharacterClass::Psyker => {
                    let bb = &balance.battle;
                    let extra = if bb.psyker_focus_per_level > 0 {
                        (level - 1) / bb.psyker_focus_per_level
                    } else {
                        0
                    };
                    // AD-1 "of the Open Mind": an extra Focus slot is a Psyker's
                    // whole build, so it lands on top of the level curve.
                    f.focus_max = (bb.psyker_focus_base as i32 + extra + bonus.focus_slots)
                        .clamp(bb.psyker_focus_base as i32, bb.psyker_focus_cap as i32)
                        as usize;
                    f.back_row = true;
                }
                // A Resonant regenerates a little HP each of its turns (innate) and
                // stands in the back row.
                CharacterClass::Resonant => {
                    f.regen = balance.battle.resonant_regen_per_turn;
                    f.back_row = true;
                }
                // The Explorer (martial baseline) earns Adrenaline through basic attacks
                // and spends it on skills; it holds the front line. Starts at 0.
                CharacterClass::Explorer => {
                    f.adrenaline_max = balance.battle.explorer_adrenaline_max;
                    // AD-1 "of Fury": walk in with Adrenaline already banked, so the
                    // first turn can be a skill instead of a wind-up attack.
                    f.adrenaline = bonus.adrenaline.min(f.adrenaline_max);
                }
                // Other martial classes hold the front line with no special resource.
                _ => {}
            }
            // The player's saved formation choice overrides the class default.
            if let Some(Some(row)) = row_overrides.get(i) {
                f.back_row = *row;
            }
            // AD-1: a unique that costs max HP costs it here, floored so a hero is
            // never assembled dead. Current HP follows the reduced maximum.
            if bonus.penalty_max_hp > 0 {
                f.max_hp = (f.max_hp - bonus.penalty_max_hp).max(1);
                f.hp = f.hp.min(f.max_hp);
            }
            f
        })
        .collect::<Vec<Fighter>>();
    // AD-2 class-pair synergies: passive, always-on while both classes are in the
    // party. Applied here for the same reason set bonuses are — this is the only
    // place that knows the whole composition.
    let comp: Vec<CharacterClass> = party.iter().map(|(_, _, c, _)| *c).collect();
    let adv = &balance.adventure;
    for syn in meld_proto::synergies::active_synergies(&comp) {
        use meld_proto::synergies::SynergyEffect as E;
        for f in fighters.iter_mut() {
            match syn.effect {
                E::PartyBarrier => f.barrier += adv.synergy_party_barrier,
                E::PartyRegen => f.regen += adv.synergy_party_regen,
                E::BackRowEvasion => {
                    if f.back_row {
                        f.evasion += adv.synergy_back_row_evasion as f64 / 100.0;
                    }
                }
            }
        }
    }
    // AD-1 sets pay the WHOLE party, including other players' heroes in a merged
    // raid — the only bonus in the game that reaches past its owner, which is what
    // makes assembling a set a group project. Collected across every member first,
    // because the payout does not care whose loadout completed it.
    let party_bonus = party
        .iter()
        .flat_map(|(_, _, _, bonus)| meld_proto::uniques::completed_sets(&bonus.set_pieces))
        .fold((0, 0, 0), |(atk, def, spd), s| {
            (atk + s.party_atk, def + s.party_def, spd + s.party_spd)
        });
    if party_bonus != (0, 0, 0) {
        for f in fighters.iter_mut() {
            f.atk += party_bonus.0;
            f.def += party_bonus.1;
            f.speed_stat += party_bonus.2;
        }
    }
    fighters
}

/// One creature joining a battle: its spawn + the combatant id to give it.
pub type EnemyMember<'a> = (&'a MonsterSpawn, Id);

#[allow(clippy::too_many_arguments)]
/// How much tougher a creature is for the size of the party facing it. Indexed off
/// `[runs] encounter_party_scale`; a party larger than the table is clamped to its
/// last entry, and an empty party (impossible in practice) scales by 1.
pub fn encounter_party_scale(party_size: usize, balance: &Balance) -> f64 {
    let table = &balance.runs.encounter_party_scale;
    if table.is_empty() || party_size == 0 {
        return 1.0;
    }
    let idx = (party_size - 1).min(table.len() - 1);
    table[idx].max(0.1)
}

/// Whether this creature is boss-tier — the encounters that should be measured in
/// minutes rather than seconds.
fn is_boss_tier(m: &meld_world::MonsterSpawn) -> bool {
    !m.boss_kind.is_empty()
        || matches!(m.encounter_class.as_str(), "gatekeeper" | "undead_rite")
}

/// The HP a boss needs so that a party attacking flat out spends
/// `[encounters] boss_target_rounds` rounds killing it.
///
/// Derived from the party in front of it rather than from a multiplier, because
/// party damage scales with gear (roughly linear in distance) while creature HP
/// scales with `stat_mult` (a different curve) — any fixed multiple drifts, and
/// measured at the old 7.5x a geared party beat a gatekeeper in 14-67 seconds.
/// Returns 0 when the encounter holds no boss, which leaves ordinary creatures alone.
fn boss_hp_for_target_rounds(
    allies: &[Fighter],
    enemies: &[EnemyMember],
    balance: &Balance,
) -> i32 {
    if !enemies.iter().any(|(m, _)| is_boss_tier(m)) {
        return 0;
    }
    let armour = enemies
        .iter()
        .filter(|(m, _)| is_boss_tier(m))
        .map(|(m, _)| m.def)
        .max()
        .unwrap_or(0);
    // What the party puts out in one round, at the multiplier of a routine heavy hit.
    let per_round: i64 = allies
        .iter()
        .map(|f| {
            let scaled = (f.atk as f64 * balance.battle.skill_power_mult).round() as i32;
            let floored = ((scaled - armour) as f64)
                .max(scaled as f64 * balance.combat_math.damage_floor_fraction)
                .round() as i32;
            floored.max(balance.combat_math.min_damage) as i64
        })
        .sum();
    (per_round * balance.encounters.boss_target_rounds.max(1.0) as i64)
        .clamp(0, i32::MAX as i64) as i32
}

pub fn build_battle(
    battle_id: Id,
    party: &[PartyMember],
    enemies: &[EnemyMember],
    runs: &InstanceRun,
    balance: &Balance,
    seed: u64,
    // Per-hero starting HP, aligned with `party`. `None` means full HP. Used to
    // carry wounds across a run's encounters (no free heal between fights).
    hp_overrides: &[Option<i32>],
    // Per-hero saved formation, aligned with `party` (see [`party_fighters`]).
    row_overrides: &[Option<bool>],
) -> Battle {
    let mut allies = party_fighters(party, runs, balance, row_overrides);
    // Creatures scale with how many heroes are facing them, on top of the distance
    // curve. Four heroes bring ~4x the damage, so a flat encounter would make a full
    // party's fights the SHORTEST in the game; the ramp is superlinear so the arc runs
    // the intended way — quick solo fights early, long ones once the party is full.
    let party_scale = encounter_party_scale(allies.len(), balance);
    // A BOSS is measured in time, not in a health multiplier. Party damage scales
    // with gear and creature HP with distance — different curves, so a fixed multiple
    // drifts into 14-second "boss" fights. Size it here instead, where what the party
    // can actually put out is known.
    let boss_floor_hp = boss_hp_for_target_rounds(&allies, enemies, balance);
    for (f, hp) in allies.iter_mut().zip(hp_overrides.iter()) {
        if let Some(h) = hp {
            f.hp = (*h).clamp(0, f.max_hp);
        }
    }

    // One enemy Fighter per grouped creature, carrying its faction + flee flag so
    // the battle can pit factions against each other.
    let enemy_fighters: Vec<Fighter> = enemies
        .iter()
        .map(|(m, cid)| {
            // FS-4 unique boss mechanics: an Elite/Gatekeeper carrying a named
            // boss identity fights under that boss's own title and kit instead
            // of its base biome creature's — the affix still prefixes it, so a
            // champion-tier boss can read "Vicious Ironmaw".
            let base_name = if m.boss_kind.is_empty() {
                m.monster_kind.as_str()
            } else {
                meld_world::boss_display_name(&m.boss_kind)
            };
            let name = if m.affix.is_empty() {
                base_name.to_string()
            } else {
                format!("{} {}", m.affix, base_name)
            };
            let mut f = Fighter::new(
                cid.clone(),
                CombatantKind::Monster,
                None,
                Some(name),
                m.level,
                {
                    let scaled = ((m.hp as f64) * party_scale).round() as i32;
                    // Only a BOSS gets the time budget, and only ever upward: a boss
                    // is never made weaker than its own stat line.
                    if is_boss_tier(m) {
                        scaled.max(boss_floor_hp)
                    } else {
                        scaled
                    }
                },
                ((m.atk as f64) * party_scale).round() as i32,
                m.def,
                m.speed_stat,
            );
            f.faction = m.faction.clone();
            f.flees = m.flees;
            // CR-6: carry the creature's pack role into the fight, so the engine can
            // shield a leader with its minions and rout them when it falls.
            f.pack_role = meld_proto::enums::PackRole::from_encounter_class(&m.encounter_class);
            // PG-2: a named boss met deeper wears a darker palette. The band comes
            // from the level it is met at, which is the same axis its deep-gated
            // abilities come online on — so look and kit escalate together.
            if !m.boss_kind.is_empty() {
                f.boss_band = meld_world::abilities::boss_palette_band(m.level);
            }
            // Creature AI content (spec §1/§2): the kind's permanent ability
            // pool (level-gated at selection time), its elemental profile,
            // and its typed basic swing. Keyed by the BASE kind — a champion
            // ("Swift dune wyrm") shares its kind's pool — unless it carries a
            // named boss identity (FS-4), which gets its own bespoke kit.
            let ability_key = if m.boss_kind.is_empty() { &m.monster_kind } else { &m.boss_kind };
            f.boss_kind = m.boss_kind.clone();
            f.abilities = meld_world::abilities::creature_abilities(ability_key);
            f.damage_modifiers = meld_world::abilities::creature_damage_modifiers(ability_key)
                .into_iter()
                .collect();
            f.basic_attack_type =
                meld_world::abilities::creature_basic_attack_type(ability_key);
            f
        })
        .collect();

    // The encounter class is the strongest present (gatekeeper > elite > standard).
    let encounter_class = enemies
        .iter()
        .map(|(m, _)| match m.encounter_class.as_str() {
            "gatekeeper" => EncounterClass::Gatekeeper,
            // The undead rite is champion-tier: reporting it as Standard would let
            // it be fled like trash and read as trash on the wire.
            "elite" | "undead_rite" => EncounterClass::Elite,
            _ => EncounterClass::Standard,
        })
        .max_by_key(|c| match c {
            EncounterClass::Gatekeeper => 2,
            EncounterClass::Elite => 1,
            EncounterClass::Standard => 0,
        })
        .unwrap_or(EncounterClass::Standard);

    Battle::new(battle_id, encounter_class, allies, enemy_fighters, balance, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_run_levels_match_canon() {
        let b = Balance::load_default().unwrap();
        assert_eq!(base_run_level(0, &b), 1);
        assert_eq!(base_run_level(500, &b), 40);
    }

    #[test]
    fn xp_award_levels_up() {
        let b = Balance::load_default().unwrap();
        let mut r = PlayerRun {
            run_id: "r".into(),
            player_id: "p".into(),
            username: "u".into(),
            character_class: CharacterClass::Explorer,
            run_level: 1,
            xp: 0,
            backpack: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
            result: None,
            party_id: 0,
            hero_levels: Vec::new(),
            hero_xp: Vec::new(),
        };
        // Award exactly the first two levels' cost, then one XP short of the third:
        // two levels gained, and the remainder carries.
        let l1 = xp_to_next(1, &b);
        let l2 = xp_to_next(2, &b);
        let gained = r.award_xp(l1 + l2 + 3, &b);
        assert_eq!(gained, 2, "l1={l1} l2={l2}");
        assert_eq!(r.run_level, 3);
        assert_eq!(r.xp, 3, "the remainder should carry toward the next level");
    }

    #[test]
    fn a_level_costs_exactly_its_number_of_same_level_fights() {
        let b = Balance::load_default().unwrap();
        // The design statement, asserted directly: two fights clear level 1, three
        // clear level 2, four clear level 3.
        assert_eq!(fights_per_level(1, &b), 2);
        assert_eq!(fights_per_level(2, &b), 3);
        assert_eq!(fights_per_level(3, &b), 4);
        assert_eq!(fights_per_level(30, &b), 31);

        // And the XP cost is exactly that many same-level encounters — so a player
        // who fights at their own level advances on schedule rather than on a curve
        // nobody can feel.
        for level in [1, 2, 5, 20, 100] {
            let one_fight = same_level_encounter_xp(level, &b);
            let expected = one_fight * fights_per_level(level, &b) as i64;
            assert_eq!(
                xp_to_next(level, &b),
                expected,
                "level {level}: {} per fight",
                one_fight
            );
        }

        // A same-level encounter pays more the deeper it is (a level-20 fight is a
        // level-20 fight, wherever the ladder sits), so the curve rises with it.
        assert!(same_level_encounter_xp(1, &b) < same_level_encounter_xp(20, &b));
        assert!(xp_to_next(1, &b) < xp_to_next(20, &b));
        assert!(xp_to_next(20, &b) < xp_to_next(255, &b));
    }

    #[test]
    fn fighting_over_your_level_advances_you_faster() {
        let b = Balance::load_default().unwrap();
        // Ten same-level fights at level 1 should carry a hero several levels, since
        // each of the early levels only wants two or three of them. This is the
        // player-facing promise: punch up and you climb quicker.
        let mut r = PlayerRun {
            run_id: "r".into(),
            player_id: "p".into(),
            username: "u".into(),
            character_class: CharacterClass::Explorer,
            run_level: 1,
            xp: 0,
            backpack: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
            result: None,
            party_id: 0,
            hero_levels: Vec::new(),
            hero_xp: Vec::new(),
        };
        let one = same_level_encounter_xp(1, &b);
        let gained = r.award_xp(one * 10, &b);
        assert!(
            gained >= 3,
            "ten level-1 fights bought only {gained} levels ({one} XP each)"
        );
        assert!(r.run_level > 3);
    }

    /// A one-hero party at a given level, for attribute-derivation assertions.
    fn solo_fighter(class: CharacterClass, level: i32, b: &Balance) -> Fighter {
        let mut runs = InstanceRun::new("i".into(), 0, b);
        runs.add_party(vec![("p".into(), "u".into(), class, "r".into())]);
        runs.runs[0].run_level = level;
        let party: Vec<PartyMember> = vec![("p".into(), "c".into(), class, GearBonus::default())];
        party_fighters(&party, &runs, b, &[]).pop().unwrap()
    }

    #[test]
    fn level_one_matches_class_base_stats() {
        // The whole point of the derivation: a level-1 hero equals its raw class
        // base stats, so nothing about the existing balance shifts.
        let b = Balance::load_default().unwrap();
        for class in [
            CharacterClass::Explorer,
            CharacterClass::Psyker,
            CharacterClass::Resonant,
        ] {
            let s = b.player.get(class_key(class)).unwrap();
            let f = solo_fighter(class, 1, &b);
            assert_eq!(f.max_hp, s.base_hp, "{:?} hp", class);
            assert_eq!(f.atk, s.base_atk, "{:?} atk", class);
            assert_eq!(f.def, s.base_def, "{:?} def", class);
            assert_eq!(f.speed_stat, s.speed_stat, "{:?} speed", class);
            // Manifestation power keys off the class attack base at level 1.
            assert_eq!(f.spell_power, s.base_atk, "{:?} spell", class);
            assert_eq!(f.dodge, 0.0, "{:?} dodge", class);
        }
    }

    #[test]
    fn shifter_starts_slippery_and_front_row() {
        // The Shifter is the one class whose base Dex clears the dodge floor, so it
        // dodges from level 1 (every other class starts at 0.0 — see the test above),
        // and it holds the front line (not a back-row caster).
        let b = Balance::load_default().unwrap();
        let sh1 = solo_fighter(CharacterClass::Shifter, 1, &b);
        assert!(sh1.dodge > 0.0, "the Shifter has innate dodge at level 1");
        assert!(!sh1.back_row, "the Shifter is a front-row skirmisher");
        // Leveling deepens the evasion + keeps it the fastest gauge.
        let sh5 = solo_fighter(CharacterClass::Shifter, 5, &b);
        assert!(sh5.dodge > sh1.dodge, "dodge grows with Dex");
        assert!(sh5.speed_stat > sh1.speed_stat, "the gauge fills faster with Dex");
    }

    #[test]
    fn explorer_starts_with_an_empty_adrenaline_pool() {
        // The martial baseline earns its resource in-battle: the pool exists (max
        // from balance) but starts empty, and it holds the front line.
        let b = Balance::load_default().unwrap();
        let h = solo_fighter(CharacterClass::Explorer, 1, &b);
        assert_eq!(h.adrenaline_max, b.battle.explorer_adrenaline_max);
        assert_eq!(h.adrenaline, 0, "Adrenaline is banked in-fight, not granted");
        assert!(!h.back_row, "the Explorer holds the front line");
    }

    #[test]
    fn leveling_grows_stats_per_class_focus() {
        let b = Balance::load_default().unwrap();
        let sq1 = solo_fighter(CharacterClass::Explorer, 1, &b);
        let sq5 = solo_fighter(CharacterClass::Explorer, 5, &b);
        // The Explorer hardens: Str -> more atk, Wll -> more HP.
        assert!(sq5.atk > sq1.atk, "explorer atk grows with Str");
        assert!(sq5.max_hp > sq1.max_hp, "explorer HP grows with Wll");
        assert!(sq5.str_ > sq1.str_ && sq5.wll > sq1.wll);

        // The Psyker's manifestation power grows with Mnd, not its atk.
        let ps1 = solo_fighter(CharacterClass::Psyker, 1, &b);
        let ps5 = solo_fighter(CharacterClass::Psyker, 5, &b);
        assert!(ps5.spell_power > ps1.spell_power, "psyker spell power grows");
        assert_eq!(ps5.atk, ps1.atk, "psyker gains no Str, so atk is flat");
        assert!(ps5.mnd > ps1.mnd);
    }

    #[test]
    fn build_battle_applies_hp_overrides() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![(
            "p1".into(),
            "u1".into(),
            CharacterClass::Explorer,
            "r1".into(),
        )]);
        // Use a real generated creature as the enemy.
        let arena = meld_world::Arena::generate(&b, 5, true);
        let enemies = vec![(&arena.monsters[0], "mc".to_string())];
        let party: Vec<PartyMember> = vec![("p1".into(), "c1".into(), CharacterClass::Explorer, GearBonus::default())];
        // Carry a wounded hero in: start at 17 HP rather than full.
        let battle = build_battle("b".into(), &party, &enemies, &runs, &b, 1, &[Some(17)], &[]);
        let (allies, _) = battle.wire_combatants();
        assert_eq!(allies.len(), 1);
        assert_eq!(allies[0].hp, 17, "wounded HP carried into the new battle");
        assert!(allies[0].max_hp > 17, "max HP stays at the class base");
    }

    /// FS-4 unique boss mechanics: a Gatekeeper's `boss_kind` (assigned in
    /// world-gen) drives the assembled Fighter's abilities/damage-modifiers/
    /// basic-attack — its own named-boss kit, not its base biome creature's —
    /// and its display name reads as the boss's title.
    #[test]
    fn boss_kind_drives_the_assembled_fighters_kit_and_name() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![("p1".into(), "u1".into(), CharacterClass::Explorer, "r1".into())]);
        let mut arena = meld_world::Arena::generate(&b, 7, false);
        arena.ensure_frontier(&b, 400.0); // cross a biome seam so a gatekeeper spawns
        let gk = arena
            .monsters
            .iter()
            .find(|m| m.encounter_class == "gatekeeper")
            .expect("a gatekeeper spawned at the crossed seam");
        assert!(!gk.boss_kind.is_empty(), "gatekeeper carries a boss identity");

        let enemies = vec![(gk, "mc".to_string())];
        let party: Vec<PartyMember> = vec![("p1".into(), "c1".into(), CharacterClass::Explorer, GearBonus::default())];
        let battle = build_battle("b".into(), &party, &enemies, &runs, &b, 1, &[], &[]);
        let (_, wire_enemies) = battle.wire_combatants();
        let boss = &wire_enemies[0];

        let boss_title = meld_world::boss_display_name(&gk.boss_kind);
        assert!(
            boss.monster_kind.as_deref().unwrap_or("").contains(boss_title),
            "display name uses the boss title, got {:?}", boss.monster_kind
        );
        assert!(
            boss.statuses.contains(&format!("boss:{}", gk.boss_kind)),
            "wire status carries the boss identity"
        );
        // The boss's own kit is used, not its base biome creature's.
        let boss_abilities = meld_world::abilities::creature_abilities(&gk.boss_kind);
        let base_abilities = meld_world::abilities::creature_abilities(&gk.monster_kind);
        assert_ne!(boss_abilities, base_abilities, "boss kit differs from the base creature's");
    }

    #[test]
    fn row_override_beats_the_class_default() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![
            ("p".into(), "u".into(), CharacterClass::Psyker, "r1".into()),
            ("p".into(), "u".into(), CharacterClass::Explorer, "r2".into()),
        ]);
        let party: Vec<PartyMember> = vec![
            ("p".into(), "c1".into(), CharacterClass::Psyker, GearBonus::default()), // class default: back
            ("p".into(), "c2".into(), CharacterClass::Explorer, GearBonus::default()), // class default: front
        ];
        // Override: send the Psyker to the front and pull the Explorer to the back.
        let fighters = party_fighters(&party, &runs, &b, &[Some(false), Some(true)]);
        assert!(!fighters[0].back_row, "Psyker forced to the front row");
        assert!(fighters[1].back_row, "Explorer forced to the back row");
        // An absent/None override keeps the class default.
        let dflt = party_fighters(&party, &runs, &b, &[]);
        assert!(dflt[0].back_row, "Psyker keeps its back-row default");
        assert!(!dflt[1].back_row, "Explorer keeps its front-row default");
    }

    #[test]
    fn gear_bonus_adds_into_atk_def_speed() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::Explorer, "r".into())]);
        let bare: Vec<PartyMember> =
            vec![("p".into(), "c".into(), CharacterClass::Explorer, GearBonus::default())];
        let geared: Vec<PartyMember> = vec![(
            "p".into(),
            "c".into(),
            CharacterClass::Explorer,
            GearBonus { atk: 5, def: 3, spd: 2, ..Default::default() },
        )];
        let f0 = party_fighters(&bare, &runs, &b, &[]).pop().unwrap();
        let f1 = party_fighters(&geared, &runs, &b, &[]).pop().unwrap();
        assert_eq!(f1.atk, f0.atk + 5);
        assert_eq!(f1.def, f0.def + 3);
        assert_eq!(f1.speed_stat, f0.speed_stat + 2);
    }

    #[test]
    fn ward_and_keyword_affixes_reach_the_fighter() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::Explorer, "r".into())]);
        let warded: Vec<PartyMember> = vec![(
            "p".into(),
            "c".into(),
            CharacterClass::Explorer,
            GearBonus {
                barrier: 12,
                regen: 3,
                evasion: 10,
                adrenaline: 4,
                ..Default::default()
            },
        )];
        let f = party_fighters(&warded, &runs, &b, &[]).pop().unwrap();
        // The hero walks in already holding the wards — that is the build.
        assert_eq!(f.barrier, 12);
        assert!(f.regen >= 3);
        assert!((f.evasion - 0.10).abs() < 1e-9, "evasion {}", f.evasion);
        // "of Fury": Adrenaline banked before the first turn, capped by the max.
        assert_eq!(f.adrenaline, 4.min(f.adrenaline_max));
    }

    #[test]
    fn a_synergy_affix_pays_out_only_when_its_ally_is_in_the_party() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::Explorer, "r".into())]);
        let synergy = GearBonus {
            synergies: vec![("resonant".to_string(), 6, 2)],
            ..Default::default()
        };
        let hero = |class: CharacterClass, bonus: GearBonus| -> PartyMember {
            ("p".into(), "c".into(), class, bonus)
        };
        // Alone, the affix is inert…
        let solo = vec![hero(CharacterClass::Explorer, synergy.clone())];
        let f_solo = party_fighters(&solo, &runs, &b, &[]).pop().unwrap();
        // …with the named ally alongside, it pays.
        let mixed = vec![
            hero(CharacterClass::Explorer, synergy.clone()),
            hero(CharacterClass::Resonant, GearBonus::default()),
        ];
        let f_mixed = party_fighters(&mixed, &runs, &b, &[]).remove(0);
        assert_eq!(f_mixed.atk, f_solo.atk + 6);
        assert_eq!(f_mixed.def, f_solo.def + 2);
        // A different ally does not satisfy it.
        let wrong = vec![
            hero(CharacterClass::Explorer, synergy),
            hero(CharacterClass::Shifter, GearBonus::default()),
        ];
        let f_wrong = party_fighters(&wrong, &runs, &b, &[]).remove(0);
        assert_eq!(f_wrong.atk, f_solo.atk);
    }

    #[test]
    fn a_unique_s_drawback_costs_what_it_says() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::Explorer, "r".into())]);
        let plain: Vec<PartyMember> =
            vec![("p".into(), "c".into(), CharacterClass::Explorer, GearBonus::default())];
        let f0 = party_fighters(&plain, &runs, &b, &[]).pop().unwrap();

        // "Reaver's Edge": +22 atk, -12 def.
        let reaver: Vec<PartyMember> = vec![(
            "p".into(),
            "c".into(),
            CharacterClass::Explorer,
            GearBonus { atk: 22, penalty_def: 12, ..Default::default() },
        )];
        let f1 = party_fighters(&reaver, &runs, &b, &[]).pop().unwrap();
        assert_eq!(f1.atk, f0.atk + 22);
        assert_eq!(f1.def, (f0.def - 12).max(0));

        // A max-HP drawback never assembles a dead hero.
        let brutal: Vec<PartyMember> = vec![(
            "p".into(),
            "c".into(),
            CharacterClass::Explorer,
            GearBonus { penalty_max_hp: 99_999, ..Default::default() },
        )];
        let f2 = party_fighters(&brutal, &runs, &b, &[]).pop().unwrap();
        assert_eq!(f2.max_hp, 1);
        assert!(f2.hp >= 1 && f2.hp <= f2.max_hp);
    }

    #[test]
    fn a_completed_set_pays_every_hero_in_the_party() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::Explorer, "r".into())]);
        let member = |class: CharacterClass, bonus: GearBonus| -> PartyMember {
            ("p".into(), "c".into(), class, bonus)
        };
        // Kiln Chorus: 2 pieces, +5 atk to the whole party.
        let two_pieces = GearBonus {
            set_pieces: vec![("kiln_chorus".to_string(), 2)],
            ..Default::default()
        };
        let one_piece = GearBonus {
            set_pieces: vec![("kiln_chorus".to_string(), 1)],
            ..Default::default()
        };
        let bare = vec![
            member(CharacterClass::Explorer, GearBonus::default()),
            member(CharacterClass::Resonant, GearBonus::default()),
        ];
        let base = party_fighters(&bare, &runs, &b, &[]);

        // One hero wears the set; BOTH heroes get the bonus.
        let with_set = vec![
            member(CharacterClass::Explorer, two_pieces),
            member(CharacterClass::Resonant, GearBonus::default()),
        ];
        let paid = party_fighters(&with_set, &runs, &b, &[]);
        assert_eq!(paid[0].atk, base[0].atk + 5);
        assert_eq!(paid[1].atk, base[1].atk + 5, "the ally shares the set bonus");

        // An incomplete set pays nobody.
        let partial = vec![
            member(CharacterClass::Explorer, one_piece),
            member(CharacterClass::Resonant, GearBonus::default()),
        ];
        let unpaid = party_fighters(&partial, &runs, &b, &[]);
        assert_eq!(unpaid[0].atk, base[0].atk);
        assert_eq!(unpaid[1].atk, base[1].atk);
    }

    #[test]
    fn a_branded_weapon_types_the_hero_s_swing() {
        use meld_proto::enums::DamageType;
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::Explorer, "r".into())]);
        // Unbranded, a hero's swing is untyped — it can never hit a weakness.
        let plain: Vec<PartyMember> =
            vec![("p".into(), "c".into(), CharacterClass::Explorer, GearBonus::default())];
        let f0 = party_fighters(&plain, &runs, &b, &[]).pop().unwrap();
        assert_eq!(f0.basic_attack_type, DamageType::None);

        let branded: Vec<PartyMember> = vec![(
            "p".into(),
            "c".into(),
            CharacterClass::Explorer,
            GearBonus { brand: Some("FIRE".into()), ..Default::default() },
        )];
        let f1 = party_fighters(&branded, &runs, &b, &[]).pop().unwrap();
        assert_eq!(f1.basic_attack_type, DamageType::Fire);

        // Nonsense on the wire leaves the swing untyped rather than panicking.
        let junk: Vec<PartyMember> = vec![(
            "p".into(),
            "c".into(),
            CharacterClass::Explorer,
            GearBonus { brand: Some("NOT_AN_ELEMENT".into()), ..Default::default() },
        )];
        let f2 = party_fighters(&junk, &runs, &b, &[]).pop().unwrap();
        assert_eq!(f2.basic_attack_type, DamageType::None);
    }

    #[test]
    fn a_class_pair_synergy_arms_the_whole_party() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::PhoenixGuard, "r".into())]);
        let member = |class: CharacterClass| -> PartyMember {
            ("p".into(), "c".into(), class, GearBonus::default())
        };
        // No Psyker: no Fortress Front, so nobody opens warded.
        let no_pair = vec![member(CharacterClass::PhoenixGuard), member(CharacterClass::Shifter)];
        let bare = party_fighters(&no_pair, &runs, &b, &[]);
        assert!(bare.iter().all(|f| f.barrier == 0), "unpaired party opened warded");

        // Phoenix Guard + Psyker: EVERY hero opens with the synergy's Barrier, not just
        // the two that formed the pair.
        let paired = vec![
            member(CharacterClass::PhoenixGuard),
            member(CharacterClass::Psyker),
            member(CharacterClass::Shifter),
        ];
        let armed = party_fighters(&paired, &runs, &b, &[]);
        let want = b.adventure.synergy_party_barrier;
        for f in &armed {
            assert!(f.barrier >= want, "{} opened with {}", f.combatant_id, f.barrier);
        }

        // Blood and Balm (Resonant + Hunter) gives the party Regen — a kit that pays
        // in Adrenaline and blood beside one that gives it back. The Resonant's
        // innate Regen is on top, not replaced.
        let sustained = vec![member(CharacterClass::Hunter), member(CharacterClass::Resonant)];
        let f = party_fighters(&sustained, &runs, &b, &[]);
        assert!(f[0].regen >= b.adventure.synergy_party_regen, "hunter regen {}", f[0].regen);
    }


    #[test]
    fn each_hero_climbs_its_own_ladder_and_the_fallen_climb_nothing() {
        let b = Balance::load_default().unwrap();
        let mut r = PlayerRun {
            run_id: "r".into(),
            player_id: "p".into(),
            username: "u".into(),
            character_class: CharacterClass::Explorer,
            run_level: 1,
            xp: 0,
            backpack: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
            result: None,
            party_id: 0,
            hero_levels: Vec::new(),
            hero_xp: Vec::new(),
        };
        let one = same_level_encounter_xp(1, &b);

        // Hero 0 fights; heroes 1-3 do not (or fell). Only hero 0 climbs. The award is
        // SPLIT four ways, so this is twenty-four encounters' worth arriving as six.
        let gained = r.award_hero_xp(0, 4, one * 6 * 4, &b);
        assert!(gained >= 2, "six level-1 fights bought {gained} levels");
        assert!(r.hero_level(0) > r.hero_level(1), "the idle hero climbed too");
        assert_eq!(r.hero_level(1), 1, "an unearned hero should sit at the base level");

        // The headline level follows the BEST hero, so one-number messages stay true.
        assert_eq!(r.run_level, r.hero_level(0));

        // Zero XP is a no-op — a dead hero is simply never passed here.
        let before = r.hero_level(0);
        assert_eq!(r.award_hero_xp(0, 4, 0, &b), 0);
        assert_eq!(r.hero_level(0), before);

        // The slot-unlock rules count heroes at a level.
        assert_eq!(r.heroes_at_level(before), 1);
        assert_eq!(r.heroes_at_level(1), 4);
        let _ = r.award_hero_xp(1, 4, one * 6 * 4, &b);
        assert_eq!(r.heroes_at_level(before), 2, "two heroes should now be there");

        // The cap holds per hero.
        let _ = r.award_hero_xp(2, 4, i64::MAX / 4, &b);
        assert_eq!(r.hero_level(2), b.runs.max_hero_level);
    }




    #[test]
    fn a_lone_hero_learns_the_whole_lesson_and_four_split_it() {
        let b = Balance::load_default().unwrap();
        let mk = |size: usize| {
            let mut r = PlayerRun {
                run_id: "r".into(),
                player_id: "p".into(),
                username: "u".into(),
                character_class: CharacterClass::Hunter,
                run_level: 1,
                xp: 0,
                backpack: vec![],
                chits: 0,
                looted_gear: vec![],
                max_distance_reached: 0,
                result: None,
                party_id: 0,
                hero_levels: vec![1; size],
                hero_xp: vec![0; size],
            };
            r.award_hero_xp(0, size, 400, &b);
            r.hero_xp[0] + (0..r.hero_levels[0] - 1).map(|l| xp_to_next(l + 1, &b)).sum::<i64>()
        };
        let solo = mk(1);
        let four = mk(4);
        assert!(
            solo > four,
            "a solo hero should absorb more of an encounter than one of four: {solo} vs {four}"
        );
        // Specifically: the whole thing versus a quarter of it.
        assert_eq!(solo, 400);
        assert_eq!(four, 100);
    }

    #[test]
    fn creatures_get_tougher_as_the_party_grows_so_the_arc_runs_the_right_way() {
        let b = Balance::load_default().unwrap();
        // Four heroes bring roughly four times the damage. If encounters did not
        // scale, a full party's fights would be the SHORTEST in the game — the exact
        // opposite of the intended arc (quick solo fights early, long ones late).
        let solo = encounter_party_scale(1, &b);
        let full = encounter_party_scale(4, &b);
        assert_eq!(solo, 1.0, "a lone hero faces the creature as written");
        assert!(full > solo * 3.0, "a full party's encounters barely grew: {full}");
        // Monotonic, so every unlocked slot makes the world push back harder.
        for n in 1..4 {
            assert!(
                encounter_party_scale(n + 1, &b) > encounter_party_scale(n, &b),
                "party of {} is not harder than {n}",
                n + 1
            );
        }
        // Out-of-range party sizes clamp instead of panicking.
        assert_eq!(encounter_party_scale(9, &b), full);
        assert_eq!(encounter_party_scale(0, &b), 1.0);
    }

    #[test]
    fn the_same_creature_hits_harder_when_more_heroes_are_present() {
        let b = Balance::load_default().unwrap();
        let hp_of = |size: usize| {
            let party: Vec<PartyMember> = (0..size)
                .map(|i| {
                    (
                        format!("p{i}"),
                        format!("c{i}"),
                        CharacterClass::Hunter,
                        GearBonus::default(),
                    )
                })
                .collect();
            let mut runs = InstanceRun::new("i".into(), 0, &b);
            runs.add_party(
                (0..size)
                    .map(|i| (format!("p{i}"), "u".into(), CharacterClass::Hunter, "r".into()))
                    .collect(),
            );
            let m = meld_world::MonsterSpawn::dungeon_boss(&b, "m".into(), "forest", "", 200, 7);
            let battle = build_battle(
                "b".into(),
                &party,
                &[(&m, "e0".to_string())],
                &runs,
                &b,
                1,
                &[],
                &[],
            );
            battle.combatant_hp("e0").unwrap_or(0)
        };
        assert!(
            hp_of(4) > hp_of(1),
            "the same creature was no tougher against four heroes"
        );
    }



    #[test]
    fn a_full_pack_refuses_new_kinds_but_still_stacks_what_it_has() {
        let b = Balance::load_default().unwrap();
        let mut r = PlayerRun {
            run_id: "r".into(),
            player_id: "p".into(),
            username: "u".into(),
            character_class: CharacterClass::Hunter,
            run_level: 1,
            xp: 0,
            backpack: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
            result: None,
            party_id: 0,
            hero_levels: vec![1],
            hero_xp: vec![0],
        };
        let item = |kind: &str| ItemStack {
            item_id: format!("i-{kind}"),
            item_kind: kind.to_string(),
            quantity: 1,
            insurance: None,
        };
        let slots = r.carry_capacity(&b);
        for n in 0..slots {
            assert!(r.try_carry(item(&format!("kind{n}")), &b), "slot {n} refused early");
        }
        assert!(r.pack_full(&b));
        // A NEW kind has nowhere to go — and the caller is told, rather than the drop
        // vanishing silently.
        assert!(!r.try_carry(item("one_too_many"), &b));
        assert_eq!(r.backpack.len(), slots);
        // But more of something already carried always fits: stacks are not slots.
        assert!(r.try_carry(item("kind0"), &b));
        assert_eq!(r.backpack.len(), slots, "a merged stack consumed a slot");
        assert_eq!(
            r.backpack.iter().find(|s| s.item_kind == "kind0").unwrap().quantity,
            2
        );
    }


    #[test]
    fn a_boss_is_measured_in_minutes_and_an_ordinary_creature_is_not() {
        let b = Balance::load_default().unwrap();
        let party_of = |gear: i32| -> Vec<PartyMember> {
            (0..4)
                .map(|i| {
                    (
                        format!("p{i}"),
                        format!("c{i}"),
                        CharacterClass::Hunter,
                        GearBonus { atk: gear, ..Default::default() },
                    )
                })
                .collect()
        };
        let runs = {
            let mut r = InstanceRun::new("i".into(), 0, &b);
            r.add_party(
                (0..4)
                    .map(|i| (format!("p{i}"), "u".into(), CharacterClass::Hunter, "r".into()))
                    .collect(),
            );
            r
        };
        let rounds = |gear: i32, boss: bool, d: i64| -> f64 {
            let party = party_of(gear);
            let m = if boss {
                meld_world::MonsterSpawn::dungeon_boss(&b, "m".into(), "forest", "choirmother", d, 7)
            } else {
                meld_world::MonsterSpawn::dungeon_boss(&b, "m".into(), "forest", "", d, 7)
            };
            let battle = build_battle(
                "b".into(),
                &party,
                &[(&m, "e0".to_string())],
                &runs,
                &b,
                1,
                &[],
                &[],
            );
            let hp = battle.combatant_hp("e0").unwrap_or(0) as f64;
            let f = &party_fighters(&party, &runs, &b, &[])[0];
            let per = (f.atk as f64 * b.battle.skill_power_mult).round().max(1.0) * 4.0;
            (hp / per).ceil()
        };
        // A boss lasts long enough to BE a boss, at any gear level — that is the whole
        // point of sizing it off the party instead of off a multiplier.
        for gear in [0i32, 210, 840] {
            let r = rounds(gear, true, 1000);
            assert!(
                r >= b.encounters.boss_target_rounds * 0.75,
                "a boss fell in {r} rounds with {gear} gear attack"
            );
        }
        // An ordinary creature is emphatically NOT a boss. Taken from a real arena,
        // because `dungeon_boss` promotes whatever it builds to gatekeeper tier.
        let arena = meld_world::Arena::generate(&b, 7, false);
        let ordinary = arena
            .monsters
            .iter()
            .find(|m| m.encounter_class == "standard" && m.boss_kind.is_empty())
            .expect("a standard creature exists");
        assert!(!is_boss_tier(ordinary), "a standard creature reads as boss-tier");
        let party = party_of(210);
        let battle = build_battle(
            "b".into(),
            &party,
            &[(ordinary, "e0".to_string())],
            &runs,
            &b,
            1,
            &[],
            &[],
        );
        let hp = battle.combatant_hp("e0").unwrap_or(0) as f64;
        let f = &party_fighters(&party, &runs, &b, &[])[0];
        let per = (f.atk as f64 * b.battle.skill_power_mult).round().max(1.0) * 4.0;
        assert!(
            (hp / per).ceil() < b.encounters.boss_target_rounds * 0.5,
            "an ordinary creature is being treated as a boss"
        );
    }

    #[test]
    fn capacity_is_the_shared_bag_plus_a_pouch_per_hero() {
        let b = Balance::load_default().unwrap();
        let cap = |heroes: usize| {
            let r = PlayerRun {
                run_id: "r".into(),
                player_id: "p".into(),
                username: "u".into(),
                character_class: CharacterClass::Hunter,
                run_level: 1,
                xp: 0,
                backpack: vec![],
                chits: 0,
                looted_gear: vec![],
                max_distance_reached: 0,
                result: None,
                party_id: 0,
                hero_levels: vec![1; heroes],
                hero_xp: vec![0; heroes],
            };
            r.carry_capacity(&b)
        };
        let bag = b.runs.backpack_slots as usize;
        let pouch = b.runs.hero_pouch_slots as usize;
        assert_eq!(cap(1), bag + pouch);
        assert_eq!(cap(4), bag + 4 * pouch);
        // A bigger party carries more — but the bag is the bulk of it, so a fourth
        // hero does not quadruple what you can haul out.
        assert!(cap(4) < cap(1) * 2, "a full party carries too much more than a solo");
    }
}
