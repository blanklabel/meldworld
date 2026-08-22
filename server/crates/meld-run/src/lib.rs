//! Run & instance lifecycle for the spike (docs/behaviors/run-lifecycle.md subset).
//!
//! Provides: base-run-level derivation, per-player ephemeral run state
//! (backpack + run level/XP), the victory/defeat outcome transitions, and the
//! bridge that assembles a [`meld_battle::Battle`] from an arena monster and a
//! party. Extraction channels, death durability (HTTP/DB side), and abandon are
//! the next slices; the run/battle spine they hang off is here.

pub mod ability_effects;

use std::collections::HashMap;

use meld_balance::Balance;
use meld_battle::{Battle, Fighter};
use meld_proto::common::{ItemStack, LootGear};
use meld_proto::enums::{
    CharacterClass, CombatantKind, DamageType, EncounterClass, Insurance, RunResult,
    TargetProfile,
};
use meld_proto::Id;
use meld_world::MonsterSpawn;

/// `base_run_level(hub) = round(1 + hub.distance × per_distance)` (CANON.md §B).
pub fn base_run_level(distance: i32, balance: &Balance) -> i32 {
    (1.0 + distance as f64 * balance.runs.base_run_level_per_distance).round() as i32
}

/// XP needed to advance from level `L`: `xp_base × xp_growth_factor^(L-1)`
/// (CANON.md §B) — the classic "double the requirement each level" curve.
pub fn xp_to_next(level: i32, balance: &Balance) -> i64 {
    let fights = fights_per_level(level, balance) as f64;
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
    // How big an encounter IS at that depth comes from the group ramp itself, not a
    // second constant that can drift from it. A flat 2.0 priced every level as a pack,
    // but the ramp keeps the first 150 tiles as duels — so level 1 cost twice the
    // fights the design statement promises.
    let group = balance.encounters.expected_group_size(distance);
    (r.xp_reference_creature * mult * group).round().max(1.0) as i64
}

/// How many same-level fights level `level` costs — the design statement itself:
/// `base` fights, plus one more every `ramp` levels, and **one more every
/// `ramp_late` levels once past `knee`**.
///
/// The ramp is deliberately gentle through the first act. The gate on your second
/// party slot is a hero at level 10, and under the old `(L + 1)` shape that cost 54
/// at-level fights — the entire opening of the game spent as one hero alone, which
/// is the least interesting configuration the game has. Here it costs 22, and the
/// ramp still has teeth further out (65 to level 20, 128 to level 30).
///
/// **Then it PLATEAUS.** Two slopes, the same shape [`xp_after_level_gap`] uses for
/// punching up, and for the same reason: one rate cannot serve both ends of a 255-level
/// ladder. Past the knee a level buys stats and nothing else — a martial class's
/// ability ladder tops out at 100 ([`meld_proto::skills::ladder_top`]) — so charging
/// an ever-steeper price for it is charging more for less. It still rises, because
/// out-levelling the ground is the route `AD-7` opened for a party that cannot get the
/// gear, and a ladder that went flat at 100 would make it the *only* route.
pub fn fights_per_level(level: i32, balance: &Balance) -> i32 {
    let r = &balance.runs;
    let level = level.max(1);
    let knee = r.fights_per_level_knee.max(1);
    let early = (level.min(knee) - 1) / r.fights_per_level_ramp.max(1);
    let late = (level - knee).max(0) / r.fights_per_level_ramp_late.max(1);
    (r.fights_per_level_base + early + late).max(1)
}

/// Total XP to climb from level 1 to `level`.
pub fn xp_total_to_level(level: i32, balance: &Balance) -> i64 {
    (1..level.max(1)).map(|l| xp_to_next(l, balance)).sum()
}

/// What an encounter of `encounter_level` is worth to a hero of `hero_level`. ONE
/// function for the whole level axis, because a reward that depends on the gap has
/// exactly one gap to read:
///
/// - hero **above** the encounter: full pay inside `xp_gap_grace` levels, then falling
///   linearly to `xp_gap_floor_mult` once it is `xp_gap_zero` levels above.
/// - hero **below** it: a bonus for punching up — `xp_up_per_level` a level to
///   `xp_up_knee`, steepening to `xp_up_per_level_steep` past it, capped at `xp_up_max`.
///   So +5 pays 1.05x, +10 pays 1.10x, +20 pays 1.25x.
///
/// **The bonus exists because the implicit one is inverted.** `base_xp` already rises
/// with depth, so a deeper encounter always paid more — but the *ratio* is what
/// "fighting up pays" means, and `(1 + d/500)^1.5` flattens: per creature, a +20-level
/// fight paid **1.82x at hero level 1 and 1.11x at 235**. The lure to punch up was
/// therefore strongest in the shallows, where the level gap is most likely to simply
/// kill you, and weakest in the deep game, where out-levelling the ground is the only
/// route left to a party that cannot get the gear. This term does not decay with level,
/// so that route stays open at 200 as much as at 20.
///
/// This is what makes "distance is the difficulty axis" true of REWARD and not only of
/// danger. Creature power rides distance, but the level curve is priced at the level's
/// own matched depth (`d = 12.5 × L`), so a party that levels without travelling gets
/// paid at hub rates against a hub-rate curve — measured, two heroes ground d=0 to level
/// 16 while taking 0-1 damage a fight, then died in one encounter the moment they walked
/// out. The falloff flattens that grind around level 6-8 and puts the next level where
/// the danger is.
pub fn xp_after_level_gap(
    xp: i64,
    encounter_level: i32,
    hero_level: i32,
    balance: &Balance,
) -> i64 {
    let r = &balance.runs;
    let gap = (hero_level - encounter_level.max(1)) as f64;
    let grace = r.xp_gap_grace.max(0) as f64;
    if gap < 0.0 {
        // Punching up. Two slopes: gentle to the knee, steeper past it, because a fight
        // twenty levels over your head is a different act from one five levels over it.
        let up = -gap;
        let knee = r.xp_up_knee.max(0) as f64;
        let mult = (1.0
            + r.xp_up_per_level.max(0.0) * up.min(knee)
            + r.xp_up_per_level_steep.max(0.0) * (up - knee).max(0.0))
        .min(r.xp_up_max.max(1.0));
        return ((xp as f64) * mult).round().max(xp as f64) as i64;
    }
    if gap <= grace {
        return xp;
    }
    let zero = (r.xp_gap_zero as f64).max(grace + 1.0);
    let t = ((gap - grace) / (zero - grace)).clamp(0.0, 1.0);
    let mult = 1.0 - t * (1.0 - r.xp_gap_floor_mult.clamp(0.0, 1.0));
    // A kill is still a kill: an award that was worth something stays worth something.
    ((xp as f64) * mult).round().clamp(0.0, xp as f64) as i64
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
    /// The party's shared BAG. Everything found this dive lands here, and nothing in
    /// it can be used in a fight — see [`PlayerRun::pouches`].
    pub backpack: Vec<ItemStack>,
    /// Per-hero POUCHES, aligned with the party's slots: the only items a hero can
    /// reach mid-battle. Filled by moving things out of the bag on the overworld, so
    /// deciding who carries the heals happens before the fight starts, not during it.
    pub pouches: Vec<Vec<ItemStack>>,
    /// Chits found this run (economy.md S1). Lives in the backpack conceptually;
    /// banked into the Vault on extraction, deleted with the run on death.
    pub chits: i64,
    /// Red-chest gear found this run. Unowned until extraction converts it to
    /// owned Vault gear (gear-item-models.md); discarded on death.
    pub looted_gear: Vec<LootGear>,
    pub max_distance_reached: i32,
    /// Encounters this run entered. Rides the Vanguard record beside the distance,
    /// because *how* you got deep is the interesting half: 500 fights and 0 fights are the
    /// same tile and completely different runs.
    pub fights: i32,
    /// Fights this run successfully fled. A flee is not a failure — it is the other way
    /// past a creature — so the board reports it rather than hiding it.
    pub flees: i32,
    pub result: Option<RunResult>,
    /// Which party (enter-maze group) this run belongs to. Battles merge across
    /// party ids (the Expandable Party raid mechanic).
    pub party_id: u32,
}

impl PlayerRun {
    pub fn is_terminal(&self) -> bool {
        self.result.is_some()
    }

    /// Burn the EPHEMERAL pieces `hero_slot` was wearing when it fell, returning their
    /// names so the loss can be reported. Insured and standard kit is untouched.
    ///
    /// Ephemeral gear is the widest build in the game (`count_ephemeral_bonus` extra
    /// affixes on top of its rarity) and this is what it is priced against: it does not
    /// merely fail to come home, it goes when **the hero wearing it** goes. So a run built
    /// around one is a run a single bad turn can unmake — which is the trade that makes the
    /// tier interesting rather than just strong.
    ///
    /// Run-side rather than in the DB on purpose: gear found this dive does not reach the
    /// `gear` table until extraction, so the piece a hero found an hour ago and is wearing
    /// right now lives ONLY here. The DB call of the same name is the backstop for a red
    /// row that somehow outlived a previous run.
    pub fn burn_equipped_ephemeral(&mut self, hero_slot: i32) -> Vec<String> {
        let mut burned = Vec::new();
        self.looted_gear.retain(|g| {
            let doomed =
                g.insurance == Insurance::Ephemeral && g.equipped_hero_slot == Some(hero_slot);
            if doomed {
                burned.push(g.name.clone());
            }
            !doomed
        });
        burned
    }

    /// Apply victory XP, leveling up as thresholds are crossed. Returns the
    /// number of levels gained.
    /// Bank `xp` on ONE hero (by party slot) and settle the levels it buys. Only
    /// living heroes are ever passed here — a hero that fell earns nothing from the
    /// fight it did not finish. Returns the levels that hero gained.
    ///
    /// Credit one hero its share of an encounter's XP.
    ///
    /// An encounter is a POOL, and it is divided among the heroes still STANDING when
    /// it ends — so the last survivor of a bad fight banks the whole thing. `shares` is
    /// that living count, and it is deliberately separate from `party_size`: the two
    /// used to be one argument (`party_size.max(slot + 1)`), which meant a lone survivor
    /// in slot 3 still divided by four and three-quarters of the pool evaporated.
    ///
    /// `party_size` only grows the per-hero vectors, seeded at the run's
    /// `base_run_level`, so a dive from a deeper hub starts every hero deeper — and so
    /// the party-slot milestones can still count heroes who have not been paid yet.
    pub fn award_hero_xp(
        &mut self,
        slot: usize,
        shares: usize,
        party_size: usize,
        xp: i64,
        balance: &Balance,
    ) -> i32 {
        let size = party_size.max(slot + 1);
        let shares = shares.max(1);
        let xp = if balance.runs.xp_split_across_party {
            // Never round a real reward down to nothing.
            ((xp as f64) / shares as f64).round().max(1.0) as i64
        } else {
            xp
        };
        if self.hero_levels.len() < size {
            let base = self.run_level;
            self.hero_levels.resize(size, base);
            self.hero_xp.resize(size, 0);
            self.sync_pouches();
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

    /// Slots in ONE hero's pouch. The pouch is the only bounded container: the limit
    /// is there to force a CHOICE about who carries what, not to punish hauling, which
    /// is why the Party Inventory beside it is unbounded.
    pub fn pouch_capacity(&self, balance: &Balance) -> usize {
        balance.runs.hero_pouch_slots.max(0) as usize
    }

    /// Put `item` in the Party Inventory. Always succeeds — the shared inventory has no
    /// slot limit, so finding something good is never punished by having to leave
    /// something else behind. Same-kind stacks merge.
    pub fn try_carry(&mut self, item: ItemStack, _balance: &Balance) -> bool {
        Self::insert_into(&mut self.backpack, item, usize::MAX)
    }

    /// Add to a container, merging same-kind stacks. Returns false only when a NEW
    /// kind would exceed `capacity` — a stack that merges never costs a slot, so
    /// topping up something already carried always fits.
    fn insert_into(into: &mut Vec<ItemStack>, item: ItemStack, capacity: usize) -> bool {
        if let Some(stack) = into
            .iter_mut()
            .find(|s| s.item_kind == item.item_kind && s.insurance == item.insurance)
        {
            stack.quantity += item.quantity;
            return true;
        }
        if into.len() >= capacity {
            return false;
        }
        into.push(item);
        true
    }

    /// Keep `pouches` aligned with the party's slots. Called wherever the party grows,
    /// so a hero can never be commanded without somewhere to carry its own kit.
    pub fn sync_pouches(&mut self) {
        self.pouches.resize_with(self.hero_levels.len(), Vec::new);
    }

    /// How many of `kind` hero `slot` is carrying — what the battle Item action is
    /// checked against.
    pub fn pouch_qty(&self, slot: usize, kind: &str) -> i32 {
        self.pouches
            .get(slot)
            .and_then(|p| p.iter().find(|i| i.item_kind == kind))
            .map_or(0, |i| i.quantity)
    }

    /// Spend one of `kind` from hero `slot`'s pouch. False when that hero is not
    /// carrying it — the pouch is the authority on what a hero may drink, so a
    /// caller must not fall back to the bag on failure.
    pub fn spend_from_pouch(&mut self, slot: usize, kind: &str) -> bool {
        let Some(pouch) = self.pouches.get_mut(slot) else {
            return false;
        };
        let Some(stack) = pouch.iter_mut().find(|i| i.item_kind == kind && i.quantity > 0) else {
            return false;
        };
        stack.quantity -= 1;
        pouch.retain(|i| i.quantity > 0);
        true
    }

    /// Move `quantity` of `kind` between the shared bag and hero `slot`'s pouch.
    ///
    /// Moves as much as it can and reports the amount actually moved, rather than
    /// refusing outright: asking for 5 when 3 are held (or when only 3 fit) should
    /// leave the player with 3 moved, not an error and nothing done. Zero means the
    /// move was impossible — nothing held, no room, or a hero slot that does not exist.
    pub fn move_item(
        &mut self,
        slot: usize,
        kind: &str,
        quantity: i32,
        to_pouch: bool,
        balance: &Balance,
    ) -> i32 {
        if quantity <= 0 || slot >= self.pouches.len() {
            return 0;
        }
        let (cap, insurance, held) = {
            let src = if to_pouch { &self.backpack } else { &self.pouches[slot] };
            let Some(stack) = src.iter().find(|i| i.item_kind == kind && i.quantity > 0) else {
                return 0;
            };
            // Only the pouch can refuse; putting something back is always allowed.
            let cap = if to_pouch { self.pouch_capacity(balance) } else { usize::MAX };
            (cap, stack.insurance, stack.quantity)
        };
        let dst_has_kind = {
            let dst = if to_pouch { &self.pouches[slot] } else { &self.backpack };
            dst.iter().any(|i| i.item_kind == kind && i.insurance == insurance)
        };
        if !dst_has_kind {
            let dst_len = if to_pouch { self.pouches[slot].len() } else { self.backpack.len() };
            if dst_len >= cap {
                return 0;
            }
        }
        let moved = quantity.min(held);
        {
            let src = if to_pouch { &mut self.backpack } else { &mut self.pouches[slot] };
            if let Some(stack) = src.iter_mut().find(|i| i.item_kind == kind) {
                stack.quantity -= moved;
            }
            src.retain(|i| i.quantity > 0);
        }
        // A real id, not an empty one: the flee toll rolls per stack keyed on
        // `item_id`, so stacks sharing a blank id would share one roll and drop or
        // survive together.
        let item = ItemStack {
            item_id: uuid::Uuid::now_v7().to_string(),
            item_kind: kind.to_string(),
            quantity: moved,
            insurance,
        };
        let dst = if to_pouch { &mut self.pouches[slot] } else { &mut self.backpack };
        Self::insert_into(dst, item, cap);
        moved
    }

    /// This hero's level inside the dive; the run's headline level until the hero has
    /// earned anything of its own.
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
    /// Wall-clock ms the instance opened — the only thing here that is not pure, and it is
    /// stamped at the boundary rather than read in the engine. The END FIGHT's clear time is
    /// measured from it, and a time is the whole point of starring a run.
    pub started_ms: u64,
    next_party_id: u32,
}

impl InstanceRun {
    pub fn new(
        instance_id: Id,
        departure_hub_distance: i32,
        balance: &Balance,
        started_ms: u64,
    ) -> Self {
        InstanceRun {
            instance_id,
            started_ms,
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
                pouches: Vec::new(),
                chits: 0,
                looted_gear: Vec::new(),
                max_distance_reached: 0,
                fights: 0,
                flees: 0,
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
        CharacterClass::Smithwright => "smithwright",
        CharacterClass::Keeper => "keeper",
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

/// The HP each hero in `comp` opens a dive on.
///
/// A dive starts at FULL health, and "full" is the max at the level being dived at — so
/// this is [`max_hp_at_level`] and must stay [`max_hp_at_level`]. Deriving the opening HP
/// any other way agrees with the ceiling only while every dive starts at level 1: a party
/// departing at level 100 otherwise opens at 52 of 1042 HP, and nothing says so.
pub fn starting_hp(comp: &[CharacterClass], level: i32, balance: &Balance) -> Vec<i32> {
    comp.iter().map(|c| max_hp_at_level(*c, level, balance)).collect()
}

/// One hero's summed combat bonuses from their own equipped gear — the same type
/// `meld-db` sums out of the gear rows, so the two cannot drift.
pub use meld_proto::equipment::GearBonus;

/// Fold a hero's raw per-item elemental entries into one profile (spec §5
/// stat aggregation): per damage type, `1 + Σ(mᵢ − 1)` — so two quarter-
/// resists (0.75) stack to a half-resist (0.5) instead of multiplying into a
/// weakness — clamped to the spec's 0.0–2.0 bounds [TUNABLE bounds live in
/// the spec; structural here].
/// The per-item modifier entries a hero's WORN WEIGHTS contribute, ready to fold in beside
/// the pieces' own rolled elemental wards.
///
/// This is where "what am I wearing" becomes "what hurts me". A step is
/// `[armor_resist] step`; the direction and count of steps per damage type is
/// [`meld_proto::equipment::weight_profile`]. Every piece contributes, so a full plate set
/// is a real slash resistance and a real blunt weakness, and a mixed loadout is a blunted
/// version of both — which is the point of letting a class wear more than one weight.
pub fn weight_modifiers(weights: &[String], balance: &Balance) -> Vec<(String, f64)> {
    let step = balance.armor_resist.step;
    let mut out = Vec::new();
    for w in weights {
        let Some(weight) = meld_proto::equipment::ArmorWeight::from_wire(w) else {
            continue;
        };
        for (ty, steps) in meld_proto::equipment::weight_profile(weight) {
            out.push((ty.to_wire().to_string(), 1.0 + (*steps as f64) * step));
        }
    }
    out
}

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

/// A magnitude that lands on a hero, as a share of that hero's OWN max HP — the rule
/// every grant in this game follows, because a hero runs 40 max HP at level 1 and ~535 at
/// 100, so a flat number is a third of a hero early and a rounding error late. Floors at 1:
/// a grant that rounds to nothing is a grant the player was told they had.
fn frac_of(max_hp: i32, fraction: f64) -> i32 {
    (((max_hp as f64) * fraction).round() as i32).max(1)
}

/// Build the ally `Fighter`s for a party (shared by battle start and raid merge).
/// `row_overrides` (aligned with `party`) lets the player's saved formation win over
/// the class-default front/back row: `Some(true)` = back, `Some(false)` = front,
/// `None`/absent = keep the class default.
/// Mind's Eye: how many Foci a Psyker of `level` may raise at the top of a fight without
/// spending the turn on them. A controller whose whole kit is "hold three things at once"
/// otherwise spends its first three turns doing nothing but setting up, which is the
/// least interesting stretch of every fight it is in.
pub fn minds_eye_casts(level: i32, balance: &Balance) -> u32 {
    let b = &balance.battle;
    if b.psyker_minds_eye_at <= 0 || level < b.psyker_minds_eye_at {
        return 0;
    }
    let step = b.psyker_minds_eye_per_level.max(1);
    let grown = 1 + (level - b.psyker_minds_eye_at) / step;
    grown.clamp(1, b.psyker_minds_eye_cap as i32) as u32
}

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
            // Wll answers a blade, Mnd answers a spell. Gear's `def` is deliberately NOT
            // added here: armour is steel, and what stops fire is the mind behind it plus
            // whatever ward the piece happens to carry (which rides `damage_modifiers`).
            let ward = (stats.base_ward + grow(mnd, stats.mnd, a.mnd_to_ward) + bonus.ward)
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
            f.ward = ward;
            // "of the Furnace" — extra damage dealt of one element, summed across pieces and
            // kept as a multiplier so the engine can apply it wherever that element lands.
            for (el, pct) in &bonus.element_power {
                if let Some(ty) = meld_proto::enums::DamageType::from_wire(el) {
                    *f.element_power.entry(ty).or_insert(1.0) += *pct as f64 / 100.0;
                }
            }
            // Elemental wards from gear (spec §5): folded + clamped 0.0–2.0.
            // Worn weights and rolled wards fold together through the one path, so a
            // plate cuirass with a fire ward on it is both facts at once.
            let mut entries = bonus.modifiers.clone();
            entries.extend(weight_modifiers(&bonus.armor_weights, balance));
            f.damage_modifiers = fold_damage_modifiers(&entries);
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
            // What the weapon IS, read off the equipped main hand: a bow reaches past a
            // front rank where a sword does not, and an arrow PIERCES where a sword cuts —
            // which is what makes armour weight a loadout decision instead of a table.
            // The class stays the fallback for a hand with no physical answer of its own
            // (a caster's Globe) and for a hero holding nothing.
            if let Some(fam) = bonus
                .main_hand
                .as_deref()
                .and_then(meld_proto::equipment::ItemFamily::from_wire)
            {
                f.reach |= fam.reaches_past_the_front();
                f.sweeps |= fam.sweeps_a_rank();
                if let Some(dt) = fam.damage_type() {
                    f.basic_attack_type = dt;
                }
            }
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
                    // Mind's Eye: the doc's precognition, which acts "the moment danger
                    // manifests". Seeded here rather than in the engine because it is a
                    // property of the HERO's level, and the engine never sees a level curve.
                    f.free_casts = minds_eye_casts(level, balance);
                    f.back_row = true;
                }
                // A Resonant regenerates a little HP each of its turns (innate) and
                // stands in the back row.
                CharacterClass::Resonant => {
                    // AD-1 "of the Wellspring": percentage POINTS of max HP on top of the
                    // innate fraction. The Resonant is the only class with innate regen, so
                    // deepening it is a twist nobody else could spend — and it stays a
                    // FRACTION of the hero's own pool, like every other magnitude that
                    // lands on a hero.
                    let frac = balance.battle.resonant_regen_fraction
                        + bonus.mender_regen_pct as f64 / 100.0;
                    f.regen = ((f.max_hp as f64) * frac).round().max(1.0) as i32;
                    f.back_row = true;
                }
                // Adrenaline belongs to the HUNTER, which is where every ability that
                // spends it lives (`meld_proto::skills`: power_strike, second_wind,
                // snare, frenzy). It was granted to the Explorer instead, which broke
                // both classes at once: the Explorer banked a resource its tempo kit
                // never spends, and the Hunter — whose six skills all cost 30-80 — had a
                // cap of 0, so `resolve_hunter` refused every one of them for the whole
                // life of the class.
                CharacterClass::Hunter => {
                    f.adrenaline_max = balance.battle.hunter_adrenaline_max;
                    // AD-1 "of Fury": walk in with Adrenaline already banked, so the
                    // first turn can be a skill instead of a wind-up attack.
                    f.adrenaline = bonus.adrenaline.min(f.adrenaline_max);
                }
                // AD-1 "of the Blazed Trail": the Explorer sets the PACE, so it walks in
                // with its gauge already part-filled — the same head start a Psyker's pin
                // buys the whole party, earned by the loadout instead.
                CharacterClass::Explorer => {
                    f.gauge = (f.gauge + bonus.start_gauge_pct as f64 / 100.0).clamp(0.0, 0.99);
                }
                // AD-1 "of the Vanishing": the Shifter is the only class whose base Dex
                // clears the dodge floor, so deepening the PERMANENT dodge is a twist
                // nobody else has a use for. Not the Evasion boon — that decays, and every
                // class can already roll it.
                CharacterClass::Shifter => {
                    f.dodge += bonus.dodge_pct as f64 / 100.0;
                }
                // AD-1 "of the Pyre": the order's standing bonus against the risen, deeper.
                CharacterClass::PhoenixGuard => {
                    f.undead_bane += bonus.undead_bane_pct as f64 / 100.0;
                }
                // AD-1 "of the Anvil": the Smithwright walks in already Tempered, its own
                // signature buff, as a share of base atk. Seeded here like the ward affixes
                // above rather than as a STACK of the ability — gear granting a standing
                // bonus is the piece's own, not one of the five the fight allows.
                CharacterClass::Smithwright => {
                    f.atk += ((f.base_atk as f64) * (bonus.tempered_pct as f64 / 100.0))
                        .round() as i32;
                }
                // AD-1 "of the Grafted Bloom": the Keeper's damage rides Mnd, so its
                // spell power is the thing to deepen — the one martial-looking class where
                // this is not a caster's affix on a swordsman.
                CharacterClass::Keeper => {
                    f.spell_power += ((f.spell_power as f64)
                        * (bonus.spell_power_pct as f64 / 100.0))
                        .round() as i32;
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
                // A FRACTION of this hero's own max HP. Flat points made a passive
                // synergy worth 7.5% of a level-1 hero and 0.6% of a level-100 one — and
                // out-healed the Resonant's innate regen party-wide, so the best healer in
                // the game was beaten by something nobody spent a turn on.
                E::PartyBarrier => {
                    f.barrier += frac_of(f.max_hp, adv.synergy_party_barrier_fraction)
                }
                E::PartyRegen => f.regen += frac_of(f.max_hp, adv.synergy_party_regen_fraction),
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

/// **At most one creature per encounter hunts the party's ROLES.**
///
/// Profiles are rolled per creature, independently, so nothing stopped a pack of five all
/// coming to the same conclusion — and `Role` means "kill the healer first". Five creatures
/// arriving at that answer separately is not five decisions, it is one decision applied five
/// times, and the healer cannot survive it.
///
/// Landed the same release as the back-row change (physical-only mitigation), and the two
/// compound: four of the nine innately-tactical kinds — Choirmother, Hollowbishop, Sepulcher,
/// Gloamhound — carry a non-physical basic attack, so the rank does not protect the healer
/// from them either. The first `Role` creature in the encounter keeps it; the rest fall back
/// to hunting the weakest, which is still pressure without being a coordinated execution.
/// `GangUp` is deliberately left alone: it already converges on ONE shared mark, and it
/// announces itself.
fn cap_role_hunters(enemies: &mut [Fighter]) {
    let mut seen = false;
    for f in enemies.iter_mut() {
        if f.target_profile != TargetProfile::Role {
            continue;
        }
        if seen {
            f.target_profile = TargetProfile::Weakest;
        }
        seen = true;
    }
}

/// A stable 64-bit hash of a combatant id — the seed for any per-creature roll that must
/// be reproducible without reaching for the battle RNG (which would make one creature's
/// roll shift every later one).
fn hash_id(id: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

// Nine, and each one is a distinct fact the assembly needs: who, against what, in which
// run, at what balance, from which seed, carrying which wounds, in which formation, and
// whether the party chose the moment. Bundling them into a struct would move the arity
// rather than remove it.
/// Build the enemy `Fighter`s for an encounter (`CR-11` made this two callers instead of
/// one: a battle being assembled, and reinforcements ANSWERING A CALL into one already
/// running).
///
/// It is extracted rather than copied on purpose. Every property a creature carries into a
/// fight is set exactly once here — its wound, its rank, its pack role, its kit, its ward,
/// its resistances, its targeting profile — and the last time this repo kept "one rule, two
/// call sites" it cost a release each time (the wall-collision line that went into one mover
/// and not the other; the damage pass that kept the O(n^2) scan the movement pass had
/// already had fixed above it). A reinforcement that arrived through a second copy of this
/// would be a creature missing whichever line the copy forgot.
///
/// `party_scale` MUST be the scale the battle was built with, not one recomputed from who is
/// standing in it now: `Battle::join` never rescales enemies, so a creature called into a
/// fight has to be sized against the party that started it.
///
/// `group_base` offsets the group ids so a reinforcement wave cannot collide with the groups
/// already in the fight. Latecomers therefore form their own group rather than merging into
/// the knot of their own species already on the field — a group is a property of the
/// encounter, and arriving separately is a real thing that happened.
pub fn enemy_fighters(
    enemies: &[EnemyMember],
    balance: &Balance,
    party_scale: f64,
    group_base: u32,
) -> Vec<Fighter> {
    // Stable group ids for this encounter, one per creature TYPE present. A boss fighting
    // under its own name is still its species for grouping — what a player sees is a knot
    // of the same thing, and that is what a group-target ability should hit.
    let mut groups: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for (m, _) in enemies.iter() {
        let next = groups.len() as u32;
        groups.entry(m.monster_kind.clone()).or_insert(next);
    }
    let group_of = |kind: &str| group_base + groups.get(kind).copied().unwrap_or(0);

    // One enemy Fighter per grouped creature, carrying its faction + flee flag so
    // the battle can pit factions against each other.
    let mut enemy_fighters: Vec<Fighter> = enemies
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
            // Prefixes stack outward from the creature: "Colossus Vicious Ironmaw" reads
            // scale, then twist, then who it is — and the scale goes first because it is the
            // one a player has to act on before the fight starts.
            let name = if m.affix.is_empty() {
                base_name.to_string()
            } else {
                format!("{} {}", m.affix, base_name)
            };
            let name = match meld_proto::warbands::title(m.expects_parties) {
                "" => name,
                scale => format!("{scale} {name}"),
            };
            let mut f = Fighter::new(
                cid.clone(),
                CombatantKind::Monster,
                None,
                Some(name),
                m.level,
                // Its FULL health, scaled. `Fighter::new` sets `hp = max_hp`, so passing
                // the creature's CURRENT hp here made a wounded creature enter the fight
                // at "full" with a smaller pool: the damage a skirmish had already done
                // was real (it died to less) but completely invisible — the HP bar read
                // 100%, and an execute that scales with missing HP found none missing.
                // A wound the player cannot see is not an opportunity.
                ((m.max_hp.max(1) as f64) * party_scale).round() as i32,
                // NOT scaled by party size. A party of four brings four times the
                // damage, so the creature needs more HEALTH to last — but it does not
                // swing four times harder at each individual hero, and scaling attack
                // made a bigger party strictly more lethal per head. (Measured: at
                // distance 0 a creature hit for 31 against a level-1 hero's 34 HP,
                // and a four-bot party wiped instead of winning.)
                m.atk,
                m.def,
                m.speed_stat,
            );
            f.faction = m.faction.clone();
            f.flees = m.flees;
            // And the wound it walked in with (`CR-2`). Creature HP persists in the
            // overworld — a skirmish it survived, a fight a party fled from — so a
            // creature you find already bleeding fights at the health it actually has.
            // Scaled by the same `party_scale` as its max, so the FRACTION is preserved:
            // a creature at half stays at half whether one hero or four are facing it.
            if m.hp < m.max_hp {
                f.hp = ((m.hp.max(1) as f64) * party_scale).round().clamp(1.0, f.max_hp as f64) as i32;
            }
            // The rank it stood in out in the world is the rank it fights in. The engine
            // already halves physical damage both ways for a back row (`back_row_damage_mult`
            // / `back_row_attack_mult`) and lets spells, elemental brands and psychic damage
            // through at full force — so a pack's rear is answered by a caster and shrugs
            // off a sword, without a line of new combat code.
            f.back_row = m.back_row;
            // GROUP: enemies of the same type and their minions. Derived here rather than
            // carried through the world, because a group is a property of the ENCOUNTER —
            // the same creature belongs to a different group depending on who it ended up
            // standing with, and there are sixteen places a spawn is created but exactly
            // one place a battle is assembled.
            f.group_id = Some(group_of(&m.monster_kind));
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
            // A RAID boss answers the crowd it is sized for with its WIDE half (FS-4). Only
            // the COUNT is carried: the engine biases how the pool is rolled, rather than
            // anything here rewriting the kit — the authored weights are read as *rarity*
            // elsewhere (the rebuke's signature), so a kit scaled in place makes a raid boss
            // answer an interruption more weakly than the ordinary version of itself.
            f.raid_parties = m.expects_parties;
            // A creature's elemental resistance rides its defence curve, since it has no Mnd
            // to grow one from — kept under 1.0 so casting stays the answer to armour.
            f.ward = ((m.def as f64) * balance.armor_resist.creature_ward_fraction).round() as i32;
            f.damage_modifiers = meld_world::abilities::creature_damage_modifiers(ability_key)
                .into_iter()
                .collect();
            f.basic_attack_type =
                meld_world::abilities::creature_basic_attack_type(ability_key);
            // CR-9: how it picks who to hit. Its kind's nature first, then its encounter
            // class (a champion is smarter than its escort), then a level roll — deeper
            // creatures are smarter on average. Seeded off the creature's own combatant id
            // so the same creature in the same fight always thinks the same way, and so
            // promoting one cannot shift any other roll in the battle.
            // THE END FIGHT's ward and its slow floor. Merged ON TOP of the kind's own
            // elemental profile, so a boss keeps its identity and gains the set piece's
            // resistance rather than having one replace the other.
            if !m.set_piece_ward.is_empty() {
                let mult = balance.encounters.end_fight_ward_mult;
                let fams: &[DamageType] = match m.set_piece_ward.as_str() {
                    "mind" => &[DamageType::Mind, DamageType::Ethereal],
                    "physical" => {
                        &[DamageType::Blunt, DamageType::Slash, DamageType::Pierce]
                    }
                    _ => &[
                        DamageType::Fire,
                        DamageType::Ice,
                        DamageType::Water,
                        DamageType::Lightning,
                        DamageType::Wind,
                        DamageType::Earth,
                    ],
                };
                for ty in fams {
                    f.damage_modifiers.insert(*ty, mult);
                }
                f.slow_floor = balance.encounters.end_fight_slow_floor;
            }
            f.target_profile = meld_world::abilities::creature_target_profile(
                ability_key,
                &m.encounter_class,
                m.level,
                hash_id(cid),
                &balance.ai,
            );
            f
        })
        .collect();
    cap_role_hunters(&mut enemy_fighters);
    enemy_fighters
}

// Nine, and each one is a distinct fact the assembly needs: who, against what, in which
// run, at what balance, from which seed, carrying which wounds, in which formation, and
// whether the party chose the moment. Bundling them into a struct would move the arity
// rather than remove it.
#[allow(clippy::too_many_arguments)]
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
    // A SURPRISE: the party walked into a creature a Psyker had pinned, so it chose the
    // moment. Every hero opens with a full gauge and therefore the first move — which is
    // the entire reason to spend a pin rather than simply avoid the creature.
    surprise: bool,
) -> Battle {
    let mut allies = party_fighters(party, runs, balance, row_overrides);
    // Creatures scale with how many heroes are facing them, on top of the distance
    // curve. Four heroes bring ~4x the damage, so a flat encounter would make a full
    // party's fights the SHORTEST in the game; the ramp is superlinear so the arc runs
    // the intended way — quick solo fights early, long ones once the party is full.
    let party_scale = encounter_party_scale(allies.len(), balance);
    for (f, hp) in allies.iter_mut().zip(hp_overrides.iter()) {
        if let Some(h) = hp {
            f.hp = (*h).clamp(0, f.max_hp);
        }
    }

    let enemy_fighters = enemy_fighters(enemies, balance, party_scale, 0);

    // The encounter class is the strongest present (gatekeeper > elite > standard).
    let encounter_class = enemies
        .iter()
        .map(|(m, _)| match m.encounter_class.as_str() {
            "gatekeeper" => EncounterClass::Gatekeeper,
            // The undead rite is champion-tier: reporting it as Standard would let
            // it be fled like trash and read as trash on the wire.
            // The end fight reports as Gatekeeper-class: raid-tier, and not fleeable like
            // trash. Three named bosses is not an Elite encounter.
            "world_end" => EncounterClass::Gatekeeper,
            "elite" | "undead_rite" => EncounterClass::Elite,
            _ => EncounterClass::Standard,
        })
        .max_by_key(|c| match c {
            EncounterClass::Gatekeeper => 2,
            EncounterClass::Elite => 1,
            EncounterClass::Standard => 0,
        })
        .unwrap_or(EncounterClass::Standard);

    let mut battle =
        Battle::new(battle_id, encounter_class, allies, enemy_fighters, balance, seed);
    if surprise {
        battle.open_with_full_party_gauges();
    }
    battle
}

#[cfg(test)]
mod tests {

    /// The headline level and the party screen have to agree, because they are the same
    /// number: `run_level` is `max(hero_levels)`. Victory used to award twice — once per
    /// hero through `award_hero_xp` (the split share) and once more into the run's OWN xp
    /// pool through `award_xp` (the FULL encounter). The run pool climbed its own ladder
    /// faster, so the banner said level 3 while the party card still read level 2.
    #[test]
    fn the_headline_level_is_the_best_heros_level() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::Explorer, "r".into())]);
        let r = &mut runs.runs[0];

        let enc = same_level_encounter_xp(1, &b);
        // Two same-level encounters is what level 1 costs, so this must be exactly one up.
        for _ in 0..2 {
            r.award_hero_xp(0, 1, 1, enc, &b);
        }
        assert_eq!(r.hero_level(0), 2, "two at-level fights is one level");
        assert_eq!(
            r.run_level,
            r.hero_level(0),
            "the run's headline level must be the hero's, not a second ladder"
        );

        // And the run pool must not be quietly climbing alongside it.
        let before = r.run_level;
        r.award_hero_xp(0, 1, 1, 1, &b);
        assert!(
            r.run_level == before || r.run_level == r.hero_level(0),
            "run_level {} drifted from hero level {}",
            r.run_level,
            r.hero_level(0)
        );
    }


    /// A class's abilities and the resource they spend have to be granted to the SAME
    /// class. Adrenaline was handed to the Explorer while every ability that spends it
    /// belongs to the Hunter, so the Explorer banked a resource it could not use and the
    /// Hunter — cap 0, costs 30-80 — had every one of its six skills refused. Both classes
    /// were shipped broken and each looked individually plausible; only the pairing is
    /// wrong, so the pairing is what gets asserted.
    #[test]
    fn a_class_that_pays_in_adrenaline_is_the_class_that_earns_it() {
        let b = Balance::load_default().unwrap();
        let spends_adrenaline = ["power_strike", "second_wind", "snare", "frenzy"];
        // The eight fieldable classes (mirrors `unlocks::owned_classes`).
        for class in [
            CharacterClass::Explorer,
            CharacterClass::Hunter,
            CharacterClass::Resonant,
            CharacterClass::Shifter,
            CharacterClass::PhoenixGuard,
            CharacterClass::Psyker,
            CharacterClass::Smithwright,
            CharacterClass::Keeper,
        ] {
            let key = class_key(class);
            let runs = {
                let mut r = InstanceRun::new("i".into(), 0, &b, 0);
                r.add_party(vec![("p".into(), "u".into(), class, "r".into())]);
                r
            };
            let party: Vec<PartyMember> = vec![("p".into(), "r".into(), class, Default::default())];
            let cap = party_fighters(&party, &runs, &b, &[])[0].adrenaline_max;
            let owns_paid_skill = meld_proto::skills::skills_for_class(key)
                .iter()
                .any(|s| spends_adrenaline.contains(&s.key));
            assert_eq!(
                owns_paid_skill,
                cap > 0,
                "{key}: owns adrenaline-paid skills = {owns_paid_skill}, but its \
                 adrenaline cap is {cap} - a class must earn what its abilities spend"
            );
        }
    }


    /// The design statement is in FIGHTS, so the check has to be too — and against
    /// what a creature at that depth ACTUALLY pays, not against the reference constant
    /// the ladder is built from. Checking the ladder against its own constant is how a
    /// flat `xp_reference_group = 2.0` sat next to a group ramp that keeps the first
    /// 150 tiles as duels, quietly making level 1 cost ~4 fights instead of 2.
    #[test]
    fn a_level_costs_the_fights_it_says_it_does_against_real_creatures() {
        let b = Balance::load_default().unwrap();
        let xp: Vec<i64> = b.creature.values().map(|c| c.xp_reward).collect();
        assert!(!xp.is_empty(), "no creatures to price the ladder against");
        let lo = *xp.iter().min().unwrap() as f64;
        let hi = *xp.iter().max().unwrap() as f64;
        for level in [1, 2, 3, 5] {
            let need = xp_to_next(level, &b) as f64;
            let want = fights_per_level(level, &b) as f64;
            let d = 12.5 * level as f64;
            let mult = (1.0 + d / b.world_scaling.stat_mult_base_divisor)
                .powf(b.world_scaling.xp_distance_exp);
            let group = b.encounters.expected_group_size(d);
            // Cheapest creature in the table → the MOST fights this level can take;
            // richest → the fewest. The promise has to hold across that whole spread.
            let most = need / (lo * mult * group);
            let fewest = need / (hi * mult * group);
            assert!(
                most <= want * 1.5 && fewest >= want * 0.5,
                "level {level} promises {want} fights but real creatures make it                  {fewest:.1}-{most:.1}"
            );
        }
    }

    /// The first 150 tiles are duels (`[[encounters.group_ramp]]` starts at 150), so
    /// the ladder must price level 1 as one creature, not a pack.
    #[test]
    fn the_opening_of_the_game_is_priced_as_duels() {
        let b = Balance::load_default().unwrap();
        assert_eq!(b.encounters.expected_group_size(12.5), 1.0, "level 1 is a duel");
        assert!(
            b.encounters.expected_group_size(250.0) > 1.0,
            "the ramp still grows further out"
        );
    }

    use super::*;

    /// Mind's Eye grows with the hero and stops at its ceiling. A controller opens the
    /// fight with its Foci already up; it does not open every fight with three turns of
    /// setup, and it does not eventually open with an unbounded number of free actions.
    #[test]
    fn minds_eye_grows_with_the_psyker_and_then_stops() {
        let b = Balance::load_default().unwrap();
        let at = b.battle.psyker_minds_eye_at;
        assert_eq!(minds_eye_casts(at - 1, &b), 0, "granted before its level");
        assert_eq!(minds_eye_casts(at, &b), 1, "the first one arrives on the rung");
        assert_eq!(
            minds_eye_casts(255, &b),
            b.battle.psyker_minds_eye_cap,
            "the cap is not a cap"
        );
        // Monotone: a level-up never takes one away.
        let mut prev = 0;
        for lv in 1..=255 {
            let n = minds_eye_casts(lv, &b);
            assert!(n >= prev, "level {lv} lost a cast ({prev} -> {n})");
            prev = n;
        }
    }

    /// The client has no `balance.toml`, so `meld_proto::hubs::start_level` carries a copy
    /// of `base_run_level`'s formula purely so a chooser can say "heroes start at 40". A
    /// copy is a thing that drifts, so it is checked against the real one at every hub —
    /// and against the level cap, which is what makes the deepest hub the end of the ladder.
    /// **No single damage source clears the end fight.** Four Psykers deleted it in 6 rounds
    /// against the intended 25, taking no hits — because Foci ignore defence outright and
    /// ride Mnd, which comes from levelling rather than loot, so neither the armour nor the
    /// gear gate was in their path at all. Each of the three now shrugs off a DIFFERENT
    /// family, which makes a mixed party the answer without nerfing the class.
    #[test]
    fn no_single_damage_family_clears_the_end_fight() {
        let b = Balance::load_default().unwrap();
        let mut arena = meld_world::Arena::generate(&b, 4, false);
        for _ in 0..80 {
            arena.ensure_frontier(&b, b.encounters.end_fight_min_distance + 900.0);
        }
        let enders: Vec<&meld_world::MonsterSpawn> = arena
            .monsters
            .iter()
            .filter(|m| m.encounter_class == "world_end")
            .collect();
        if enders.is_empty() {
            return; // this seed placed it past the streamed frontier; other seeds cover it
        }
        let wards: std::collections::HashSet<&str> =
            enders.iter().map(|m| m.set_piece_ward.as_str()).collect();
        assert_eq!(
            wards.len(),
            3,
            "the three bosses share wards ({wards:?}) — one damage type clears the fight"
        );
        for want in ["mind", "physical", "elemental"] {
            assert!(wards.contains(want), "nothing in the encounter wards {want}");
        }

        // …and the ward has to actually bite: a Psyker's Mind tick on the mind-warded boss
        // must land for meaningfully less than on the others.
        let mult = b.encounters.end_fight_ward_mult;
        assert!(
            (0.05..0.6).contains(&mult),
            "a ward multiplier of {mult} either does nothing or is an immunity"
        );
    }

    /// A set piece is not a big creature: control cannot remove it from the fight. One
    /// Gravity Vortex plus an Anchor left each boss acting **0.3 times in the whole fight**,
    /// so `end_fight_boss_atk` — the entire danger of the encounter — never happened.
    #[test]
    fn control_cannot_take_the_end_fight_out_of_the_fight() {
        let b = Balance::load_default().unwrap();
        let floor = b.encounters.end_fight_slow_floor;
        assert!(
            (0.4..0.9).contains(&floor),
            "a slow floor of {floor} either ignores control entirely or does not stop it"
        );
        // It must sit ABOVE the deepest slow in the game, or the clamp changes nothing.
        assert!(
            floor > b.battle.psyker_anchor_slow_mult,
            "the floor ({floor}) is under Anchor's multiplier ({}), so Anchor still wins",
            b.battle.psyker_anchor_slow_mult
        );
        assert!(
            floor > b.battle.status_slow_mult,
            "the floor ({floor}) is under an ordinary web/chill ({}), so nothing is clamped",
            b.battle.status_slow_mult
        );
    }

    /// THE END FIGHT has to be **winnable**, and this pins the division nobody did.
    ///
    /// The previous version of this test asserted "clears in 15–35 rounds" and "a geared
    /// hero survives 3–8 hits" side by side, and never compared them — so it passed while
    /// describing a party that dies five times over before the bosses are down. Playing it
    /// through `mcp/` measured exactly that: 14 hero-turns, 11.5% of the bosses' health,
    /// wipe. The model was right the whole time; the arithmetic stopped one line early.
    ///
    /// So the numbers here are **calibrated from played runs**, not re-derived: a level-100
    /// party in tier-32 gear puts out ~470 damage per hero-turn and takes ~189 per hero-turn
    /// at `end_fight_boss_atk = 420`. Incoming scales with the attack number; output does
    /// not. Both constants are floors — the reference policy heals reactively, does not keep
    /// Barrier up, and spreads damage across three bosses instead of focusing one — so a
    /// party that plays well beats these figures and a careless one does worse. And they are
    /// AVERAGES: at these values the reference policy wins or loses run to run at an
    /// identical seed, because ATB is real-time and action ordering is not reproducible — so
    /// a margin near 1.0 means "decided by play", not "wins".
    ///
    /// ⚠️ **This is deliberately NOT a gear check any more, because the game cannot express
    /// one here yet.** See the `EW-0` roadmap entry: a creature's ABILITY damage never
    /// subtracts hero defence (`apply_typed_damage` applies resistances and the minimum,
    /// then stops), and only its basic `Attack` goes through `atk - def`. These bosses act
    /// almost entirely through abilities, so armour is close to irrelevant to them —
    /// measured, an UNGEARED level-100 party lasted 26 hero-turns against a geared party's
    /// 25. No value of `end_fight_boss_atk` creates a gate through a stat the fight barely
    /// reads, so asserting one here would only pin a fiction.
    #[test]
    fn the_end_fight_is_hard_and_winnable() {
        let b = Balance::load_default().unwrap();
        let e = &b.encounters;

        // Measured through the real wire, not derived. See the note above. Both figures are
        // from runs where the party DRANK — the starting kit is ~1130 HP of healing on a
        // 2648 HP party, so a measurement without potions is a measurement of a party that
        // left 42% of its effective health in its pockets.
        const OUTPUT_PER_HERO_TURN: f64 = 357.0;
        const TURNS_SURVIVED_AT_REFERENCE_ATK: f64 = 18.5;
        const REFERENCE_ATK: f64 = 420.0;

        // Incoming damage scales with the attack number, so the turns a party lasts scales
        // inversely with it. This is what makes attack the FIGHT-LENGTH dial and HP the
        // win-condition dial: cutting HP alone leaves a party dying at the same turn count.
        let turns_survived =
            TURNS_SURVIVED_AT_REFERENCE_ATK * (REFERENCE_ATK / e.end_fight_boss_atk as f64);
        let total_boss_hp = e.end_fight_boss_hp as f64
            * e.end_fight_bosses as f64
            * encounter_party_scale(4, &b);
        let turns_to_kill = total_boss_hp / OUTPUT_PER_HERO_TURN;
        let margin = turns_to_kill / turns_survived;

        // THE division. Below 1.0 the reference party wins outright; above it, winning takes
        // playing better than the reference. Past 1.6 no amount of skill closes the gap and
        // the encounter is the impossible one again.
        assert!(
            margin <= 1.6,
            "the end fight needs {turns_to_kill:.0} hero-turns to kill and survives \
             {turns_survived:.0} — {margin:.1}x more fight than party, i.e. unwinnable"
        );
        // And it must not be a walkover: the apex should be in doubt.
        assert!(
            margin >= 0.7,
            "the end fight dies in {turns_to_kill:.0} hero-turns while the party survives \
             {turns_survived:.0} — {margin:.1}x, so the apex is a formality"
        );
        // Long enough to be a fight rather than a coin flip.
        assert!(
            turns_survived >= 16.0,
            "the party lasts {turns_survived:.0} hero-turns — four rounds is an ambush, \
             not the end of the game"
        );
        // And it must out-reward a Gatekeeper, or the apex is a worse source than a pass.
        assert!(e.end_fight_loot_mult > e.gatekeeper_loot_mult);
    }

    /// The client has no `balance.toml`, so `meld_proto::hubs::start_level` carries a copy
    /// of `base_run_level`'s formula. The two must agree at EVERY distance, not just at a
    /// few authored rows: now that the authored deep hubs are retired and a `BD-5` forward
    /// town supplies its own distance, a departure point can sit anywhere a player can
    /// build, so a sweep is the only honest form of this test.
    #[test]
    fn the_client_copy_of_the_ladder_agrees_with_the_real_curve() {
        let b = Balance::load_default().unwrap();
        for d in (0..=4000).step_by(25) {
            assert_eq!(
                meld_proto::hubs::start_level(d),
                base_run_level(d, &b),
                "d{d} advertises a different starting level than the run would give it"
            );
        }
        for h in meld_proto::hubs::HUBS {
            assert_eq!(meld_proto::hubs::start_level(h.distance), base_run_level(h.distance, &b));
        }
    }

    /// The HP a dive opens on IS the ceiling it fights under, at every level on the
    /// ladder — not only at 1, where a level-1 constant and a level-aware derivation
    /// happen to be the same number. Found by playing a level-100 party and reading its
    /// own party screen: 52 of 1042.
    #[test]
    fn a_hero_starts_a_dive_at_full_health_whatever_level_it_leaves_at() {
        let b = Balance::load_default().unwrap();
        let comp = [
            CharacterClass::PhoenixGuard,
            CharacterClass::Hunter,
            CharacterClass::Psyker,
            CharacterClass::Resonant,
        ];
        for level in [1, 10, 40, 100, b.runs.max_hero_level] {
            let opening = starting_hp(&comp, level, &b);
            for (i, class) in comp.iter().enumerate() {
                let ceiling = max_hp_at_level(*class, level, &b);
                assert_eq!(
                    opening[i], ceiling,
                    "a {class:?} departing at level {level} opens at {} of {ceiling}",
                    opening[i]
                );
            }
        }
        // And the ladder has to actually climb, or the assertion above is satisfied by a
        // constant and proves nothing.
        assert!(
            starting_hp(&comp, 100, &b)[0] > 4 * starting_hp(&comp, 1, &b)[0],
            "HP barely grows with level, so this test cannot catch the bug it is for"
        );
    }

    /// Armour answers for damage TYPES, through the same `modifier_for` a creature's hide
    /// uses. Plate turns an edge and fears a hammer; a robe is the reverse; and the whole
    /// point is that the two heroes take DIFFERENT damage from the same blow.
    #[test]
    fn what_you_wear_decides_what_hurts_you() {
        let b = Balance::load_default().unwrap();
        let plate: Vec<String> = std::iter::repeat_n("heavy".to_string(), 4).collect();
        let robes: Vec<String> = std::iter::repeat_n("robe".to_string(), 4).collect();

        let p = fold_damage_modifiers(&weight_modifiers(&plate, &b));
        let r = fold_damage_modifiers(&weight_modifiers(&robes, &b));
        let slash = meld_proto::enums::DamageType::Slash;
        let blunt = meld_proto::enums::DamageType::Blunt;

        assert!(p[&slash] < 1.0, "plate takes a full slash ({})", p[&slash]);
        assert!(p[&blunt] > 1.0, "plate shrugs off a hammer ({})", p[&blunt]);
        assert!(r[&slash] > 1.0, "a robe turns a blade ({})", r[&slash]);
        // Real armour is strictly better than cloth at being hit — that is what armour IS.
        assert!(p[&slash] < r[&slash] && p[&blunt] < r[&blunt]);
        // The interesting property is not which is better, it is that **the weapon you bring
        // matters against plate and does not against cloth**: plate has a wide spread between
        // its best and worst physical answer, a robe has none. That spread is the whole
        // reason a party cares whether the thing in front of it carries an axe or a maul.
        let spread = |m: &std::collections::HashMap<meld_proto::enums::DamageType, f64>| {
            m[&blunt] / m[&slash]
        };
        assert!(
            spread(&p) > 1.5,
            "a hammer is only {:.2}x a sword against plate — weight is cosmetic",
            spread(&p)
        );
        assert!(
            (spread(&r) - 1.0).abs() < 0.01,
            "a robe prefers one physical type over another ({:.2}x), which cloth should not",
            spread(&r)
        );

        // A full set is an IDENTITY, never a switch: the 0.0/2.0 clamp means an immunity or
        // a doubling, and a resistance that reaches either stops being a trade-off.
        for (label, m) in [("plate", &p), ("robe", &r)] {
            for (ty, v) in m.iter() {
                assert!(
                    *v > 0.15 && *v < 1.85,
                    "a full {label} set reaches {v} against {ty:?} — that is a switch, not a trade"
                );
            }
        }
    }

    /// One piece is a nudge and a full set is an identity — otherwise mixing weights, which
    /// several classes may do, would be pointless.
    #[test]
    fn armor_resistance_stacks_with_how_much_you_wear() {
        let b = Balance::load_default().unwrap();
        let slash = meld_proto::enums::DamageType::Slash;
        let one = fold_damage_modifiers(&weight_modifiers(&["heavy".into()], &b));
        let four = fold_damage_modifiers(&weight_modifiers(
            &std::iter::repeat_n("heavy".to_string(), 4).collect::<Vec<_>>(),
            &b,
        ));
        assert!(four[&slash] < one[&slash], "wearing more plate does not resist more");
        assert!(one[&slash] > 0.9, "a single piece should be a nudge, not a build");

        // A mixed loadout lands between the two pure ones, which is what makes the choice
        // interesting for a class that may wear either.
        let mixed = fold_damage_modifiers(&weight_modifiers(
            &["heavy".into(), "heavy".into(), "robe".into(), "robe".into()],
            &b,
        ));
        assert!(mixed[&slash] > four[&slash], "mixing in robes should soften the slash resist");
    }

    /// An unknown weight is skipped rather than panicking or defaulting to a stance: a row
    /// written by an older build must not silently hand out plate.
    #[test]
    fn an_unknown_armor_weight_grants_nothing() {
        let b = Balance::load_default().unwrap();
        assert!(weight_modifiers(&["mithril".into(), "".into()], &b).is_empty());
    }

    #[test]
    fn base_run_levels_match_canon() {
        let b = Balance::load_default().unwrap();
        assert_eq!(base_run_level(0, &b), 1);
        assert_eq!(base_run_level(500, &b), 40);
    }

    /// At most ONE creature per encounter hunts the party's roles. Five creatures reaching
    /// "kill the healer first" independently is not five decisions, it is one decision
    /// applied five times — and the healer cannot survive it.
    #[test]
    fn only_one_creature_per_encounter_hunts_the_healer() {
        let mut pack: Vec<Fighter> = (0..5)
            .map(|i| {
                let mut f = Fighter::new(
                    format!("m{i}"),
                    CombatantKind::Monster,
                    None,
                    Some("choirmother".into()),
                    1,
                    100,
                    10,
                    2,
                    50,
                );
                f.target_profile = TargetProfile::Role;
                f
            })
            .collect();
        cap_role_hunters(&mut pack);
        let hunters = pack.iter().filter(|f| f.target_profile == TargetProfile::Role).count();
        assert_eq!(hunters, 1, "a whole pack converged on the healer independently");
        // The rest still apply pressure — they fall back, they do not go passive.
        assert_eq!(
            pack.iter().filter(|f| f.target_profile == TargetProfile::Weakest).count(),
            4,
            "the demoted creatures should hunt the weakest, not stop fighting"
        );
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
            pouches: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
                fights: 0,
                flees: 0,
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
        // The design statement, asserted directly: a level costs `base` fights, plus
        // one more every `ramp` levels.
        assert_eq!(fights_per_level(1, &b), 2);
        assert_eq!(fights_per_level(5, &b), 2);
        assert_eq!(fights_per_level(6, &b), 3);
        assert_eq!(fights_per_level(11, &b), 4);

        // The gate that actually matters: level 10 opens your SECOND party slot, so
        // the whole first act is spent alone until you reach it. It has to land
        // inside a session — the old (L + 1) shape wanted 54 at-level fights for it.
        let to_ten: i32 = (1..10).map(|l| fights_per_level(l, &b)).sum();
        assert!(
            (15..=30).contains(&to_ten),
            "level 10 should cost 15-30 at-level fights, not {to_ten}"
        );
        // The ramp still has to bite later, or the ladder is flat.
        let to_thirty: i32 = (1..30).map(|l| fights_per_level(l, &b)).sum();
        assert!(to_thirty > to_ten * 4, "the ramp went missing: {to_ten} -> {to_thirty}");

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

    /// **The ladder plateaus past the knee — it does not flatten.** Two properties, and
    /// the pair is the design: the first hundred levels are UNTOUCHED (the knee only
    /// bends what comes after it), and past it a level still costs more than the last
    /// one did, just at a gentler rate. Past 100 a level buys stats alone — a martial
    /// ladder tops out there — so an ever-steeper price is a rising charge for a
    /// shrinking reward. But it must not go flat: out-levelling the ground is `AD-7`'s
    /// route for a party that cannot get the gear, and a free deep ladder makes grinding
    /// strictly better than the loot chase it is supposed to be an alternative to.
    #[test]
    fn the_ladder_plateaus_past_the_knee_without_flattening() {
        let b = Balance::load_default().unwrap();
        let knee = b.runs.fights_per_level_knee;

        // Below the knee, the early ramp is the whole story — one more fight every
        // `ramp` levels, exactly as it was before the plateau existed.
        for level in 1..=knee {
            let want = b.runs.fights_per_level_base + (level - 1) / b.runs.fights_per_level_ramp;
            assert_eq!(
                fights_per_level(level, &b),
                want,
                "the knee bent the ladder at level {level}, which is below it"
            );
        }

        // Never cheaper than the level before it, at any point on either slope.
        for level in 1..b.runs.max_hero_level {
            assert!(
                fights_per_level(level + 1, &b) >= fights_per_level(level, &b),
                "level {} costs fewer fights than level {level}",
                level + 1
            );
            assert!(
                xp_to_next(level + 1, &b) > xp_to_next(level, &b),
                "level {} costs less XP than level {level}",
                level + 1
            );
        }

        // Still rising past the knee — a flat deep ladder is the failure mode.
        let cap = b.runs.max_hero_level;
        assert!(
            fights_per_level(cap, &b) > fights_per_level(knee, &b),
            "the ladder went flat past the knee"
        );

        // …but rising more SLOWLY than it was. Compared as fights gained per level so
        // the assertion is on the SLOPE and not on where the two spans happen to sit.
        let early_slope = (fights_per_level(knee, &b) - fights_per_level(1, &b)) as f64
            / (knee - 1).max(1) as f64;
        let late_slope =
            (fights_per_level(cap, &b) - fights_per_level(knee, &b)) as f64 / (cap - knee).max(1) as f64;
        assert!(
            late_slope < early_slope,
            "the plateau is not a plateau: {early_slope:.3} fights/level before the knee, \
             {late_slope:.3} after"
        );
    }

    /// The design statement, as arithmetic: +5 levels up pays 1.05x, +10 pays 1.10x,
    /// +20 pays 1.25x. These three numbers ARE the spec — they were chosen first and the
    /// two slopes fitted to them, so a retune that breaks them is a retune of the design
    /// and has to say so here.
    #[test]
    fn punching_up_pays_the_stated_curve() {
        let b = Balance::load_default().unwrap();
        // 10_000 rather than a round 100 so the assertion is on the curve and not on
        // where the rounding happens to land.
        let full = 10_000;
        for (up, want) in [(5, 1.05), (10, 1.10), (20, 1.25)] {
            let paid = xp_after_level_gap(full, 40 + up, 40, &b);
            assert_eq!(
                paid,
                (full as f64 * want).round() as i64,
                "{up} levels up paid {}x, not {want}x",
                paid as f64 / full as f64
            );
        }
    }

    /// **The reason this term exists.** The bonus the depth curve pays implicitly is a
    /// RATIO of `(1 + d/500)^1.5`, which flattens as d grows: per creature a +20 fight
    /// was worth 1.82x at hero level 1 and 1.11x at 235. So the lure to punch up was
    /// strongest in the shallows, where the gap is most likely to just kill you, and
    /// weakest in the deep game, where out-levelling the ground is the last route open
    /// to a party that cannot get the gear. This half must not decay.
    #[test]
    fn the_reward_for_punching_up_does_not_decay_with_level() {
        let b = Balance::load_default().unwrap();
        let full = 10_000;
        for up in [5, 10, 20] {
            let shallow = xp_after_level_gap(full, 5 + up, 5, &b);
            for hero in [20, 60, 120, 200, 250] {
                assert_eq!(
                    xp_after_level_gap(full, hero + up, hero, &b),
                    shallow,
                    "{up} levels up pays differently at hero {hero} than at 5"
                );
            }
        }
    }

    /// Monotonic, never a pay CUT for a harder fight, and bounded — past the cap you are
    /// being carried rather than fighting, and the depth term is already paying multiples.
    #[test]
    fn punching_up_is_monotonic_and_capped() {
        let b = Balance::load_default().unwrap();
        let full = 10_000;
        let mut prev = full;
        for up in 0..250 {
            let paid = xp_after_level_gap(full, 1 + up, 1, &b);
            assert!(paid >= prev, "{up} levels up paid less than {}", up - 1);
            assert!(
                paid <= (full as f64 * b.runs.xp_up_max).round() as i64,
                "{up} levels up broke the cap"
            );
            prev = paid;
        }
        assert_eq!(prev, (full as f64 * b.runs.xp_up_max).round() as i64, "the cap is reachable");
    }

    /// The reported bug, as arithmetic: a party that levels without travelling was paid
    /// hub rates against a curve priced at its own level's depth, so it climbed to the
    /// teens on creatures that could not scratch it and then died the moment it walked
    /// out. Ground you have outgrown has to stop paying for it.
    #[test]
    fn ground_you_have_outgrown_stops_paying_for_itself() {
        let b = Balance::load_default().unwrap();
        let full = 1000i64;
        // At level the encounter pays in full, and every level BELOW it pays MORE — a
        // hero who is behind must never be taxed for being behind, and is now actively
        // rewarded for it (`punching_up_pays_the_stated_curve`).
        assert_eq!(xp_after_level_gap(full, 12, 12, &b), full);
        for hero in [1, 5, 11] {
            assert!(
                xp_after_level_gap(full, 12, hero, &b) > full,
                "hero {hero} was not paid for punching up"
            );
        }
        // …and so does a hero inside the grace band.
        let grace = b.runs.xp_gap_grace;
        assert_eq!(xp_after_level_gap(full, 12, 12 + grace, &b), full);
        // Past it, the payout falls off, monotonically, and bottoms out at the floor.
        let mut prev = full;
        for over in grace + 1..=b.runs.xp_gap_zero + 4 {
            let paid = xp_after_level_gap(full, 12, 12 + over, &b);
            assert!(paid <= prev, "the falloff went back up at +{over}");
            assert!(paid <= full, "a gap paid MORE than the encounter was worth");
            prev = paid;
        }
        let floor = (full as f64 * b.runs.xp_gap_floor_mult).round() as i64;
        assert_eq!(xp_after_level_gap(full, 12, 12 + b.runs.xp_gap_zero, &b), floor);
        assert_eq!(xp_after_level_gap(full, 12, 200, &b), floor, "the floor is a floor");
        assert!(floor > 0, "a kill has to stay worth something");
        // Zero in, zero out — the falloff never invents a reward.
        assert_eq!(xp_after_level_gap(0, 1, 99, &b), 0);

        // The shape that matters at the table: the hub ring (mlevel 1) is worth full
        // XP to a new hero and next to nothing to a level-12 one, while a hero AT the
        // depth its level is priced for (d = 12.5 x L) is never affected at all.
        assert_eq!(xp_after_level_gap(full, 1, 1, &b), full);
        assert!(
            xp_after_level_gap(full, 1, 12, &b) * 4 < full,
            "grinding the hub at level 12 still pays"
        );
        for level in [1, 5, 12, 20, 40] {
            assert_eq!(
                xp_after_level_gap(full, level, level, &b),
                full,
                "level {level} at its own depth lost XP"
            );
        }
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
            pouches: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
                fights: 0,
                flees: 0,
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
        let mut runs = InstanceRun::new("i".into(), 0, b, 0);
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
    fn the_hunter_starts_with_an_empty_adrenaline_pool() {
        // The martial baseline earns its resource in-battle: the pool exists (max
        // from balance) but starts empty, and it holds the front line.
        //
        // This test used to assert the pool belonged to the EXPLORER, which is how the
        // wrong pairing survived — a test can hold a bug in place as firmly as it holds a
        // feature. The invariant that matters is
        // `a_class_that_pays_in_adrenaline_is_the_class_that_earns_it`.
        let b = Balance::load_default().unwrap();
        let h = solo_fighter(CharacterClass::Hunter, 1, &b);
        assert_eq!(h.adrenaline_max, b.battle.hunter_adrenaline_max);
        assert_eq!(h.adrenaline, 0, "Adrenaline is banked in-fight, not granted");
        assert!(!h.back_row, "the Hunter holds the front line");
    }

    /// And the Explorer, whose kit costs nothing, must not be handed a resource it has no
    /// way to spend — an Adrenaline bar filling to full and never being usable reads as a
    /// broken class.
    #[test]
    fn the_explorer_carries_no_resource_it_cannot_spend() {
        let b = Balance::load_default().unwrap();
        let e = solo_fighter(CharacterClass::Explorer, 1, &b);
        assert_eq!(e.adrenaline_max, 0, "the Explorer's kit spends nothing");
        assert!(!e.back_row, "the Explorer holds the front line");
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
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
        let battle = build_battle("b".into(), &party, &enemies, &runs, &b, 1, &[Some(17)], &[], false);
        let (allies, _) = battle.wire_combatants();
        assert_eq!(allies.len(), 1);
        assert_eq!(allies[0].hp, 17, "wounded HP carried into the new battle");
        assert!(allies[0].max_hp > 17, "max HP stays at the class base");
    }

    /// CR-2: a creature's wound comes INTO the fight, and reads as a wound.
    ///
    /// The bug this pins: the enemy's max HP was built from its CURRENT hp, and
    /// `Fighter::new` sets `hp = max_hp` — so a creature at half entered the fight at
    /// "full" with half the pool. The damage was real (it died to less) and completely
    /// invisible: the HP bar read 100%, and an execute that scales with missing HP found
    /// nothing missing. A wound the player cannot see is not an opportunity.
    #[test]
    fn a_wounded_creature_walks_into_the_fight_wounded() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
        runs.add_party(vec![("p1".into(), "u1".into(), CharacterClass::Explorer, "r1".into())]);
        let party: Vec<PartyMember> =
            vec![("p1".into(), "c1".into(), CharacterClass::Explorer, GearBonus::default())];
        let mut arena = meld_world::Arena::generate(&b, 5, true);

        // Untouched: full, exactly as before this rule existed.
        let whole = {
            let enemies = vec![(&arena.monsters[0], "mc".to_string())];
            let battle = build_battle("b".into(), &party, &enemies, &runs, &b, 1, &[], &[], false);
            let (_, foes) = battle.wire_combatants();
            assert_eq!(foes[0].hp, foes[0].max_hp, "an untouched creature is not at full");
            foes[0].max_hp
        };

        // Halved out in the world: the pool is unchanged and the BAR shows the wound.
        arena.monsters[0].hp = arena.monsters[0].max_hp / 2;
        let enemies = vec![(&arena.monsters[0], "mc".to_string())];
        let battle = build_battle("b".into(), &party, &enemies, &runs, &b, 1, &[], &[], false);
        let (_, foes) = battle.wire_combatants();
        assert_eq!(foes[0].max_hp, whole, "the wound shrank the creature instead of hurting it");
        assert!(foes[0].hp < foes[0].max_hp, "the wound is invisible in the fight");
        // The FRACTION is what carried, not a raw number — that is what keeps it honest
        // once `encounter_party_scale` has multiplied the pool for a bigger party.
        let left = foes[0].hp as f64 / foes[0].max_hp as f64;
        assert!((left - 0.5).abs() < 0.02, "half a creature came in at {left:.3}");
        // Alive, whatever the rounding: a creature written in dead is a corpse nobody killed.
        assert!(foes[0].hp >= 1);
    }

    /// FS-4 unique boss mechanics: a Gatekeeper's `boss_kind` (assigned in
    /// world-gen) drives the assembled Fighter's abilities/damage-modifiers/
    /// basic-attack — its own named-boss kit, not its base biome creature's —
    /// and its display name reads as the boss's title.
    #[test]
    fn boss_kind_drives_the_assembled_fighters_kit_and_name() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
        let battle = build_battle("b".into(), &party, &enemies, &runs, &b, 1, &[], &[], false);
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
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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

    /// The whole chain, end to end: what a hero WEARS reaches the `Fighter` the engine asks
    /// about damage types. Folding in isolation is not proof — the entries have to survive
    /// battle assembly, which is where the harness's dressed sets and the Vault's real ones
    /// both arrive.
    #[test]
    fn worn_resistance_reaches_the_fighter_the_engine_asks() {
        use meld_proto::enums::DamageType;
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::PhoenixGuard, "r".into())]);
        let dressed: Vec<PartyMember> = vec![(
            "p".into(),
            "c".into(),
            CharacterClass::PhoenixGuard,
            GearBonus {
                armor_weights: std::iter::repeat_n("heavy".to_string(), 4).collect(),
                // Four quarter-resist fire wards, the way an epic set rolls them.
                modifiers: std::iter::repeat_n(("FIRE".to_string(), 0.75), 4).collect(),
                ..Default::default()
            },
        )];
        let f = party_fighters(&dressed, &runs, &b, &[]).pop().unwrap();

        // The weight's physical stance is there…
        let slash = f.damage_modifiers[&DamageType::Slash];
        let blunt = f.damage_modifiers[&DamageType::Blunt];
        assert!(slash < 1.0 && blunt > 1.0, "plate: slash {slash}, blunt {blunt}");
        // …and so is the rolled elemental ward, folded with it rather than replacing it.
        let fire = f.damage_modifiers[&DamageType::Fire];
        assert!(fire < 0.5, "four quarter-resist wards left fire at {fire}");
        assert!(fire >= 0.0, "a ward went NEGATIVE, which is absorption, not resistance");
    }

    /// A FALL BURNS THAT HERO'S EPHEMERAL KIT — and only that hero's, and only the
    /// ephemeral tier. This is what the tier's extra affixes are priced against: the widest
    /// build in the game is also the one a single bad turn can end. It has to be run-side,
    /// because gear found this dive does not reach the `gear` table until extraction.
    #[test]
    fn a_fallen_hero_burns_its_own_ephemeral_kit_and_nobody_elses() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::Hunter, "r".into())]);
        let piece = |name: &str, ins: Insurance, slot: Option<i32>| LootGear {
            gear_id: name.into(),
            name: name.into(),
            rarity: "epic".into(),
            slot: "chest".into(),
            class_key: "hunter".into(),
            insurance: ins,
            tier: 12,
            atk_bonus: 0,
            def_bonus: 9,
            spd_bonus: 0,
            base_max_durability: 70,
            max_durability: 70,
            equipped_hero_slot: slot,
            damage_modifiers: Vec::new(),
            family: String::new(),
            armor_weight: "medium".into(),
            affixes: Vec::new(),
            unique_key: String::new(),
            set_key: String::new(),
        };
        let run = &mut runs.runs[0];
        run.looted_gear = vec![
            piece("Doomed", Insurance::Ephemeral, Some(0)),
            piece("Someone Else's", Insurance::Ephemeral, Some(1)),
            piece("In the Bag", Insurance::Ephemeral, None),
            piece("Insured", Insurance::Insured, Some(0)),
            piece("Standard", Insurance::Standard, Some(0)),
        ];

        let burned = run.burn_equipped_ephemeral(0);

        assert_eq!(burned, vec!["Doomed".to_string()], "the wrong pieces burned");
        let left: Vec<&str> = run.looted_gear.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            left,
            vec!["Someone Else's", "In the Bag", "Insured", "Standard"],
            "a hero's death took another hero's kit, the backpack, or a tier that survives"
        );
        // Nothing to burn is not an error, and reports nothing rather than something.
        assert!(run.burn_equipped_ephemeral(0).is_empty());
    }

    #[test]
    fn ward_and_keyword_affixes_reach_the_fighter() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
    }

    /// A class-locked affix has to be spendable BY that class, and "of Fury" was not: it
    /// banks Adrenaline and was restricted to the **Explorer**, which has no Adrenaline at
    /// all — so the one class that could roll it was the one class it did nothing for, and
    /// `furys_yoke` (which pays 25 max HP for it) was a pure downgrade for its own wearer.
    /// Two copies of one rule: the engine moved Adrenaline to the Hunter, the affix table
    /// did not, and the proto-side test compared the two wrong values against each other.
    /// Asserted through the ENGINE rather than the table, so the check is "does the grant
    /// land" and not "do two constants match".
    #[test]
    fn a_keyword_affix_lands_on_the_class_it_is_locked_to() {
        let b = Balance::load_default().unwrap();
        let owner = meld_proto::affixes::find("adrenaline_primed")
            .and_then(|a| a.only_class)
            .expect("of Fury is class-locked");
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
        runs.add_party(vec![("p".into(), "u".into(), owner, "r".into())]);
        let primed: Vec<PartyMember> = vec![(
            "p".into(),
            "c".into(),
            owner,
            GearBonus { adrenaline: 4, ..Default::default() },
        )];
        let f = party_fighters(&primed, &runs, &b, &[]).pop().unwrap();
        assert!(
            f.adrenaline_max > 0,
            "of Fury is locked to a class that banks no Adrenaline - the affix is inert \
             wherever it can roll"
        );
        assert_eq!(f.adrenaline, 4.min(f.adrenaline_max));
    }

    #[test]
    fn a_synergy_affix_pays_out_only_when_its_ally_is_in_the_party() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
        // Each hero's OWN share: the grant is a fraction of the hero it lands on, so a
        // Shifter's opening Barrier is smaller than a Phoenix Guard's and should be.
        for f in &armed {
            let want = frac_of(f.max_hp, b.adventure.synergy_party_barrier_fraction);
            assert!(
                f.barrier >= want,
                "{} opened with {} of its own {} HP",
                f.combatant_id,
                f.barrier,
                f.max_hp
            );
        }

        // Blood and Balm (Resonant + Hunter) gives the party Regen — a kit that pays
        // in Adrenaline and blood beside one that gives it back. The Resonant's
        // innate Regen is on top, not replaced.
        let sustained = vec![member(CharacterClass::Hunter), member(CharacterClass::Resonant)];
        let f = party_fighters(&sustained, &runs, &b, &[]);
        let want_regen = frac_of(f[0].max_hp, b.adventure.synergy_party_regen_fraction);
        assert!(f[0].regen >= want_regen, "hunter regen {}", f[0].regen);
    }


    fn fresh_run() -> PlayerRun {
        PlayerRun {
            run_id: "r".into(),
            player_id: "p".into(),
            username: "u".into(),
            character_class: CharacterClass::Explorer,
            run_level: 1,
            xp: 0,
            backpack: vec![],
            pouches: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
                fights: 0,
                flees: 0,
            result: None,
            party_id: 0,
            hero_levels: Vec::new(),
            hero_xp: Vec::new(),
        }
    }

    /// **An encounter is a POOL divided among whoever is still STANDING.** Three heroes
    /// down means the survivor banks the whole thing — a fight that nearly killed you is
    /// worth what it cost. Dividing by the full party instead evaporated the fallen
    /// heroes' shares, and a survivor in a LATE slot was the worst case: the divisor and
    /// the vector sizing were one argument (`party_size.max(slot + 1)`), so the last hero
    /// standing in slot 3 still divided by four.
    /// **The encounter scales off the ROSTER, not the survivors** — and that pairing is
    /// what makes the XP rule above a risk rather than free money. A party of four that
    /// loses three still meets a four-hero encounter with one hero standing, so the extra
    /// XP is bought with a fight scoped for people who are no longer in it. Scale this to
    /// the living count instead and the risk half quietly disappears.
    #[test]
    fn a_fight_is_scoped_to_the_roster_even_when_only_one_hero_is_left() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
        runs.add_party(vec![("p1".into(), "u1".into(), CharacterClass::Explorer, "r1".into())]);
        let arena = meld_world::Arena::generate(&b, 5, true);

        let hp_of = |party: &[PartyMember], hp: &[Option<i32>]| -> i32 {
            let enemies = vec![(&arena.monsters[0], "mc".to_string())];
            let battle = build_battle("b".into(), party, &enemies, &runs, &b, 1, hp, &[], false);
            let (_, foes) = battle.wire_combatants();
            foes[0].max_hp
        };
        let member = |n: usize| -> PartyMember {
            ("p1".to_string(), format!("c{n}"), CharacterClass::Explorer, GearBonus::default())
        };

        let solo = hp_of(&[member(0)], &[None]);
        let four: Vec<PartyMember> = (0..4).map(member).collect();
        let full = hp_of(&four, &[None, None, None, None]);
        assert!(full > solo, "a four-hero encounter should be the bigger one");

        // Three of the four are down. The creature is exactly as big as it was.
        let three_down = hp_of(&four, &[None, Some(0), Some(0), Some(0)]);
        assert_eq!(three_down, full, "the encounter shrank when the party died");
        assert!(three_down > solo, "the survivor got a solo-sized fight for free");
    }

    #[test]
    fn the_last_hero_standing_banks_the_whole_pool() {
        let b = Balance::load_default().unwrap();
        // Divisible by four so the split is exact (the share is ROUNDED per hero, so an
        // odd pool would be a rounding argument rather than a sharing one), and small
        // enough that nobody levels — `hero_xp` is then the whole banked share rather
        // than the remainder left after level costs are subtracted.
        let pool = (xp_to_next(1, &b) / 8) * 4;

        let mut full = fresh_run();
        for slot in 0..4 {
            full.award_hero_xp(slot, 4, 4, pool, &b);
        }
        let quarter = full.hero_xp[0];
        assert!(quarter > 0);
        for slot in 0..4 {
            assert_eq!(full.hero_xp[slot], quarter, "an even split is not even");
        }

        let mut alone = fresh_run();
        alone.award_hero_xp(3, 1, 4, pool, &b);
        assert_eq!(alone.hero_xp[3], pool, "the survivor did not bank the whole pool");
        assert_eq!(alone.hero_xp[3], quarter * 4, "a quarter times four is the pool");
        // The fallen stay SEATED at the run's base level, so the party-slot milestones
        // can still count heroes who have not been paid yet.
        assert_eq!(alone.hero_levels.len(), 4, "the party lost its empty slots");
        assert_eq!(alone.hero_xp[0], 0, "a fallen hero earned from a fight it lost");

        let mut half = fresh_run();
        half.award_hero_xp(0, 2, 4, pool, &b);
        assert_eq!(half.hero_xp[0], quarter * 2, "two standing should take half each");
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
            pouches: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
                fights: 0,
                flees: 0,
            result: None,
            party_id: 0,
            hero_levels: Vec::new(),
            hero_xp: Vec::new(),
        };
        let one = same_level_encounter_xp(1, &b);

        // Hero 0 fights; heroes 1-3 do not (or fell). Only hero 0 climbs. The award is
        // SPLIT four ways, so this is twenty-four encounters' worth arriving as six.
        let gained = r.award_hero_xp(0, 4, 4, one * 6 * 4, &b);
        assert!(gained >= 2, "six level-1 fights bought {gained} levels");
        assert!(r.hero_level(0) > r.hero_level(1), "the idle hero climbed too");
        assert_eq!(r.hero_level(1), 1, "an unearned hero should sit at the base level");

        // The headline level follows the BEST hero, so one-number messages stay true.
        assert_eq!(r.run_level, r.hero_level(0));

        // Zero XP is a no-op — a dead hero is simply never passed here.
        let before = r.hero_level(0);
        assert_eq!(r.award_hero_xp(0, 4, 4, 0, &b), 0);
        assert_eq!(r.hero_level(0), before);

        // The slot-unlock rules count heroes at a level.
        assert_eq!(r.heroes_at_level(before), 1);
        assert_eq!(r.heroes_at_level(1), 4);
        let _ = r.award_hero_xp(1, 4, 4, one * 6 * 4, &b);
        assert_eq!(r.heroes_at_level(before), 2, "two heroes should now be there");

        // The cap holds per hero.
        let _ = r.award_hero_xp(2, 4, 4, i64::MAX / 4, &b);
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
                pouches: vec![],
                chits: 0,
                looted_gear: vec![],
                max_distance_reached: 0,
                fights: 0,
                flees: 0,
                result: None,
                party_id: 0,
                hero_levels: vec![1; size],
                hero_xp: vec![0; size],
            };
            r.award_hero_xp(0, size, size, 400, &b);
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
            let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
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
                false,
            );
            battle.combatant_hp("e0").unwrap_or(0)
        };
        assert!(
            hp_of(4) > hp_of(1),
            "the same creature was no tougher against four heroes"
        );
    }



    fn run_with_pouches(heroes: usize) -> PlayerRun {
        PlayerRun {
            run_id: "r".into(),
            player_id: "p".into(),
            username: "u".into(),
            character_class: CharacterClass::Hunter,
            run_level: 1,
            xp: 0,
            backpack: vec![],
            pouches: vec![Vec::new(); heroes],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
                fights: 0,
                flees: 0,
            result: None,
            party_id: 0,
            hero_levels: vec![1; heroes],
            hero_xp: vec![0; heroes],
        }
    }

    /// The Party Inventory has NO slot limit — finding something must never cost you
    /// something you already found. The bounded container is the pouch, and its limit
    /// exists to make "who carries the heals" a choice, not to tax hauling.
    #[test]
    fn the_party_inventory_never_refuses_and_stacks_what_it_has() {
        let b = Balance::load_default().unwrap();
        let mut r = run_with_pouches(1);
        let item = |kind: &str| ItemStack {
            item_id: format!("i-{kind}"),
            item_kind: kind.to_string(),
            quantity: 1,
            insurance: None,
        };
        for n in 0..500 {
            assert!(r.try_carry(item(&format!("kind{n}")), &b), "kind {n} was refused");
        }
        assert_eq!(r.backpack.len(), 500);
        // More of something already carried merges rather than taking a new row.
        assert!(r.try_carry(item("kind0"), &b));
        assert_eq!(r.backpack.len(), 500, "a merged stack took a row");
        assert_eq!(
            r.backpack.iter().find(|s| s.item_kind == "kind0").unwrap().quantity,
            2
        );
    }


    #[test]
    fn outgrowing_a_fight_lets_you_stomp_it() {
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
            let mut r = InstanceRun::new("i".into(), 0, &b, 0);
            r.add_party(
                (0..4)
                    .map(|i| (format!("p{i}"), "u".into(), CharacterClass::Hunter, "r".into()))
                    .collect(),
            );
            r
        };
        let rounds = |gear: i32, boss: bool, d: i64| -> f64 {
            let party = party_of(gear);
            let m = meld_world::MonsterSpawn::dungeon_boss(
                &b,
                "m".into(),
                "forest",
                if boss { "choirmother" } else { "" },
                d,
                7,
            );
            let battle = build_battle(
                "b".into(),
                &party,
                &[(&m, "e0".to_string())],
                &runs,
                &b,
                1,
                &[],
                &[],
                false,
            );
            let hp = battle.combatant_hp("e0").unwrap_or(0) as f64;
            let f = &party_fighters(&party, &runs, &b, &[])[0];
            let per = (f.atk as f64 * b.battle.skill_power_mult).round().max(1.0) * 4.0;
            (hp / per).ceil()
        };
        // Tier-appropriate gear for distance 1000 is about +210 attack; tier-40 gear
        // is +840. Carrying the deep loadout into a shallow boss should STOMP it —
        // that is the reward the whole gear chase pays out, and it is why boss health
        // is a fixed multiple of the encounter rather than derived from the party.
        let fair = rounds(210, true, 1000);
        let overgeared = rounds(840, true, 1000);
        assert!(
            overgeared * 2.0 < fair,
            "over-gearing barely helped: {overgeared} rounds vs {fair} at parity"
        );
        // And the under-geared party is punished at the same boss.
        let bare = rounds(0, true, 1000);
        assert!(bare > fair, "gear made no difference to a boss: {bare} vs {fair}");

        // A boss is still a real fight at parity, and an ordinary creature is not.
        assert!(fair >= 20.0, "a boss at parity folds in {fair} rounds");
        // An ordinary creature is not a boss. Taken from a real arena, because
        // `dungeon_boss` promotes whatever it builds to gatekeeper tier.
        let arena = meld_world::Arena::generate(&b, 7, false);
        let ordinary = arena
            .monsters
            .iter()
            .find(|m| m.encounter_class == "standard" && m.boss_kind.is_empty())
            .expect("a standard creature exists");
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
            false,
        );
        let hp = battle.combatant_hp("e0").unwrap_or(0) as f64;
        let f = &party_fighters(&party, &runs, &b, &[])[0];
        let per = (f.atk as f64 * b.battle.skill_power_mult).round().max(1.0) * 4.0;
        assert!(
            (hp / per).ceil() < fair / 4.0,
            "an ordinary creature fights like a boss"
        );
    }

    /// A transfer must CONSERVE items. Anything that leaves one container has to arrive
    /// in the other — a move that silently drops the difference is worse than a refusal,
    /// because the player only finds out when they reach for it in a fight.
    #[test]
    fn moving_items_conserves_them_in_both_directions() {
        let b = Balance::load_default().unwrap();
        let mut r = run_with_pouches(2);
        r.backpack.push(ItemStack {
            item_id: "i1".into(),
            item_kind: "bloom_salve".into(),
            quantity: 5,
            insurance: None,
        });
        let total = |r: &PlayerRun| -> i32 {
            r.backpack.iter().chain(r.pouches.iter().flatten()).map(|i| i.quantity).sum()
        };
        assert_eq!(r.move_item(1, "bloom_salve", 2, true, &b), 2);
        assert_eq!(r.pouch_qty(1, "bloom_salve"), 2);
        assert_eq!(total(&r), 5);
        assert_eq!(r.move_item(1, "bloom_salve", 1, false, &b), 1);
        assert_eq!(r.pouch_qty(1, "bloom_salve"), 1);
        assert_eq!(total(&r), 5);
        // Asking for more than is held moves what there is rather than failing whole.
        assert_eq!(r.move_item(1, "bloom_salve", 99, false, &b), 1);
        assert_eq!(r.pouch_qty(1, "bloom_salve"), 0);
        assert_eq!(total(&r), 5);
        assert_eq!(r.move_item(0, "nothing_like_this", 1, true, &b), 0);
        assert_eq!(r.move_item(9, "bloom_salve", 1, true, &b), 0, "no such hero");
        assert_eq!(total(&r), 5);
    }

    /// A hero reaches only its OWN pouch in a fight. If `spend_from_pouch` fell back to
    /// the Party Inventory the two containers would collapse back into one pile and the
    /// decision they exist to create would vanish.
    #[test]
    fn a_hero_can_only_spend_from_its_own_pouch() {
        let b = Balance::load_default().unwrap();
        let mut r = run_with_pouches(3);
        r.backpack.push(ItemStack {
            item_id: "i1".into(),
            item_kind: "elixir".into(),
            quantity: 4,
            insurance: None,
        });
        assert_eq!(r.move_item(2, "elixir", 1, true, &b), 1);
        assert!(r.spend_from_pouch(2, "elixir"), "hero 2 is carrying one");
        assert!(!r.spend_from_pouch(2, "elixir"), "and has now spent it");
        assert!(!r.spend_from_pouch(0, "elixir"), "hero 0 never carried one");
        assert!(!r.spend_from_pouch(9, "elixir"), "no such hero");
        // Three still sit in the Party Inventory, and being there did not help.
        assert_eq!(
            r.backpack.iter().find(|i| i.item_kind == "elixir").unwrap().quantity,
            3
        );
    }

    /// A pouch is bounded and the same size for every hero, so who carries what is a
    /// choice rather than a function of party composition.
    #[test]
    fn every_hero_gets_the_same_bounded_pouch() {
        let b = Balance::load_default().unwrap();
        let mut r = run_with_pouches(4);
        let pouch = r.pouch_capacity(&b);
        assert!(pouch > 0, "a pouch with no slots means no hero can carry anything");
        for kind in 0..pouch {
            r.backpack.push(ItemStack {
                item_id: format!("i{kind}"),
                item_kind: format!("kind{kind}"),
                quantity: 1,
                insurance: None,
            });
            assert_eq!(r.move_item(2, &format!("kind{kind}"), 1, true, &b), 1);
        }
        r.backpack.push(ItemStack {
            item_id: "over".into(),
            item_kind: "one_too_many".into(),
            quantity: 1,
            insurance: None,
        });
        assert_eq!(
            r.move_item(2, "one_too_many", 1, true, &b),
            0,
            "a full pouch should refuse a new kind"
        );
        // Refusing left the item where it was rather than destroying it.
        assert!(r.backpack.iter().any(|i| i.item_kind == "one_too_many"));
        // Every other hero still has an empty pouch of the same size.
        for slot in [0, 1, 3] {
            assert!(r.pouches[slot].is_empty());
        }
    }



    #[test]
    fn a_bigger_party_faces_a_tougher_creature_but_not_a_harder_hitting_one() {
        let b = Balance::load_default().unwrap();
        let build = |size: usize| {
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
            let mut runs = InstanceRun::new("i".into(), 0, &b, 0);
            runs.add_party(
                (0..size)
                    .map(|i| (format!("p{i}"), "u".into(), CharacterClass::Hunter, "r".into()))
                    .collect(),
            );
            let arena = meld_world::Arena::generate(&b, 7, false);
            let m = arena
                .monsters
                .iter()
                .find(|m| m.encounter_class == "standard")
                .expect("a standard creature")
                .clone();
            let atk = m.atk;
            let battle = build_battle(
                "b".into(),
                &party,
                &[(&m, "e0".to_string())],
                &runs,
                &b,
                1,
                &[],
                &[],
                false,
            );
            (battle.combatant_hp("e0").unwrap_or(0), atk)
        };
        let (hp1, base_atk) = build(1);
        let (hp4, _) = build(4);
        // Health scales: four heroes bring four times the damage, so the creature has
        // to LAST longer or a full party's fights are the shortest in the game.
        assert!(hp4 > hp1, "a creature facing four heroes was no tougher");

        // Attack deliberately does NOT scale. A pack does not swing four times harder
        // at each individual hero; scaling it made a larger party strictly more lethal
        // per head, and a level-1 four-bot party wiped at distance 0 instead of
        // winning. The guard for that is the `four_players_kill_monster` conformance
        // test — it drives real bots over the wire and is what caught it, which
        // arithmetic here did not.
        assert!(
            hp4 < hp1 * 5,
            "the party ramp is scaling something it should not: {hp1} -> {hp4}"
        );
        let _ = base_atk;
    }

    /// FS-4: a raid boss answers a CROWD with its wide half — played out, not asserted.
    ///
    /// The bug this closes is arithmetic. A raid boss's HP rides its declared party count and
    /// its attack deliberately does not, because a swing lands on ONE hero and scaling it
    /// would delete whoever arrives before the merge fills. But that reasoning is *about*
    /// single targets: a single-target blow is divided by however many heroes turned up (a
    /// quarter of a lone party's incoming damage, a SIXTEENTH of a full raid's) while an
    /// all-enemy one is divided by nothing. So the more help you brought, the less each hero
    /// felt — a Worldbreaker at sixteen heroes was a longer fight and an *easier* one.
    ///
    /// So the assertion is the RATIO, never the rates: the tunables are `[TUNABLE]` and the
    /// wide share drifts with every kit edit, but "a full raid must feel at least what one
    /// party feels" is the rule. `per_hero` is the whole model — a wide turn costs every hero
    /// its damage, a single one costs a hero its damage divided by the crowd.
    ///
    /// Played rather than modelled: the boss's picks come out of the real engine's weighted
    /// roll against real cooldowns, level gates and HP thresholds, over real world-generated
    /// gatekeepers. Modelling it is what let three bosses ship with their only party-wide
    /// ability gated at level 45 — twenty levels past where a gatekeeper first stands — so
    /// their raid tiers escalated nothing at all. Only playing it found that.
    #[test]
    fn a_raid_boss_answers_a_crowd_with_its_wide_half() {
        let b = Balance::load_default().unwrap();
        // A wide turn lands on every hero; a single one is split across whoever showed up.
        let per_hero = |wide_share: f64, heroes: f64| wide_share + (1.0 - wide_share) / heroes;

        // Aggregated across seeds: one gatekeeper is only a few dozen turns, too small a
        // sample to read a share off on its own.
        let wide_share_at = |parties: u8| {
            let (mut wide, mut total) = (0u32, 0u32);
            for wseed in [7u64, 11, 42] {
                let mut runs = InstanceRun::new("i".into(), 500, &b, 0);
                let classes = [
                    CharacterClass::PhoenixGuard,
                    CharacterClass::PhoenixGuard,
                    CharacterClass::Resonant,
                    CharacterClass::Resonant,
                ];
                runs.add_party(
                    classes
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            (format!("p{i}"), format!("u{i}"), *c, format!("r{i}"))
                        })
                        .collect(),
                );
                let mut arena = meld_world::Arena::generate(&b, wseed, false);
                arena.ensure_frontier(&b, 400.0); // cross a biome seam so a gatekeeper spawns
                let mut gk = arena
                    .monsters
                    .iter()
                    .find(|m| m.encounter_class == "gatekeeper")
                    .expect("a gatekeeper spawned at the crossed seam")
                    .clone();
                // Only the DECLARED count differs between arms — its stats are untouched, so
                // the two fights differ in the kit alone and nothing else can explain a gap.
                gk.expects_parties = parties;
                let enemies = vec![(&gk, "mc".to_string())];
                let party: Vec<PartyMember> = classes
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        (format!("p{i}"), format!("c{i}"), *c, GearBonus::default())
                    })
                    .collect();
                let mut battle =
                    build_battle("b".into(), &party, &enemies, &runs, &b, 1, &[], &[], false);
                let (allies, foes) = battle.wire_combatants();
                let hero_ids: std::collections::HashSet<String> =
                    allies.iter().map(|c| c.combatant_id.clone()).collect();
                let boss_id = foes[0].combatant_id.clone();
                for _ in 0..12_000 {
                    if battle.is_over() {
                        break;
                    }
                    let mut ready: Vec<String> = Vec::new();
                    for ev in battle.tick() {
                        match ev {
                            // The heroes have to actually SWING, or the boss never drops
                            // below its own HP thresholds and its enrage rows never unlock —
                            // a defending party silently under-samples the wide half.
                            meld_battle::Event::TurnReady { combatant_id } => {
                                ready.push(combatant_id)
                            }
                            meld_battle::Event::Resolved(r) if r.actor_id == boss_id => {
                                total += 1;
                                let hit: std::collections::HashSet<&String> = r
                                    .effects
                                    .iter()
                                    .filter(|e| hero_ids.contains(&e.target_id))
                                    .map(|e| &e.target_id)
                                    .collect();
                                if hit.len() >= 2 {
                                    wide += 1;
                                }
                            }
                            _ => {}
                        }
                    }
                    for id in ready {
                        let _ = battle.submit(
                            &id,
                            "a".into(),
                            meld_proto::enums::BattleActionKind::Attack,
                            Some(vec![boss_id.clone()]),
                            None,
                            None,
                        );
                    }
                }
                assert!(total > 20, "seed {wseed} gave only {total} boss turns to read");
            }
            f64::from(wide) / f64::from(total)
        };

        let shares: Vec<f64> = (1..=meld_proto::warbands::max_parties())
            .map(wide_share_at)
            .collect();
        // Every rung is a wider fight than the one below it — the ladder has to be a ladder,
        // or "Leviathan" and "Colossus" differ by a health bar and a word.
        for (i, pair) in shares.windows(2).enumerate() {
            assert!(
                pair[1] > pair[0],
                "{} parties goes wide {:.1}% of the time and {} parties only {:.1}%",
                i + 2,
                pair[1] * 100.0,
                i + 1,
                pair[0] * 100.0
            );
        }
        // THE CLAIM. A lone party against an unlabelled boss is the baseline every hero in
        // the game already accepts; a full raid on the boss sized for it must feel at LEAST
        // that. Before the wide half escalated, sixteen heroes felt about half of it.
        let solo_baseline = per_hero(shares[0], 4.0);
        let full_raid = per_hero(*shares.last().unwrap(), 16.0);
        assert!(
            full_raid > solo_baseline,
            "a full raid feels {full_raid:.3} per hero where one party feels \
             {solo_baseline:.3} - the raid boss is the easier fight"
        );
        // And under-manning it is worse still, which is what the plate's warning promises.
        assert!(
            per_hero(*shares.last().unwrap(), 4.0) > full_raid,
            "bringing nobody costs each hero no more than bringing everybody"
        );
    }

}
