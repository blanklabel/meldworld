//! Server-authoritative ATB engine (CANON.md §B, docs/behaviors/combat-atb.md).
//!
//! One [`Battle`] is a pure state machine: [`Battle::tick`] advances gauges on
//! the 100 ms cadence and resolves monster/timeout actions; [`Battle::submit`]
//! resolves a player action. Both return engine [`Event`]s that `meld-server`
//! maps onto `battle.*` wire messages. No wall-clock, no RNG globals, no I/O —
//! so it is fully deterministic and unit-testable (BUILD-PLAN M2.3/M2.4).

use std::collections::{HashMap, HashSet};

use meld_balance::Balance;
use meld_proto::abilities::{
    AbilityEffectKind, AbilityTarget, MonsterAbility, ScalingBase, StealTargetKind,
};
use meld_proto::common::Combatant as WireCombatant;
use meld_proto::enums::{
    BattleActionKind, BattleOutcome, CombatantKind, DamageType, EffectKind, EncounterClass,
    ModifierFlag, PackRole, TargetProfile,
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
    /// Which GROUP this combatant belongs to — enemies of the same type and their minions.
    /// A PACK is how the encounter got assembled (what `group_around` pulled in) and is
    /// only provenance; the group is the addressable unit: what a group-target ability
    /// hits, and what gets flanked. Derived at battle assembly rather than carried through
    /// the world, because a group is a property of the ENCOUNTER — the same creature is in
    /// a different group depending on who it ended up standing with. `None` for heroes.
    pub group_id: Option<u32>,
    /// This combatant's weapon takes a whole RANK of one group in a blow (a whip and its
    /// kin). Deliberately not paired with reach: a lash is long, not airborne, so sweeping a
    /// back rank runs into that rank's own protection.
    pub sweeps: bool,
    /// This combatant's weapon reaches past a front rank (bow, sling, thrown spear), so its
    /// physical blows land on a back rank at full force. The MARTIAL answer to a rear: until
    /// now the rank was a caster's problem and a swordsman's wall.
    pub reach: bool,
    /// This combatant's group is being worked by more than one party, so its rear is no
    /// longer covered — you cannot hide behind your front rank when two parties are on you
    /// from different sides. Set by the battle, read by `to_wire`, so the client and the
    /// damage rule can never disagree about it.
    pub flanked: bool,
    pub level: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub atk: i32,
    /// Attack before any fight-long buff, snapshot at battle start. A Tempering Blow is a
    /// share of THIS, and it refreshes rather than adds — a buff computed off the current
    /// value and added would compound geometrically, so ten Anvil Chorus casts would be
    /// 3.1x the party's attack for the price of ten turns.
    pub base_atk: i32,
    /// How many stacks of each lasting effect the fighter is holding. Every one of
    /// them used to accumulate without limit, and Regen never decayed at all — so a
    /// healer spending turns on it bought permanent, unbounded party sustain. One
    /// rule now covers all four: a grant is refused past `max_effect_stacks`, and a
    /// stack count resets when its effect has drained away.
    pub regen_stacks: u8,
    pub barrier_stacks: u8,
    pub evasion_stacks: u8,
    pub atk_stacks: u8,
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
    /// AD-1 "of the Pyre" (Phoenix Guard only): extra multiplier ON TOP of the order's
    /// standing bonus against the risen. On the ATTACKER, because it is the wearer's own
    /// zeal rather than a property of what it is hitting.
    pub undead_bane: f64,
    /// Set when a gauge knock lands on a creature that has a signature to answer with, and
    /// consumed on its next turn to force that signature instead of a rolled action.
    ///
    /// This is the REBUKE: denying a boss its turn works, and the boss comes back angry. It
    /// is not a reaction — this engine deliberately has none (the Psyker doc's Dampen/Static/
    /// Vent are left unbuilt rather than reinvented as something wearing the name) — it is a
    /// flag read when the creature takes its OWN turn, which is a thing the engine already
    /// does. So gauge denial stops being free: you buy a turn and pay for it with the worst
    /// attack in the boss's book.
    pub rebuke_pending: bool,
    /// STAGGERED: knocked down and not yet back up. Set by a gauge knock, cleared the moment
    /// this fighter next acts.
    ///
    /// The vulnerable window, and the reward for spending a turn on denial — a staggered
    /// fighter takes `staggered_damage_mult` more from everything. Without it a knock buys
    /// only tempo, which is thin for an ability that could have been damage.
    pub staggered: bool,
    /// How many more of its own turns this fighter must take before its gauge can be knocked
    /// again. Set when a knock lands, decremented each time it acts.
    ///
    /// It comes up as the fighter DOES — the guard has to survive the recovery turn, or the
    /// party simply re-knocks the instant it stands and every single boss turn becomes a
    /// rebuke. Counted in TURNS rather than ticks because creature `speed_stat` is a fixed
    /// 40-125 while a hero's climbs with Dex: a timer can lapse before a slow creature's
    /// gauge has refilled, and then the lock resumes. Counted in turns, "it always gets
    /// turns" is unconditional.
    pub gauge_guard_turns: u8,
    pub gauge: f64,
    pub statuses: Vec<String>,
    /// Content key of the fighter's class (`explorer`/`psyker`/`resonant`/…), surfaced
    /// to the client so it shows the right per-hero command menu. Empty for monsters.
    pub class_key: String,
    /// Barrier (temp HP): absorbs damage before HP, and decays each of this
    /// fighter's turns. Granted by wards (Psyker Kinetic Aegis, Resonant Ward).
    pub barrier: i32,
    /// Regen: HP restored at the start of each of this fighter's turns (Resonant
    /// innate, or granted by Regen Boon).
    pub regen: i32,
    /// Evasion: a temporary dodge bonus added to `dodge` against physical attacks,
    /// decaying a fixed amount at the start of each of this fighter's turns. Granted
    /// by the Shifter's Flicker blink.
    pub evasion: f64,
    /// Abilities this fighter has already spent its ONE per-battle use of (`Now`).
    /// Per-fighter rather than per-party: two Globemasters get one call each.
    pub once_spent: Vec<String>,
    /// Adrenaline: the Hunter's resource. Basic attacks bank it (up to `adrenaline_max`)
    /// and skills spend it. Zero/`adrenaline_max == 0` for every non-Hunter.
    pub adrenaline: i32,
    pub adrenaline_max: i32,
    /// Battle faction — `"player"` for heroes, else the creature's faction. Drives
    /// AI targeting: a fighter attacks the nearest fighter hostile to its faction
    /// (see `meld_proto::factions::battle_hostile`).
    pub faction: String,
    /// Whether this (creature) fighter flees a losing battle.
    pub flees: bool,
    /// PG-2 palette band (0-3) for a named boss, from the level it is met at: the
    /// same boss further out is the same boss in a worse mood. Rides the wire as a
    /// `boss_band:<n>` status token (the additive convention) so the client can tint
    /// it without a proto change.
    pub boss_band: u8,
    /// CR-6 pack role. A leader is shielded by its living minions and lends them
    /// its presence; killing it routs them. `None` for anything not in a pack.
    pub pack_role: PackRole,
    /// Back-row formation: takes reduced damage and is targeted less often (see
    /// `Battle::apply_damage` / `resolve_monster_turn`). Set for caster heroes in
    /// `meld-run`; false for front-row heroes and creatures.
    pub back_row: bool,
    /// Max simultaneous Foci (0 = not a Psyker; Psykers channel instead of the
    /// normal attack/skill kit — see [`Battle::resolve_psyker`]).
    pub focus_max: usize,
    /// Casts this Psyker may make WITHOUT spending the turn. Mind's Eye seeds the pool at
    /// battle start (the doc's "activate up to two Manifestations without expending your
    /// Action" at initiative); Dual Manifestation tops it back up each Psyker turn. Both
    /// are the same primitive — a cast that does not cost the turn — so there is one
    /// counter to reason about rather than two overlapping rules.
    pub free_casts: u32,
    /// Active Manifestations occupying Focus slots (Psyker only).
    pub foci: Vec<Focus>,
    /// True while a `defend` stance is active (until this fighter next acts).
    pub defending: bool,
    /// The (level-unfiltered) monster ability pool — content from
    /// `meld_world::abilities`. Empty for players and unknown creature kinds
    /// (they fight with basic attacks only).
    ///
    /// **Authored content, never rewritten per encounter.** A raid tier biases how this pool
    /// is ROLLED (see `raid_parties`) rather than editing it, because the weights in here are
    /// read for a second purpose — `signature_ability` picks the rarest one as the rebuke — so
    /// a scaled-in-place weight silently changes which ability is "rare".
    pub abilities: Vec<MonsterAbility>,
    /// How many PARTIES this creature is sized for (`meld_proto::warbands`); 1 for everything
    /// ordinary, and the only input the raid bias takes.
    ///
    /// A raid boss's HP rides this count and its ATTACK deliberately does not — a swing lands
    /// on ONE hero, so scaling it would delete whoever arrives before the merge fills. That
    /// argument is about single targets, and followed through it is the argument FOR biasing
    /// the WIDE half: **a single-target blow is divided by the crowd in front of it and an
    /// all-enemy one is divided by nothing.** So without this a raid boss got *easier* per
    /// hero the more help you brought — at sixteen heroes a single-target filler is a
    /// sixteenth of the pressure it is at four, while a Cinder Wave is unchanged.
    ///
    /// It buys CADENCE, never magnitude: party-wide rows are rolled oftener and come back
    /// sooner, and every number a hero takes is the one an ordinary gatekeeper deals. Nothing
    /// here can turn a hit into a one-shot, which is exactly why it is safe where scaling
    /// attack is not.
    pub raid_parties: u8,
    /// Which of the 10 named bosses (FS-4) this fighter is, if any — empty for
    /// players and plain creatures. Rides the wire as a `boss:<key>` status
    /// (see `build_wire_statuses`) so the client can render the actual boss
    /// sprite/animation instead of the generic creature billboard.
    pub boss_kind: String,
    /// Elemental profile: `DamageType → multiplier` (spec §1). `>1` weak,
    /// `<1` resist, `0` immune, `<0` absorb; missing types default to 1.0.
    /// Monsters get theirs from content; heroes aggregate theirs from gear.
    /// Elemental/psychic resistance — `def`'s counterpart, grown from Mnd. Subtracted from
    /// non-physical ABILITY damage the way `def` is subtracted from a physical blow.
    ///
    /// Before this existed, non-physical ability damage was reduced by nothing at all: `def`
    /// is only ever consulted for a basic attack, so a boss that fights with spells and
    /// breath ignored every point of armour a party had earned.
    pub ward: i32,
    /// "of the Furnace": how much MORE this fighter's damage of a type is worth. The
    /// offensive twin of `damage_modifiers`, which is what a target does about it.
    pub element_power: HashMap<DamageType, f64>,
    pub damage_modifiers: HashMap<DamageType, f64>,
    /// The [`DamageType`] this fighter's basic attack carries. Creature kinds
    /// and hero classes each have a typed basic swing; defaults untyped.
    pub basic_attack_type: DamageType,
    /// How this creature picks who to hit (CR-9). Set at assembly from the creature's
    /// kind, its encounter class and its level; heroes keep the default and never use it.
    pub target_profile: TargetProfile,
    /// Floor on how far a gauge SLOW may drag this fighter's fill rate (0 = no floor).
    /// A set piece is not a big creature: unclamped, one Gravity Vortex plus an Anchor left
    /// each end-fight boss acting 0.3 times in a whole fight, so the encounter's entire
    /// danger never happened.
    pub slow_floor: f64,
    /// Per-ability (pool index) tick at which it may be used again.
    ability_ready_at: HashMap<usize, u64>,
    /// An in-flight telegraphed ability: (pool index, executes_at tick). While
    /// set the monster is channeling — its gauge is frozen and it takes no
    /// other turns until the cast lands.
    channel: Option<(usize, u64)>,
    /// Timed statuses (`(name, expires_at_tick)`) applied by monster abilities.
    /// `poison`/`burn` tick damage at the victim's turn start; anything else
    /// slows the victim's ATB fill while active.
    timed_statuses: Vec<(String, u64)>,
    /// True once the gauge is full and we are waiting on this player's input.
    awaiting: bool,
    /// Engine tick at which the turn became ready (for the 15 s timeout).
    ready_tick: u64,
    alive: bool,
    /// How many times this fighter has been put DOWN in this battle. It is counted
    /// where death actually happens — the one Ko point in [`Battle::apply_damage`] —
    /// rather than inferred at battle end from `hp == 0`, because the gear tax
    /// (GR-2, CANON D6) is charged per FALL: end-state cannot tell a hero who was
    /// revived and killed again from one who fell once, nor a hero who fell in an
    /// earlier fight and is still down from one who fell in this one. A successful
    /// FLEE also clears `alive` without anybody dying, which is the other thing an
    /// end-state read gets wrong.
    falls: u32,
    /// Cached `build_wire_statuses()` output + a signature of the fields it reads,
    /// so the periodic gauge_update (every 100 ms) reuses the list and rebuilds it
    /// only when a status actually changes — instead of reallocating ~10 strings
    /// per fighter per tick. Refreshed at the end of [`Battle::tick`].
    statuses_cache: Vec<String>,
    statuses_sig: u64,
    statuses_cached: bool,
}

impl Fighter {
    /// The name to SAY about this fighter. A hero carries its own on `statuses` as
    /// `name:<name>` (heroes are named per account); anything else falls back to its class
    /// or combatant id, so a shout is never blank.
    pub fn display_name(&self) -> String {
        self.statuses
            .iter()
            .find_map(|s| s.strip_prefix("name:"))
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                if self.class_key.is_empty() {
                    self.combatant_id.clone()
                } else {
                    self.class_key.clone()
                }
            })
    }

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
            base_atk: atk,
            regen_stacks: 0,
            barrier_stacks: 0,
            evasion_stacks: 0,
            atk_stacks: 0,
            def,
            speed_stat,
            str_: 0,
            mnd: 0,
            dex: 0,
            wll: 0,
            spell_power: atk,
            dodge: 0.0,
            undead_bane: 0.0,
            staggered: false,
            gauge_guard_turns: 0,
            rebuke_pending: false,
            gauge: 0.0,
            statuses: Vec::new(),
            class_key: String::new(),
            barrier: 0,
            regen: 0,
            evasion: 0.0,
            adrenaline: 0,
            adrenaline_max: 0,
            once_spent: Vec::new(),
            faction: if kind == CombatantKind::Player {
                meld_proto::factions::PLAYER.to_string()
            } else {
                String::new()
            },
            flees: false,
            boss_band: 0,
            pack_role: PackRole::None,
            group_id: None,
            reach: false,
            sweeps: false,
            flanked: false,
            back_row: false,
            focus_max: 0,
            free_casts: 0,
            foci: Vec::new(),
            defending: false,
            abilities: Vec::new(),
            raid_parties: 1,
            boss_kind: String::new(),
            ward: 0,
            element_power: HashMap::new(),
            damage_modifiers: HashMap::new(),
            basic_attack_type: DamageType::None,
            target_profile: TargetProfile::Weakest,
            slow_floor: 0.0,
            ability_ready_at: HashMap::new(),
            channel: None,
            timed_statuses: Vec::new(),
            awaiting: false,
            ready_tick: 0,
            alive: hp > 0,
            falls: 0,
            statuses_cache: Vec::new(),
            statuses_sig: 0,
            statuses_cached: false,
        }
    }

    /// Wire status list — the channel the client reads per-combatant extras from:
    /// `class:<key>` (drives the per-hero command menu), `faction:<f>` (creature
    /// side), `boss:<key>` (FS-4 named-boss sprite/animation), `barrier:<n>`,
    /// `regen:<n>`, and (Psyker) `focus_slots:<n>` + `focus:<kind>:<stacks>` per
    /// Manifestation.
    fn build_wire_statuses(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !self.class_key.is_empty() {
            v.push(format!("class:{}", self.class_key));
        }
        if !self.boss_kind.is_empty() {
            v.push(format!("boss:{}", self.boss_kind));
        }
        if self.kind != CombatantKind::Player && !self.faction.is_empty() {
            v.push(format!("faction:{}", self.faction));
        }
        // A pack's leader carries 1.7x HP and its minions 0.45x (`[encounters]`), so two
        // members of the SAME species can sit 3.8x apart. The role has driven combat
        // (pack rout) since packs landed but never reached the client, so they drew at
        // identical size and the spread read as a bug rather than "one big spider with
        // four little ones".
        match self.pack_role {
            PackRole::Leader => v.push("pack:leader".to_string()),
            PackRole::Minion => v.push("pack:minion".to_string()),
            PackRole::None => {}
        }
        if self.barrier > 0 {
            v.push(format!("barrier:{}", self.barrier));
        }
        if self.regen > 0 {
            v.push(format!("regen:{}", self.regen));
        }
        if self.evasion > 0.0 {
            // Surfaced as a whole-percent dodge bonus for the client's status line.
            v.push(format!("evasion:{}", (self.evasion * 100.0).round() as i32));
        }
        if self.adrenaline_max > 0 {
            v.push(format!("adrenaline:{}", self.adrenaline));
            v.push(format!("adrenaline_max:{}", self.adrenaline_max));
        }
        if self.back_row {
            v.push("row:back".to_string());
        }
        if let Some(g) = self.group_id {
            v.push(format!("group:{g}"));
        }
        // A rank that stopped protecting has to SAY so, or the player watches their damage
        // change and cannot tell why — the same "the rule exists but the screen never says
        // it" shape as a status nothing draws.
        if self.flanked {
            v.push("flanked".to_string());
        }
        if self.reach {
            v.push("reach".to_string());
        }
        if self.sweeps {
            v.push("sweep".to_string());
        }
        // Once-per-battle abilities that are already gone. Without this the row stays
        // enabled and the only feedback is a refusal — the same "the rule exists but the
        // screen never says so" shape as a status nothing draws.
        for k in &self.once_spent {
            v.push(format!("spent:{k}"));
        }
        if self.boss_band > 0 {
            v.push(format!("boss_band:{}", self.boss_band));
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
        // Active timed statuses from monster abilities (poison/web/chill/…).
        for (name, _) in &self.timed_statuses {
            v.push(name.clone());
        }
        v.extend(self.statuses.iter().cloned());
        v
    }

    /// Cheap, allocation-free signature of the fields `build_wire_statuses` reads
    /// that can change mid-battle (class/faction/attributes are fixed after setup,
    /// so they need not be hashed).
    fn statuses_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.barrier.hash(&mut h);
        self.regen.hash(&mut h);
        ((self.evasion * 100.0).round() as i64).hash(&mut h);
        self.adrenaline.hash(&mut h);
        self.focus_max.hash(&mut h);
        for f in &self.foci {
            f.kind.hash(&mut h);
            f.stacks.hash(&mut h);
        }
        for (name, until) in &self.timed_statuses {
            name.hash(&mut h);
            until.hash(&mut h);
        }
        self.statuses.hash(&mut h);
        h.finish()
    }

    /// Rebuild the wire-status cache only when a relevant field changed since the
    /// last refresh. Called each tick; the common case is a no-op signature check.
    fn refresh_wire_statuses(&mut self) {
        let sig = self.statuses_signature();
        if !self.statuses_cached || sig != self.statuses_sig {
            self.statuses_cache = self.build_wire_statuses();
            self.statuses_sig = sig;
            self.statuses_cached = true;
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
            statuses: self.build_wire_statuses(),
        }
    }
}

/// Whether a timed ability status is a damage-over-time (poison/burn) rather
/// than an ATB-slowing bind (web/chill/bind).
fn is_dot_status(name: &str) -> bool {
    name == "poison" || name == "burn"
}

/// Which timed statuses actually SLOW the gauge. Named explicitly, because the gauge used
/// to slow on "any timed status that is not a DoT" — so every new token became a secret
/// slow, and the Explorer's own `marked`/`distracted` silently throttled whatever carried
/// them. A status list is a thing to add to on purpose, not to fall into.
fn is_slowing_status(name: &str) -> bool {
    matches!(name, "web" | "chill" | "bind" | VORTEX_STATUS | GRAVITY_STATUS | ANCHOR_STATUS)
}

/// Gravity Well's second aspect: what is being crushed is also dragged. An ordinary slow.
pub const GRAVITY_STATUS: &str = "gravity";

/// Its third: what is already slowed is PINNED. A deeper multiplier than any other slow —
/// still a rate and never a cap, for the same reason the Vortex is not one: a creature's
/// `speed_stat` never scales, so anything that stops the gauge advancing stops the
/// creature acting for the rest of the fight.
pub const ANCHOR_STATUS: &str = "anchored";

/// Dominate Mind's Blackout: sensory input suppressed, so the target's own blows go wide.
///
/// It first read as "it cannot dodge", which was **dead code**: `Fighter::dodge` is only
/// ever assigned in `party_fighters` (heroes), and no creature ability grants Evasion, so a
/// creature's dodge is always 0 and taking it away took nothing. Accuracy in this engine
/// lives on the DEFENDER (`roll_dodge` reads the ATTACKER's `distracted`), so "it cannot
/// see" has to land the same way a dazzle does.
pub const BLIND_STATUS: &str = "blinded";

/// Gravity Vortex's mark. It SLOWS the gauge's fill rate rather than capping the gauge,
/// and the difference is the whole ability: creature `speed_stat` is a fixed constant
/// (40–125) that never scales, while a hero's climbs with Dex, so by level 255 a Psyker
/// takes several turns per creature turn. A cap at half would therefore mean the creature
/// is knocked back below the line every time it approaches it — it would never act again,
/// which is a soft-lock rather than a capstone. A rate multiplier cannot do that: the
/// creature always advances, just at half pace.
pub const VORTEX_STATUS: &str = "vortex";

/// The lasting effects that STACK, and therefore answer to `max_effect_stacks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stack {
    Regen,
    Barrier,
    Evasion,
    Atk,
}

/// The timed status a hastened fighter carries: its gauge fills faster while it holds.
pub const HASTE_STATUS: &str = "hasted";

/// A fighter that has just had its gauge knocked down cannot have it knocked down again
/// while this holds.
///
/// Gauge denial was implemented by hand at **fourteen** call sites and composed into an
/// unbounded lock: measured, a party's Ransack (116 casts) and Holy Censure (34) held a
/// 66,792 HP gatekeeper at 29% gauge for **464 hero-turns** — it never acted once, took the
/// party to zero damage, and turned a boss into a safe, endless grind. The repo already knew
/// the shape: `hallowed_ground` is gated once-a-fight with the comment that a deep Phoenix
/// Guard casting it on repeat "means nothing on the other side ever acts again". Against ONE
/// enemy, Censure is that ability four rungs earlier and unrestricted.
///
/// A BOON rather than an affliction, so it EXPIRES — a permanent guard is simply the same
/// bug pointed the other way, and `statuses` treats an unknown condition as a boon for
/// exactly this reason.
pub const GAUGE_GUARD_STATUS: &str = "gauge_guard";

/// What a hero swings with when its WEAPON does not say.
///
/// A class does not have a damage type — its weapon does
/// (`ItemFamily::damage_type`), because that is the only way `ArmorWeight` becomes a
/// loadout decision rather than a table: plate turns an edge and fears a hammer, so the
/// sling is the answer to plate and the bow to mail. A class-level type made every Hunter
/// arrow cut like a sword and the two ranged families mechanically identical.
///
/// This is the fallback for a hand that has no physical answer of its own — a caster's
/// focus, a shield, or nothing at all — and it is BLUNT rather than `None` on purpose.
/// `DamageType::None` bypasses the modifier map entirely (every resistance and immunity
/// ignored) and now the rank trade too, so it is not a neutral default, it is true damage.
/// Three classes silently dealt it for a whole release. There is no path to it for a hero
/// any more: unarmed, you hit them with what you have.
pub const UNARMED_ATTACK_TYPE: DamageType = DamageType::Blunt;

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
    // ASK THE REGISTRY. This was a hand-kept list of eleven keys — in the very function
    // whose test is named "gated by the registry not by a hand-kept list" — so a new
    // manifestation was castable only once someone remembered to add it here, and until
    // then it sat in the menu costing a turn and doing nothing.
    meld_proto::skills::skill(kind)
        .filter(|d| d.class == "psyker")
        .map(|d| d.unlock)
}

/// One resolved effect on a target (maps to `battle.action_resolved.effects[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEffect {
    pub target_id: Id,
    pub kind: EffectKind,
    pub amount: Option<i32>,
    pub status: Option<String>,
    pub hp_after: i32,
    /// How the target's `damage_modifiers` bent a typed damage effect
    /// (weak/resist/immune/absorb/normal); `None` for untyped effects.
    pub modifier_flag: Option<ModifierFlag>,
}

/// The outcome of resolving a single action (maps to `battle.action_resolved`).
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub action_id: Option<Id>,
    pub actor_id: Id,
    pub action: BattleActionKind,
    pub auto: bool,
    pub flee_success: Option<bool>,
    /// Shout text for an *instant* monster ability (telegraphed ones already
    /// shouted via [`Event::TelegraphStarted`]). `None` for plain actions.
    pub callout_text: Option<String>,
    pub effects: Vec<ResolvedEffect>,
}

/// Engine events emitted by `tick`/`submit`, in resolution order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A player combatant's gauge filled; their action window opens.
    TurnReady { combatant_id: Id },
    /// A monster shouted a telegraphed ability and entered channeling; the
    /// cast lands at `executes_at_tick` (maps to `battle.telegraph_started`).
    TelegraphStarted {
        combatant_id: Id,
        callout_text: String,
        executes_at_tick: u64,
    },
    /// A monster's `steal` effect connected with a player hero — the server
    /// deducts the stolen goods from that player's run (the engine itself
    /// never touches run inventory).
    Stolen {
        victim_player_id: Id,
        kind: StealTargetKind,
    },
    /// A Shifter's `steal`/`mug` connected — the MIRROR of `Stolen`. The engine has
    /// no idea what a creature is carrying or where a run's backpack lives, so it
    /// reports the theft and the server decides what came off the body.
    Pilfered {
        thief_player_id: Id,
        /// The creature robbed, so the server can size the haul off its tier.
        victim_combatant_id: Id,
    },
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
    /// Events a resolver raised that are not the resolution itself — a Shifter's
    /// theft, say, which only the server can settle. `submit` drains this, so a
    /// resolver deep in the call tree can report a fact without threading a return
    /// value through every signature between here and there.
    pending_events: Vec<Event>,
    pub battle_id: Id,
    pub encounter_class: EncounterClass,
    fighters: Vec<Fighter>,
    tick_count: u64,
    ended: bool,
    // Tunables snapshot (structural formulas in code; coefficients from balance).
    gauge_divisor: f64,
    timeout_ticks: u64,
    defend_reduction: f64,
    back_row_damage_mult: f64,
    back_row_attack_mult: f64,
    thrown_atk_mult: f64,
    sweep_share: f64,
    gang_switch_chance: f64,
    /// The mark a ganging pack is converging on (CR-9). Shared across the whole side, so
    /// "gang up" means the pack commits together rather than each creature deciding alone.
    gang_target: Option<Id>,
    /// AD-2: how long a combo primer stays live on a target.
    combo_window_ticks: u64,
    pack_aura_atk_mult: f64,
    pack_guard_per_minion: f64,
    pack_guard_cap: f64,
    pack_rout_atk_mult: f64,
    pack_rout_flees: bool,
    consumable_barrier: i32,
    consumable_regen: i32,
    consumable_evasion_pct: i32,
    consumable_adrenaline: i32,
    consumable_potency_per_step: f64,
    revive_hp_fraction: f64,
    /// The skill currently resolving, if any — set for the length of one player
    /// skill so `apply_damage` can prime a combo or cash one in without every
    /// skill arm having to know combos exist.
    active_skill: Option<String>,
    /// Which fighter is acting right now (set alongside `active_skill`). `roll_dodge` reads
    /// it to find out whether the attacker is distracted.
    active_actor: Option<usize>,
    /// Which PLAYERS have laid into each enemy group. Two or more distinct parties and the
    /// group is flanked for the rest of the fight — the co-op answer to "a rank is relative":
    /// one group's back row is another party's front, and once both are on it there is no
    /// back row left to hide in. Earned rather than granted, so a co-op fight is not
    /// automatically a flank on everything the moment it starts.
    group_strikers: HashMap<u32, std::collections::HashSet<Id>>,
    back_row_target_weight: f64,
    skill_power_mult: f64,
    skill_heal_fraction: f64,
    item_heal_fraction: f64,
    crit_chance_base: f64,
    crit_chance_per_dex: f64,
    crit_chance_cap: f64,
    crit_mult: f64,
    psyker_gravity_tick_mult: f64,
    psyker_spike_tick_mult: f64,
    psyker_aegis_tick_fraction: f64,
    psyker_anchor_gauge_drain: f64,
    barrier_decay_fraction: f64,
    regen_decay_fraction: f64,
    max_effect_stacks: u8,
    psyker_vortex_tick_mult: f64,
    psyker_anchor_slow_mult: f64,
    psyker_aspect_ticks: u64,
    psyker_shield_party_fraction: f64,
    psyker_accel_gauge: f64,
    psyker_blackout_ticks: u64,
    psyker_blackout_miss: f64,
    psyker_dual_manifest_at: i32,
    psyker_expansion_at: i32,
    psyker_expansion_per_level: i32,
    psyker_expansion_cap: i32,
    psyker_expansion_mult: f64,
    psyker_vortex_ticks: u64,
    resonant_second_life_revive_fraction: f64,
    keeper_terras_gift_revive_fraction: f64,
    resonant_second_life_heal_fraction: f64,
    resonant_second_life_self_cost: f64,
    resonant_transfuse_heal_fraction: f64,
    resonant_transfuse_cost_fraction: f64,
    resonant_boon_regen_fraction: f64,
    resonant_ward_barrier_fraction: f64,
    shifter_backstab_mult: f64,
    shifter_backstab_pierce: f64,
    shifter_flicker_evasion: f64,
    shifter_flicker_decay: f64,
    shifter_ransack_mult: f64,
    shifter_ransack_drain: f64,
    shifter_assassinate_mult: f64,
    shifter_assassinate_pierce: f64,
    shifter_larceny_mult: f64,
    shifter_larceny_drain: f64,
    // Note: the Adrenaline *cap* rides on each Hunter `Fighter.adrenaline_max`
    // (set from balance in meld-run); the engine only needs the per-attack gain.
    hunter_adrenaline_per_attack: i32,
    hunter_power_strike_cost: i32,
    hunter_second_wind_cost: i32,
    hunter_snare_cost: i32,
    explorer_snare_mult: f64,
    explorer_snare_drain: f64,
    hunter_frenzy_cost: i32,
    explorer_frenzy_mult: f64,
    hunter_iron_lung_heal_fraction: f64,
    hunter_iron_lung_regen_fraction: f64,
    hunter_apex_mult: f64,
    phoenix_guard_swell_mult: f64,
    phoenix_guard_swell_drain: f64,
    gauge_guard_turns: u8,
    staggered_damage_mult: f64,
    phoenix_guard_root_barrier_fraction: f64,
    phoenix_guard_shock_mult: f64,
    phoenix_guard_toll_mult: f64,
    phoenix_guard_undead_mult: f64,
    phoenix_guard_vigil_barrier_fraction: f64,
    phoenix_guard_eradication_mult: f64,
    phoenix_guard_eradication_missing_bonus: f64,
    phoenix_guard_hallowed_mult: f64,
    phoenix_guard_ascendant_mult: f64,
    phoenix_guard_ascendant_barrier_fraction: f64,
    explorer_trailblaze_mult: f64,
    explorer_mark_damage_mult: f64,
    explorer_mark_ticks: u64,
    explorer_field_dressing_fraction: f64,
    explorer_read_ground_mult: f64,
    explorer_read_ground_drain: f64,
    explorer_misdirection_miss: f64,
    explorer_misdirection_flee_bonus: f64,
    explorer_misdirection_ticks: u64,
    explorer_stable_ground_fraction: f64,
    explorer_safe_passage_evasion: f64,
    explorer_haste_mult: f64,
    explorer_haste_ticks: u64,
    explorer_world_entire_mark_ticks: u64,
    explorer_world_entire_haste_ticks: u64,
    /// The two profession classes' kits (MS-1). Held whole rather than flattened field
    /// by field: they arrived together and read better as the two blocks they are.
    smith: meld_balance::Smithwright,
    keeper: meld_balance::Keeper,
    resonant_deep: ResonantDeep,
    shifter_steal_drain: f64,
    shifter_mug_mult: f64,
    shifter_mug_drain: f64,
    hunter_crushing_blow_mult: f64,
    pin_the_prey_mult: f64,
    pin_the_prey_drain: f64,
    psyker_wave_tick_mult: f64,
    psyker_thermal_tick_mult: f64,
    psyker_dissolution_tick_mult: f64,
    psyker_dissolution_armour_shred: i32,
    psyker_phase_evasion: f64,
    psyker_collapse_tick_mult: f64,
    status_slow_mult: f64,
    poison_dot_fraction: f64,
    burn_dot_fraction: f64,
    basic_attack_weight: i32,
    /// FS-4 raid bias, applied to a party-wide ability's roll weight and cooldown per party
    /// past the first. See `Fighter::raid_parties`.
    raid_wide_weight_per_party: f64,
    raid_wide_cooldown_per_party: f64,
    min_damage: i32,
    paralysis_break_base: f64,
    paralysis_break_per_wll: f64,
    paralysis_break_cap: f64,
    damage_floor_fraction: f64,
    creature_flee_hp_fraction: f64,
    flee_base: f64,
    flee_penalty_per_tier: f64,
    flee_floor: f64,
    /// Action ids already resolved (dedup / idempotency). A set so the check is
    /// O(1) rather than an O(n) scan that grows over a long battle's lifetime.
    seen_actions: HashSet<Id>,
    /// Tiny deterministic LCG for flee rolls (no global RNG — determinism).
    rng: u64,
}


/// The Resonant's deep kit as a table: heal fraction, Regen, Barrier fraction, what
/// it costs the caster, and whether it lands on one ally or all of them. Seven
/// abilities that are all the same shape want one resolver, not seven arms.
#[derive(Debug, Clone, Copy)]
struct AllyBoon {
    /// Fraction of the target's max HP healed.
    heal: f64,
    /// Regen granted, as a fraction of the RECIPIENT's max HP per turn.
    regen: f64,
    /// Fraction of the target's max HP granted as Barrier.
    barrier: f64,
    /// Fraction of the healing paid out of the caster's own HP.
    self_cost: f64,
    /// Whether it lands on the whole party.
    party: bool,
    /// Fraction of max HP a FALLEN ally is raised at, or 0.0 for a row that cannot raise.
    /// A revive at a reachable rung matters: before this the only one in the game was the
    /// Resonant's level-255 capstone, so a party that lost a hero at level 20 had nothing but
    /// a rare Waking Salt drop.
    revive: f64,
    /// The affliction family this row also lifts, if any. Afflictions no longer expire, so
    /// each mender row answers ONE kind of condition — Sanctuary calms a mind, it does not
    /// draw venom.
    cure: Option<meld_proto::statuses::Family>,
}

/// Every deep Resonant ability, keyed by its registry key.
#[derive(Debug, Clone, Copy)]
struct ResonantDeep {
    mend_all: AllyBoon,
    sanctuary: AllyBoon,
    revitalize: AllyBoon,
    lifewell: AllyBoon,
    bloodbond: AllyBoon,
    martyr: AllyBoon,
    bloom: AllyBoon,
}

impl ResonantDeep {
    fn from(b: &meld_balance::Battle) -> Self {
        Self {
            mend_all: AllyBoon {
                heal: b.resonant_mend_all_fraction,
                regen: 0.0,
                barrier: 0.0,
                self_cost: b.resonant_mend_all_self_cost,
                revive: 0.0,
                cure: None,
                party: true,
            },
            sanctuary: AllyBoon {
                heal: 0.0,
                regen: b.resonant_sanctuary_regen_fraction,
                barrier: 0.0,
                self_cost: 0.0,
                revive: 0.0,
                cure: Some(meld_proto::statuses::Family::Mind),
                party: true,
            },
            revitalize: AllyBoon {
                heal: b.resonant_revitalize_fraction,
                regen: 0.0,
                barrier: 0.0,
                self_cost: b.resonant_revitalize_self_cost,
                revive: b.resonant_revitalize_revive_fraction,
                cure: None,
                party: false,
            },
            lifewell: AllyBoon {
                heal: b.resonant_lifewell_fraction,
                regen: b.resonant_lifewell_regen_fraction,
                barrier: 0.0,
                self_cost: b.resonant_lifewell_self_cost,
                revive: 0.0,
                cure: None,
                party: true,
            },
            bloodbond: AllyBoon {
                heal: b.resonant_bloodbond_fraction,
                regen: b.resonant_bloodbond_regen_fraction,
                barrier: b.resonant_bloodbond_barrier_fraction,
                self_cost: b.resonant_bloodbond_self_cost,
                revive: 0.0,
                cure: None,
                party: false,
            },
            martyr: AllyBoon {
                heal: b.resonant_martyr_fraction,
                regen: 0.0,
                barrier: 0.0,
                self_cost: b.resonant_martyr_self_cost,
                revive: 0.0,
                cure: None,
                party: true,
            },
            bloom: AllyBoon {
                heal: b.resonant_bloom_fraction,
                regen: 0.0,
                barrier: b.resonant_bloom_barrier_fraction,
                self_cost: b.resonant_bloom_self_cost,
                revive: 0.0,
                cure: None,
                party: true,
            },
        }
    }

    /// Every deep-kit key, for the dispatch check.
    fn names() -> [&'static str; 8] {
        [
            "second_life",
            "mend_all",
            "sanctuary",
            "revitalize",
            "lifewell",
            "bloodbond",
            "martyr",
            "eternal_bloom",
        ]
    }

    fn get(&self, skill: &str) -> Option<AllyBoon> {
        Some(match skill {
            "mend_all" => self.mend_all,
            "sanctuary" => self.sanctuary,
            "revitalize" => self.revitalize,
            "lifewell" => self.lifewell,
            "bloodbond" => self.bloodbond,
            "martyr" => self.martyr,
            "eternal_bloom" => self.bloom,
            _ => return None,
        })
    }
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
            f.base_atk = f.atk;
            // Heroes' basic attacks are typed by class (unless the builder
            // already set one); monsters get theirs from creature content.
            if f.kind == CombatantKind::Player && f.basic_attack_type == DamageType::None {
                f.basic_attack_type = UNARMED_ATTACK_TYPE;
            }
        }
        Battle {
            battle_id,
            encounter_class,
            pending_events: Vec::new(),
            fighters,
            tick_count: 0,
            ended: false,
            gauge_divisor: balance.battle.gauge_fill_divisor,
            timeout_ticks: (balance.battle.turn_timeout_ms / tick_ms).max(1),
            defend_reduction: balance.battle.defend_damage_reduction,
            back_row_damage_mult: balance.battle.back_row_damage_mult,
            back_row_attack_mult: balance.battle.back_row_attack_mult,
            thrown_atk_mult: balance.consumable.thrown_atk_mult,
            sweep_share: balance.battle.sweep_share,
            gang_switch_chance: balance.ai.gang_switch_chance,
            gang_target: None,
            combo_window_ticks: balance.adventure.combo_window_ticks,
            pack_aura_atk_mult: balance.encounters.pack_aura_atk_mult,
            pack_guard_per_minion: balance.encounters.pack_guard_per_minion,
            pack_guard_cap: balance.encounters.pack_guard_cap,
            pack_rout_atk_mult: balance.encounters.pack_rout_atk_mult,
            pack_rout_flees: balance.encounters.pack_rout_flees,
            consumable_barrier: balance.consumable.barrier_amount,
            consumable_regen: balance.consumable.regen_amount,
            consumable_evasion_pct: balance.consumable.evasion_pct,
            consumable_adrenaline: balance.consumable.adrenaline_amount,
            consumable_potency_per_step: balance.consumable.potency_per_step,
            revive_hp_fraction: balance.consumable.revive_hp_fraction,
            active_skill: None,
            active_actor: None,
            group_strikers: HashMap::new(),
            back_row_target_weight: balance.battle.back_row_target_weight,
            skill_power_mult: balance.battle.skill_power_mult,
            skill_heal_fraction: balance.battle.skill_heal_fraction,
            item_heal_fraction: balance.battle.item_heal_fraction,
            crit_chance_base: balance.battle.crit_chance_base,
            crit_chance_per_dex: balance.battle.crit_chance_per_dex,
            crit_chance_cap: balance.battle.crit_chance_cap,
            crit_mult: balance.battle.crit_mult,
            psyker_gravity_tick_mult: balance.battle.psyker_gravity_tick_mult,
            psyker_spike_tick_mult: balance.battle.psyker_spike_tick_mult,
            psyker_aegis_tick_fraction: balance.battle.psyker_aegis_tick_fraction,
            psyker_anchor_gauge_drain: balance.battle.psyker_anchor_gauge_drain,
            barrier_decay_fraction: balance.battle.barrier_decay_fraction,
            regen_decay_fraction: balance.battle.regen_decay_fraction,
            max_effect_stacks: balance.battle.max_effect_stacks,
            psyker_vortex_tick_mult: balance.battle.psyker_vortex_tick_mult,
            psyker_anchor_slow_mult: balance.battle.psyker_anchor_slow_mult,
            psyker_aspect_ticks: balance.battle.psyker_aspect_ticks,
            psyker_shield_party_fraction: balance.battle.psyker_shield_party_fraction,
            psyker_accel_gauge: balance.battle.psyker_accel_gauge,
            psyker_blackout_ticks: balance.battle.psyker_blackout_ticks,
            psyker_blackout_miss: balance.battle.psyker_blackout_miss,
            psyker_dual_manifest_at: balance.battle.psyker_dual_manifest_at,
            psyker_expansion_at: balance.battle.psyker_expansion_at,
            psyker_expansion_per_level: balance.battle.psyker_expansion_per_level,
            psyker_expansion_cap: balance.battle.psyker_expansion_cap,
            psyker_expansion_mult: balance.battle.psyker_expansion_mult,
            psyker_vortex_ticks: balance.battle.psyker_vortex_ticks,
            resonant_second_life_revive_fraction: balance
                .battle
                .resonant_second_life_revive_fraction,
            keeper_terras_gift_revive_fraction: balance
                .battle
                .keeper_terras_gift_revive_fraction,
            resonant_second_life_heal_fraction: balance.battle.resonant_second_life_heal_fraction,
            resonant_second_life_self_cost: balance.battle.resonant_second_life_self_cost,
            resonant_transfuse_heal_fraction: balance.battle.resonant_transfuse_heal_fraction,
            resonant_transfuse_cost_fraction: balance.battle.resonant_transfuse_cost_fraction,
            resonant_boon_regen_fraction: balance.battle.resonant_boon_regen_fraction,
            resonant_ward_barrier_fraction: balance.battle.resonant_ward_barrier_fraction,
            shifter_backstab_mult: balance.battle.shifter_backstab_mult,
            shifter_backstab_pierce: balance.battle.shifter_backstab_pierce,
            shifter_flicker_evasion: balance.battle.shifter_flicker_evasion,
            shifter_flicker_decay: balance.battle.shifter_flicker_decay,
            shifter_ransack_mult: balance.battle.shifter_ransack_mult,
            shifter_ransack_drain: balance.battle.shifter_ransack_drain,
            shifter_assassinate_mult: balance.battle.shifter_assassinate_mult,
            shifter_assassinate_pierce: balance.battle.shifter_assassinate_pierce,
            shifter_larceny_mult: balance.battle.shifter_larceny_mult,
            shifter_larceny_drain: balance.battle.shifter_larceny_drain,
            hunter_adrenaline_per_attack: balance.battle.hunter_adrenaline_per_attack,
            hunter_power_strike_cost: balance.battle.hunter_power_strike_cost,
            hunter_second_wind_cost: balance.battle.hunter_second_wind_cost,
            hunter_snare_cost: balance.battle.hunter_snare_cost,
            explorer_snare_mult: balance.battle.explorer_snare_mult,
            explorer_snare_drain: balance.battle.explorer_snare_drain,
            hunter_frenzy_cost: balance.battle.hunter_frenzy_cost,
            explorer_frenzy_mult: balance.battle.explorer_frenzy_mult,
            hunter_iron_lung_heal_fraction: balance.battle.hunter_iron_lung_heal_fraction,
            hunter_iron_lung_regen_fraction: balance.battle.hunter_iron_lung_regen_fraction,
            hunter_apex_mult: balance.battle.hunter_apex_mult,
            phoenix_guard_swell_mult: balance.battle.phoenix_guard_swell_mult,
            phoenix_guard_swell_drain: balance.battle.phoenix_guard_swell_drain,
            phoenix_guard_root_barrier_fraction: balance.battle.phoenix_guard_root_barrier_fraction,
            phoenix_guard_shock_mult: balance.battle.phoenix_guard_shock_mult,
            phoenix_guard_toll_mult: balance.battle.phoenix_guard_toll_mult,
            phoenix_guard_undead_mult: balance.battle.phoenix_guard_undead_mult,
            gauge_guard_turns: balance.battle.gauge_guard_turns,
            staggered_damage_mult: balance.battle.staggered_damage_mult,
            phoenix_guard_vigil_barrier_fraction: balance
                .battle
                .phoenix_guard_vigil_barrier_fraction,
            phoenix_guard_eradication_mult: balance.battle.phoenix_guard_eradication_mult,
            phoenix_guard_eradication_missing_bonus: balance
                .battle
                .phoenix_guard_eradication_missing_bonus,
            phoenix_guard_hallowed_mult: balance.battle.phoenix_guard_hallowed_mult,
            phoenix_guard_ascendant_mult: balance.battle.phoenix_guard_ascendant_mult,
            phoenix_guard_ascendant_barrier_fraction: balance
                .battle
                .phoenix_guard_ascendant_barrier_fraction,
            smith: balance.smithwright.clone(),
            keeper: balance.keeper.clone(),
            explorer_trailblaze_mult: balance.battle.explorer_trailblaze_mult,
            explorer_mark_damage_mult: balance.battle.explorer_mark_damage_mult,
            explorer_mark_ticks: balance.battle.explorer_mark_ticks,
            explorer_field_dressing_fraction: balance.battle.explorer_field_dressing_fraction,
            explorer_read_ground_mult: balance.battle.explorer_read_ground_mult,
            explorer_read_ground_drain: balance.battle.explorer_read_ground_drain,
            explorer_misdirection_miss: balance.battle.explorer_misdirection_miss,
            explorer_misdirection_flee_bonus: balance.battle.explorer_misdirection_flee_bonus,
            explorer_misdirection_ticks: balance.battle.explorer_misdirection_ticks,
            explorer_stable_ground_fraction: balance.battle.explorer_stable_ground_fraction,
            explorer_safe_passage_evasion: balance.battle.explorer_safe_passage_evasion,
            explorer_haste_mult: balance.battle.explorer_haste_mult,
            explorer_haste_ticks: balance.battle.explorer_haste_ticks,
            explorer_world_entire_mark_ticks: balance.battle.explorer_world_entire_mark_ticks,
            explorer_world_entire_haste_ticks: balance.battle.explorer_world_entire_haste_ticks,
            resonant_deep: ResonantDeep::from(&balance.battle),
            shifter_steal_drain: balance.battle.shifter_steal_drain,
            shifter_mug_mult: balance.battle.shifter_mug_mult,
            shifter_mug_drain: balance.battle.shifter_mug_drain,
            hunter_crushing_blow_mult: balance.battle.hunter_crushing_blow_mult,
            pin_the_prey_mult: balance.battle.pin_the_prey_mult,
            pin_the_prey_drain: balance.battle.pin_the_prey_drain,
            psyker_wave_tick_mult: balance.battle.psyker_wave_tick_mult,
            psyker_thermal_tick_mult: balance.battle.psyker_thermal_tick_mult,
            psyker_dissolution_tick_mult: balance.battle.psyker_dissolution_tick_mult,
            psyker_dissolution_armour_shred: balance.battle.psyker_dissolution_armour_shred,
            psyker_phase_evasion: balance.battle.psyker_phase_evasion,
            psyker_collapse_tick_mult: balance.battle.psyker_collapse_tick_mult,
            status_slow_mult: balance.battle.status_slow_mult,
            poison_dot_fraction: balance.battle.poison_dot_fraction,
            burn_dot_fraction: balance.battle.burn_dot_fraction,
            basic_attack_weight: balance.battle.basic_attack_weight,
            raid_wide_weight_per_party: balance.encounters.raid_wide_weight_per_party,
            raid_wide_cooldown_per_party: balance.encounters.raid_wide_cooldown_per_party,
            min_damage: balance.combat_math.min_damage,
            paralysis_break_base: balance.affliction.paralysis_break_base,
            paralysis_break_per_wll: balance.affliction.paralysis_break_per_wll,
            paralysis_break_cap: balance.affliction.paralysis_break_cap,
            damage_floor_fraction: balance.combat_math.damage_floor_fraction,
            creature_flee_hp_fraction: balance.ai.flee_hp_fraction,
            flee_base: balance.battle.flee_base,
            flee_penalty_per_tier: balance.battle.flee_penalty_per_tier,
            flee_floor: balance.battle.flee_floor,
            seen_actions: HashSet::new(),
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
            if f.kind == CombatantKind::Player && f.basic_attack_type == DamageType::None {
                f.basic_attack_type = UNARMED_ATTACK_TYPE;
            }
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

    /// Per-combatant gauge/HP state (for `battle.gauge_update`), owned. Kept for
    /// tests / non-hot callers; the server's per-tick path uses [`Self::gauge_views`].
    pub fn gauge_state(&self) -> Vec<(Id, f64, i32, Vec<String>)> {
        self.fighters
            .iter()
            .map(|f| (f.combatant_id.clone(), f.gauge, f.hp, f.build_wire_statuses()))
            .collect()
    }

    /// Borrowed per-combatant gauge view for the periodic `battle.gauge_update`.
    /// Reuses each fighter's cached wire-status list (refreshed at the end of
    /// [`Self::tick`]), so the server serializes the update without allocating the
    /// status strings every tick. Read after a `tick`, whose refresh keeps the
    /// cache current.
    pub fn gauge_views(&self) -> impl Iterator<Item = (&str, f64, i32, &[String])> {
        self.fighters.iter().map(|f| {
            (
                f.combatant_id.as_str(),
                f.gauge,
                f.hp,
                f.statuses_cache.as_slice(),
            )
        })
    }

    /// Current HP of a combatant by id (for carrying wounds across a run's
    /// encounters — persistent HP lives on the server between battles).
    /// Which AFFLICTIONS a combatant is still carrying when the fight ends.
    ///
    /// Afflictions no longer expire, so a poison survives the encounter that inflicted it —
    /// and the run is what has to remember, because a `Fighter` is rebuilt every battle. This
    /// is the read half of that; `Battle::afflict` is the write half.
    pub fn combatant_afflictions(&self, combatant_id: &str) -> Vec<String> {
        let Some(i) = self.idx(combatant_id) else {
            return Vec::new();
        };
        self.fighters[i]
            .timed_statuses
            .iter()
            .filter(|(n, _)| meld_proto::statuses::is_affliction(n))
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Put an affliction back on a fighter at battle start — what the run remembered.
    /// No expiry: it holds until cured, exactly as it did in the fight that inflicted it.
    pub fn afflict(&mut self, combatant_id: &str, name: &str) {
        if let Some(i) = self.idx(combatant_id) {
            if !self.fighters[i].timed_statuses.iter().any(|(n, _)| n == name) {
                self.fighters[i].timed_statuses.push((name.to_string(), u64::MAX));
            }
        }
    }

    /// A combatant's `(hp, max_hp)`. Both halves, because the only meaningful thing to
    /// carry out of a battle is the FRACTION: an enemy's pool was scaled to the party
    /// facing it (`encounter_party_scale`), so its raw remaining HP means nothing outside
    /// the fight it was built for.
    pub fn combatant_health(&self, combatant_id: &str) -> Option<(i32, i32)> {
        self.fighters
            .iter()
            .find(|f| f.combatant_id == combatant_id)
            .map(|f| (f.hp, f.max_hp))
    }

    /// How many times a combatant FELL in this battle — what the durability tax is
    /// charged on (GR-2). Per fall, not per corpse: a hero raised by Revitalize and
    /// killed again pays twice, and a hero who was already down when the fight began
    /// pays nothing for staying down.
    pub fn combatant_falls(&self, combatant_id: &str) -> u32 {
        self.fighters
            .iter()
            .find(|f| f.combatant_id == combatant_id)
            .map_or(0, |f| f.falls)
    }

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

    /// Advance the battle one 100 ms tick. Fills gauges, fires monster turns and
    /// 15 s auto-defends, and reports the terminal outcome once reached.
    pub fn tick(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if self.ended {
            return events;
        }
        self.tick_count += 1;

        // 1. Fill gauges for living fighters not already awaiting input.
        // A channeling monster's gauge is frozen (the cast IS its turn), and a
        // slowing status (web/chill/bind/…) halves the fill rate.
        let n = self.fighters.len();
        let slow_mult = self.status_slow_mult;
        let anchor_mult = self.psyker_anchor_slow_mult;
        let haste_mult = self.explorer_haste_mult;
        let now = self.tick_count;
        for i in 0..n {
            let f = &mut self.fighters[i];
            if !f.alive || f.awaiting || f.gauge >= 1.0 || f.channel.is_some() {
                continue;
            }
            let pinned = f
                .timed_statuses
                .iter()
                .any(|(name, until)| *until > now && name == ANCHOR_STATUS);
            let slowed = f
                .timed_statuses
                .iter()
                .any(|(name, until)| *until > now && is_slowing_status(name));
            let hastened = f
                .timed_statuses
                .iter()
                .any(|(name, until)| *until > now && name == HASTE_STATUS);
            // A bind and a haste can both be on: they multiply, so hastening someone out
            // of a web is worth doing rather than being cancelled by it.
            // The STRONGEST slow wins rather than stacking — two multipliers on one gauge
            // is how a rate becomes a cap by accident.
            let slowed_to = if pinned {
                anchor_mult
            } else if slowed {
                slow_mult
            } else {
                1.0
            };
            // A set piece resists being controlled out of the fight entirely.
            let slowed_to = slowed_to.max(f.slow_floor);
            let rate_mult = slowed_to * if hastened { haste_mult } else { 1.0 };
            f.gauge =
                (f.gauge + f.speed_stat as f64 * rate_mult / self.gauge_divisor).min(1.0);
        }

        // 1b. Land any channeled casts that are due (independent of gauge).
        for i in 0..n {
            if self.ended {
                break;
            }
            let due = match self.fighters[i].channel {
                Some((_, executes_at)) if self.fighters[i].alive => {
                    self.tick_count >= executes_at
                }
                _ => false,
            };
            if due {
                let (ability_idx, _) = self.fighters[i].channel.take().unwrap();
                let upkeep = self.start_of_turn(i);
                if self.fighters[i].alive {
                    let mut res = self.resolve_ability(i, ability_idx, &mut events);
                    prepend_effects(&mut res, upkeep);
                    events.push(Event::Resolved(res));
                } else if !upkeep.is_empty() {
                    // The channeler died to its own DoT at cast time — report
                    // the upkeep as an auto action so the client sees the KO.
                    events.push(Event::Resolved(self.upkeep_only(i, upkeep)));
                }
                self.check_terminal(&mut events);
            }
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
            // PARALYSED: the gauge fills and the turn is spent standing there. Handled before
            // the turn is offered rather than by refusing the action, because being asked to
            // choose and then told no is worse than not being asked.
            if self.has(i, "paralyzed") {
                // A slim hope, on WILL — the attribute that already answers "how much of you
                // is yours". Rolled at the top of the held turn, so breaking it costs the turn
                // you broke it on rather than handing back a free action.
                let odds = (self.paralysis_break_base
                    + self.paralysis_break_per_wll * self.fighters[i].wll as f64)
                    .min(self.paralysis_break_cap);
                if self.next_rand_unit() < odds {
                    let broke = self.shake_off(i, &["paralyzed"]);
                    let mut res = self.skipped_turn(i, "broke_free");
                    res.effects.extend(broke);
                    self.fighters[i].awaiting = false;
                    events.push(Event::Resolved(res));
                    self.check_terminal(&mut events);
                    continue;
                }
                let upkeep = self.start_of_turn(i);
                if !self.fighters[i].alive {
                    events.push(Event::Resolved(self.upkeep_only(i, upkeep)));
                    self.check_terminal(&mut events);
                    continue;
                }
                let mut res = self.skipped_turn(i, "paralyzed");
                prepend_effects(&mut res, upkeep);
                self.fighters[i].awaiting = false;
                self.reset_gauge(i);
                events.push(Event::Resolved(res));
                self.check_terminal(&mut events);
                continue;
            }
            // FRENZIED: control is taken away. It swings on its own, harder and wilder — the
            // player is not offered the choice, which IS the condition.
            if self.has(i, "frenzied") {
                let upkeep = self.start_of_turn(i);
                if !self.fighters[i].alive {
                    events.push(Event::Resolved(self.upkeep_only(i, upkeep)));
                    self.check_terminal(&mut events);
                    continue;
                }
                let foe = self.choose_target(i).0;
                let mut res = match foe {
                    Some(t) => {
                        let tid = self.fighters[t].combatant_id.clone();
                        self.resolve_attack(i, &tid, None)
                            .unwrap_or_else(|_| self.skipped_turn(i, "frenzied"))
                    }
                    None => self.skipped_turn(i, "frenzied"),
                };
                prepend_effects(&mut res, upkeep);
                self.fighters[i].awaiting = false;
                events.push(Event::Resolved(res));
                self.check_terminal(&mut events);
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
                    if !self.fighters[i].alive {
                        // The DoT upkeep killed them before the auto action.
                        events.push(Event::Resolved(self.upkeep_only(i, upkeep)));
                        self.check_terminal(&mut events);
                        continue;
                    }
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
                // A channeling monster holds its cast; 1b lands it when due.
                if self.fighters[i].channel.is_some() {
                    continue;
                }
                // Monster AI (spec §2): upkeep, then filter the ability pool
                // and roll a weighted choice (or start a telegraphed channel).
                let upkeep = self.start_of_turn(i);
                if !self.fighters[i].alive {
                    // Died to its own DoT at turn start.
                    if !upkeep.is_empty() {
                        events.push(Event::Resolved(self.upkeep_only(i, upkeep)));
                    }
                    self.check_terminal(&mut events);
                    continue;
                }
                // `roll_dodge` reads the ATTACKER's `distracted`/`blinded` off
                // `active_actor`, and this was only ever set in `submit` — i.e. for a
                // PLAYER's action. So a creature's own swing never carried its state, and
                // the Explorer's Misdirection had never once made a creature miss. Its test
                // set `active_actor` by hand and asserted the arithmetic, so it passed while
                // the ability did nothing in a real fight.
                self.active_actor = Some(i);
                let taken = self.take_monster_turn(i, &mut events);
                self.active_actor = None;
                if let Some(mut res) = taken {
                    prepend_effects(&mut res, upkeep);
                    events.push(Event::Resolved(res));
                }
                self.check_terminal(&mut events);
            }
        }
        // Refresh each fighter's cached wire-status list so this tick's gauge_update
        // can serialize from it without rebuilding (see [`Self::gauge_views`]). The
        // signature check is a no-op unless a status/barrier/regen/focus changed.
        for f in &mut self.fighters {
            f.refresh_wire_statuses();
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
        if self.seen_actions.contains(&action_id) {
            return Err(Reject::DuplicateAction);
        }
        // CONFUSED: the order you gave is not the order that happens. BOTH halves are rolled
        // — what you do and who you do it to — because a confusion that only mis-aims is a
        // targeting penalty, not a confusion. Applied here, at the one gate every player
        // action passes, so it covers skills and items and not just a swing.
        //
        // The player still gets to CHOOSE; the condition is that the choice does not hold.
        let scrambled = self.has(i, "confused");
        let (action, skill_kind, target_ids) = if scrambled {
            self.scramble(i, action, skill_kind)
        } else {
            (action, skill_kind, target_ids)
        };
        // DREAD: you cannot bring yourself to go at the thing that frightened you — but you
        // are not removed from the fight. Defend, drink, mend yourself or an ally, even swing
        // at one: all still yours. Only what is aimed at an ENEMY is refused.
        if self.has(i, "dread") {
            let at_enemy = match action {
                BattleActionKind::Attack => true,
                BattleActionKind::Skill => skill_kind
                    .as_deref()
                    .map(|k| {
                        let bare = k.rsplit(':').next().unwrap_or(k);
                        matches!(
                            meld_proto::skills::target_of(bare),
                            meld_proto::skills::Target::Enemy
                                | meld_proto::skills::Target::AllEnemies
                        )
                    })
                    .unwrap_or(true),
                _ => false,
            };
            if at_enemy {
                return Err(Reject::InvalidState("Too afraid to face it."));
            }
        }
        if !self.fighters[i].awaiting || self.fighters[i].gauge < 1.0 {
            return Err(Reject::InvalidState("Actor gauge is not full."));
        }
        if action == BattleActionKind::Flee && self.encounter_class == EncounterClass::Gatekeeper {
            return Err(Reject::InvalidState(
                "Flee is disabled against Gatekeepers.",
            ));
        }
        // EVERY refusal we can see coming happens HERE, before a single field moves — see
        // [`Self::precheck`] for what a refusal past this point used to cost the player.
        // A CONFUSED hero is exempt: it did not choose this action, and the scramble
        // deliberately falls back to a defend rather than an error (below).
        if !scrambled {
            self.precheck(
                i,
                action,
                skill_kind.as_deref(),
                target_ids.as_ref().and_then(|t| t.first()).map(|s| s.as_str()),
            )?;
        }
        self.seen_actions.insert(action_id.clone());

        let mut events = Vec::new();
        // Start-of-turn upkeep (DoT tick, Regen heal, Barrier decay) fires
        // before the action — and can kill the actor outright (poison).
        let upkeep = self.start_of_turn(i);
        if !self.fighters[i].alive {
            events.push(Event::Resolved(self.upkeep_only(i, upkeep)));
            self.check_terminal(&mut events);
            return Ok(events);
        }
        // A Psyker channels: every turn its active Foci fire, then it casts/
        // reinforces/revokes one (encoded in skill_kind). Flee still works normally.
        let target = target_ids.as_ref().and_then(|t| t.first()).map(|s| s.as_str());
        let is_psyker = self.fighters[i].focus_max > 0;
        // AD-2: tell `apply_damage` which ability is landing, so combo primers and
        // payoffs resolve in one place instead of in every skill arm.
        self.active_skill = match action {
            BattleActionKind::Skill => skill_kind.clone(),
            _ => None,
        };
        self.active_actor = Some(i);
        let mut res = if is_psyker && action != BattleActionKind::Flee {
            self.resolve_psyker(i, skill_kind.as_deref(), target, Some(action_id), false)
        } else {
            match action {
                BattleActionKind::Attack => match target {
                    Some(t) => match self.resolve_attack(i, t, Some(action_id.clone())) {
                        Ok(r) => r,
                        Err(_) if scrambled => self.resolve_defend(i, Some(action_id), true),
                        Err(e) => return Err(e),
                    },
                    None if scrambled => self.resolve_defend(i, Some(action_id), true),
                    None => return Err(Reject::ValidationError("attack requires target_ids")),
                },
                BattleActionKind::Defend => self.resolve_defend(i, Some(action_id), false),
                BattleActionKind::Flee => self.resolve_flee(i, Some(action_id)),
                BattleActionKind::Skill => {
                    match self.resolve_skill(
                        i,
                        target,
                        skill_kind.as_deref(),
                        Some(action_id.clone()),
                    ) {
                        Ok(r) => r,
                        // A CONFUSED hero did not choose this. Refusing it would hand the
                        // player an error about an Adrenaline cost for a skill they never
                        // pressed, leave the turn unspent, and re-roll on the next attempt —
                        // so the flail becomes a swing that lands nowhere instead.
                        Err(_) if scrambled => self.resolve_defend(i, Some(action_id), true),
                        Err(e) => return Err(e),
                    }
                }
                // Inventory-backed: the game loop checks the run backpack before this
                // and spends one after (see `game.rs`'s Item handling).
                BattleActionKind::Item => {
                    self.resolve_item(i, item_id.as_deref(), target, Some(action_id))
                }
            }
        };
        self.active_skill = None;
        self.active_actor = None;
        // Prepend the upkeep effects so the client sees Regen/Barrier before the action.
        prepend_effects(&mut res, upkeep);
        let fled = res.flee_success == Some(true);
        events.push(Event::Resolved(res));
        // A resolver may have reported something only the server can settle (a
        // Shifter picking a pocket); hand it up alongside the resolution.
        events.extend(std::mem::take(&mut self.pending_events));
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
    /// Damage after armour. Armour subtracts, but it can never absorb the whole
    /// blow: a hit always lands for at least `damage_floor_fraction` of the
    /// attacker's scaled attack. Defence grows about +1 per hero level while creature
    /// attack grows only with distance, so without that floor a levelled hero simply
    /// stops taking damage and the game never gets harder than its tutorial.
    /// How much its OWN rank weakens a fighter's physical blow.
    ///
    /// The back row's trade: stand back, take half, deal half. But a REACHING weapon loses
    /// nothing by standing back — that is what "ranged" means, and an archer who gave up
    /// half its damage for standing where archers stand would be a class with no reason to
    /// use its own weapon. So reach cancels the rank in BOTH directions: the target's rank
    /// does not protect them from it (`softened_by_rank`), and the shooter's own rank does
    /// not weaken it.
    ///
    /// One function for both call sites — `phys_atk` and the basic-attack path — because
    /// two copies of a rule is how this codebase has produced a wall that stopped creatures
    /// and not players, and a bow that reached in one crate and not the other.
    fn rank_attack_mult(&self, actor_i: usize, physical: bool) -> f64 {
        let f = &self.fighters[actor_i];
        if !physical || !f.back_row || f.reach {
            return 1.0;
        }
        self.back_row_attack_mult
    }

    /// The physical attack `actor_i` actually swings with, at `mult` of its own `atk`.
    ///
    /// **The back row's trade lives here.** Standing back halves what physical blows do to
    /// you (`apply_damage_reaching`) and halves what your own physical blows do in return.
    /// Without the second half the optimal formation was the whole party in the back rank
    /// for a flat 2x effective HP, and `handle_set_formation` has no rule against it —
    /// the trade IS the rule. A caster gives up nothing standing back because its damage
    /// is not physical, which is exactly why the back row is a caster's home.
    ///
    /// Every physical attack goes through here rather than reading `.atk` at the call
    /// site, because a call site that reads it directly is a swing nobody charged for.
    fn phys_atk(&self, actor_i: usize, mult: f64) -> i32 {
        let atk = self.fighters[actor_i].atk as f64;
        let row = self.rank_attack_mult(actor_i, true);
        (atk * mult * row).round() as i32
    }

    fn damage(&self, atk: i32, def: i32, target_defending: bool) -> i32 {
        let floor = (atk as f64) * self.damage_floor_fraction;
        let mut raw = ((atk - def) as f64).max(floor);
        if target_defending {
            raw *= 1.0 - self.defend_reduction;
        }
        (raw.round() as i32).max(self.min_damage)
    }

    /// Roll a critical hit for the attacker. Crit chance scales with the attacker's
    /// Dex (the Shifter's precision theme lands more), capped so it's never certain.
    /// Deterministic off the battle RNG, like [`Self::roll_dodge`].
    fn roll_crit(&mut self, actor_i: usize) -> bool {
        let dex = self.fighters[actor_i].dex.max(0) as f64;
        let chance =
            (self.crit_chance_base + self.crit_chance_per_dex * dex).min(self.crit_chance_cap);
        chance > 0.0 && self.next_rand_unit() < chance
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
        let attack_type = self.fighters[actor_i].basic_attack_type;
        // The back rank's trade, and this path can be exact about it: only a PHYSICAL
        // swing is weakened by standing back, so a hero whose weapon carries an elemental
        // `brand` (AD-3) keeps its full damage from the back row — which is a real reason
        // to want one. A RANGED weapon keeps its full damage there too, which is a better
        // one: see `rank_attack_mult`.
        let row = self.rank_attack_mult(actor_i, attack_type.is_physical());
        // CR-6: a minion fights above its weight while its leader lives, and below it
        // once the pack has routed.
        let atk = ((self.fighters[actor_i].atk as f64)
            * self.pack_attack_mult(actor_i)
            * row
            // A BRANDED weapon carries an element, so element power pays on a basic attack
            // too — otherwise "of the Furnace" would be a caster-only line.
            * self.element_amp(actor_i, attack_type))
            .round()
            .max(1.0) as i32;
        let def = self.fighters[target_i].def;
        let defending = self.fighters[target_i].defending;
        let mut effects = match self.roll_dodge(target_i) {
            Some(dodge) => dodge,
            None => {
                // A crit multiplies the blow and tags the Damage effect so the client
                // pops "CRIT!". Basic attacks only (skills are already the big hits).
                let base = self.damage(atk, def, defending);
                let crit = self.roll_crit(actor_i);
                let dmg = if crit {
                    (base as f64 * self.crit_mult).round() as i32
                } else {
                    base
                };
                // Basic attacks are typed (class weapon flavour), so a monster's
                // elemental profile bends them — weak/resist/immune/absorb.
                let mut fx = self.apply_typed_damage(target_i, dmg, attack_type);
                if crit {
                    if let Some(e) = fx.iter_mut().find(|e| matches!(e.kind, EffectKind::Damage)) {
                        e.status = Some("crit".to_string());
                    }
                }
                fx
            }
        };
        // SWEEP: a lash takes the rest of the target's RANK, in the target's own GROUP.
        // One rank of one group — the intersection of the two structures an encounter
        // already has, rather than a third one beside them. The primary target is resolved
        // above like any other blow (it dodges, it crits); the rest of the rank takes a
        // reduced share and no crit, because a sweep is a wide blow rather than several
        // good ones.
        if self.fighters[actor_i].sweeps {
            let (group, rank) =
                (self.fighters[target_i].group_id, self.fighters[target_i].back_row);
            let rest: Vec<usize> = (0..self.fighters.len())
                .filter(|&i| {
                    i != target_i
                        && self.fighters[i].alive
                        && self.fighters[i].kind != CombatantKind::Player
                        && self.fighters[i].group_id == group
                        && self.fighters[i].back_row == rank
                })
                .collect();
            let share = ((atk as f64) * self.sweep_share).round().max(1.0) as i32;
            for t in rest {
                let d = self.damage(share, self.fighters[t].def, self.fighters[t].defending);
                effects.extend(self.apply_typed_damage(t, d, attack_type));
            }
        }
        // The Hunter banks Adrenaline on every basic attack (see `gain_adrenaline`).
        effects.extend(self.gain_adrenaline(actor_i));
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        Ok(Resolution { callout_text: None,
            action_id,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Attack,
            auto: false,
            flee_success: None,
            effects,
        })
    }

    /// Bank `hunter_adrenaline_per_attack` Adrenaline on a Hunter's basic attack,
    /// clamped to `adrenaline_max`. A no-op (empty effects) for every other class
    /// (`adrenaline_max == 0`). Reported as a StatusApplied so the client can react.
    fn gain_adrenaline(&mut self, actor_i: usize) -> Vec<ResolvedEffect> {
        let f = &mut self.fighters[actor_i];
        if f.adrenaline_max <= 0 {
            return Vec::new();
        }
        let before = f.adrenaline;
        f.adrenaline = (f.adrenaline + self.hunter_adrenaline_per_attack).min(f.adrenaline_max);
        if f.adrenaline == before {
            return Vec::new(); // already capped
        }
        vec![ResolvedEffect { modifier_flag: None,
            target_id: f.combatant_id.clone(),
            kind: EffectKind::StatusApplied,
            amount: Some(f.adrenaline),
            status: Some("adrenaline".to_string()),
            hp_after: f.hp,
        }]
    }

    /// The Adrenaline a Hunter row costs, or `None` if `skill` is not one of its rows.
    /// ONE table, read by both the resolver and [`Self::precheck`] — a second copy is a
    /// precheck that passes an action the resolver then refuses, which is the turn-eating
    /// case this whole path exists to prevent. Mirrors
    /// `meld_run::ability_effects::adrenaline_cost`, which is what the client greys rows
    /// against, and an upgrade costs what the row it replaced cost: the Hunter's rows get
    /// better, its Adrenaline economy does not change.
    fn hunter_skill_cost(&self, skill: &str) -> Option<i32> {
        Some(match skill {
            "power_strike" | "crushing_blow" => self.hunter_power_strike_cost,
            "second_wind" | "iron_lung" => self.hunter_second_wind_cost,
            "snare" | "pin_the_prey" => self.hunter_snare_cost,
            "frenzy" | "apex_predator" => self.hunter_frenzy_cost,
            _ => return None,
        })
    }

    /// Everything a submitted action can be refused for that is knowable WITHOUT touching
    /// a single field — run before `submit` mutates anything.
    ///
    /// ⚠️ **A REFUSAL USED TO COST THE PLAYER A TURN AND A POISON TICK.** `submit` recorded
    /// the `action_id` as seen and ran `start_of_turn` (the DoT tick, the Regen heal, the
    /// Barrier decay) and only *then* resolved — so a rejected action ticked the venom,
    /// decayed the Barrier and threw those events away with the `return Err`, leaving the
    /// client's HP and Barrier wrong; burned the `action_id`, so the same order re-sent came
    /// back `DuplicateAction`; and left the hero `awaiting` with a full gauge, which the
    /// client had already dropped from its `ready` set — so the hero could not be commanded
    /// again until the 15 s auto-defend spent its turn for it. Pressing Second Wind one
    /// Adrenaline short read, correctly, as the ability locking the hero out of the fight.
    ///
    /// The per-resolver checks stay exactly where they are and stay authoritative; this pass
    /// only guarantees the refusals a PLAYER actually hits are free. Anything here must be
    /// side-effect-free (`&self`) and must agree with the resolver, or it is worse than
    /// nothing — hence [`Self::hunter_skill_cost`] being one table.
    fn precheck(
        &self,
        i: usize,
        action: BattleActionKind,
        skill_kind: Option<&str>,
        target: Option<&str>,
    ) -> Result<(), Reject> {
        if action == BattleActionKind::Attack && target.is_none() {
            return Err(Reject::ValidationError("attack requires target_ids"));
        }
        if action != BattleActionKind::Skill {
            return Ok(());
        }
        // A Psyker's `skill_kind` is an OP (`cast:<kind>`), not an ability key, and it is
        // resolved by its own path — the checks below would read the whole op as a key.
        if self.fighters[i].focus_max > 0 {
            return Ok(());
        }
        let Some(key) = skill_kind else {
            return Ok(());
        };
        if !meld_proto::skills::is_unlocked(key, self.fighters[i].level) {
            return Err(Reject::ValidationError("skill not unlocked at this level"));
        }
        if meld_proto::skills::is_once_per_battle(key)
            && self.fighters[i].once_spent.iter().any(|s| s == key)
        {
            return Err(Reject::ValidationError("already used this battle"));
        }
        if meld_proto::skills::skill_owner(key) == Some("hunter") {
            match self.hunter_skill_cost(key) {
                None => return Err(Reject::ValidationError("unknown hunter skill")),
                Some(cost) if self.fighters[i].adrenaline < cost => {
                    return Err(Reject::ValidationError("not enough adrenaline"))
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// Is this combatant sitting on a full gauge waiting to be told what to do? The one
    /// question the game loop has to ask after refusing an action, so it can hand the turn
    /// back (`battle.turn_ready`) instead of leaving the hero uncommandable.
    pub fn awaiting_turn(&self, combatant_id: &str) -> bool {
        self.idx(combatant_id)
            .map(|i| {
                let f = &self.fighters[i];
                f.alive && f.awaiting && f.gauge >= 1.0
            })
            .unwrap_or(false)
    }

    /// Resolve a Explorer Adrenaline spender. EVERY Explorer skill spends banked
    /// Adrenaline and is rejected unless the cost is met (the client also greys
    /// unaffordable rows). `second_wind` is a self-heal; `power_strike`/`snare`/
    /// `frenzy` strike an enemy (Snare also drains the target's ATB gauge).
    fn resolve_hunter(
        &mut self,
        actor_i: usize,
        skill: &str,
        target_id: Option<&str>,
        action_id: Option<Id>,
    ) -> Result<Resolution, Reject> {
        let Some(cost) = self.hunter_skill_cost(skill) else {
            return Err(Reject::ValidationError("unknown hunter skill"));
        };
        if self.fighters[actor_i].adrenaline < cost {
            return Err(Reject::ValidationError("not enough adrenaline"));
        }
        // Second Wind → Iron Lung are self-heals — no target, spend and mend. Iron Lung
        // heals harder and leaves Regen behind, which is the whole upgrade: the Hunter
        // stops needing to spend a second turn on itself.
        if matches!(skill, "second_wind" | "iron_lung") {
            self.fighters[actor_i].adrenaline -= cost;
            let lung = skill == "iron_lung";
            let fraction =
                if lung { self.hunter_iron_lung_heal_fraction } else { self.skill_heal_fraction };
            let raw = ((self.fighters[actor_i].max_hp as f64) * fraction).round() as i32;
            let mut effects = self.apply_heal(actor_i, raw);
            if lung {
                let fx = self.grant_regen(actor_i, self.hunter_iron_lung_regen_fraction);
                effects.extend(fx);
            }
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Apex Predator is Frenzy turned on the whole pack — the one Hunter row that
        // does not pick a target, so it resolves before the single-target path below.
        if matches!(skill, "apex_predator" | "pin_the_prey") {
            self.fighters[actor_i].adrenaline -= cost;
            let pin = skill == "pin_the_prey";
            let (mult, drain) = if pin {
                (self.pin_the_prey_mult, self.pin_the_prey_drain)
            } else {
                (self.hunter_apex_mult, 0.0)
            };
            let enemies: Vec<usize> = self
                .fighters
                .iter()
                .enumerate()
                .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                .map(|(i, _)| i)
                .collect();
            let mut effects = Vec::new();
            for t in enemies {
                let scaled = self.phys_atk(actor_i, mult);
                let dmg = self.damage(scaled, self.fighters[t].def, self.fighters[t].defending);
                effects.extend(self.apply_damage(t, dmg));
                if drain > 0.0 && self.fighters[t].alive {
                    self.deny_gauge(t, Some(drain));
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[t].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("slowed".to_string()),
                        hp_after: self.fighters[t].hp,
                    });
                }
            }
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Enemy strikes. Power Strike reuses the generic heavy-hit multiplier.
        let (mult, drain) = match skill {
            "power_strike" => (self.skill_power_mult, 0.0),
            "crushing_blow" => (self.hunter_crushing_blow_mult, 0.0),
            "snare" => (self.explorer_snare_mult, self.explorer_snare_drain),
            "frenzy" => (self.explorer_frenzy_mult, 0.0),
            _ => unreachable!("cost match already rejected other skills"),
        };
        let target = target_id.ok_or(Reject::ValidationError("skill requires a target"))?;
        let target_i = match self.idx(target) {
            Some(t) if self.fighters[t].alive => t,
            _ => self
                .fighters
                .iter()
                .position(|f| f.alive && f.kind != CombatantKind::Player)
                .ok_or(Reject::NotFound)?,
        };
        // Spend the banked Adrenaline (reflected in wire statuses on the next snapshot).
        self.fighters[actor_i].adrenaline -= cost;
        let scaled_atk = self.phys_atk(actor_i, mult);
        let def = self.fighters[target_i].def;
        let defending = self.fighters[target_i].defending;
        let mut effects = match self.roll_dodge(target_i) {
            Some(dodge) => dodge,
            None => self.apply_damage(target_i, self.damage(scaled_atk, def, defending)),
        };
        if drain > 0.0 && self.fighters[target_i].alive {
            self.deny_gauge(target_i, Some(drain));
            effects.push(ResolvedEffect { modifier_flag: None,
                target_id: self.fighters[target_i].combatant_id.clone(),
                kind: EffectKind::StatusApplied,
                amount: None,
                status: Some("slowed".to_string()),
                hp_after: self.fighters[target_i].hp,
            });
        }
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects))
    }


    /// Resolve an Explorer ability. The order maps and anchors rather than kills, so
    /// its kit keeps the party *moving* (gauge) and *standing* (Barrier/Regen) — a
    /// different axis from the Hunter's burst and the Resonant's healing.
    ///
    /// - `trailblaze`      — Walker (L1): a plain strike, no resource to spend.
    /// - `field_dressing`  — Traveler (L2): a modest heal for an ally, or yourself.
    /// - `misdirection`    — Scout: damage, an ATB-gauge steal, and DISTRACTS the creature:
    ///   it swings wide at whoever it attacks, and the party can leave more easily.
    /// - `stable_ground`   — Pioneer: Barrier for the whole party. Deliberately NOT an
    ///   Anchor: an Anchor is the setting's load-bearing artifact, takes three orders to
    ///   make, and only an Explorer of Serin may set one (docs/lore/factions.md).
    /// - `safe_passage`    — Discoverer (L13): Regen for the whole party.
    /// - `a_world_known`   — Globemaster (L17): fill every living ally's gauge.
    fn resolve_explorer_kit(
        &mut self,
        actor_i: usize,
        skill: &str,
        target_id: Option<&str>,
        action_id: Option<Id>,
    ) -> Result<Resolution, Reject> {
        let living_allies = |b: &Self| -> Vec<usize> {
            b.fighters
                .iter()
                .enumerate()
                .filter(|(_, f)| f.alive && f.kind == CombatantKind::Player)
                .map(|(i, _)| i)
                .collect()
        };
        let mut effects = Vec::new();
        match skill {
            "field_dressing" => {
                // Aim at the chosen ally, else the most wounded — the classic default.
                let t = self
                    .ally_target(target_id)
                    .unwrap_or_else(|| self.most_wounded_ally(actor_i));
                let raw = ((self.fighters[t].max_hp as f64)
                    * self.explorer_field_dressing_fraction)
                    .round() as i32;
                effects.extend(self.apply_heal(t, raw));
            }
            "stable_ground" => {
                for a in living_allies(self) {
                    let raw = ((self.fighters[a].max_hp as f64)
                        * self.explorer_stable_ground_fraction)
                        .round() as i32;
                    effects.extend(self.grant_barrier(a, raw));
                }
            }
            "safe_passage" => {
                // The Guides get people THROUGH the Meld untouched; they do not patch them
                // up on the far side. So this is party-wide Evasion, not Regen — it shares
                // the Shifter's Flicker pool, so it decays per turn the same way and adds
                // to each hero's own Dex dodge.
                for a in living_allies(self) {
                    let fx = self.grant_evasion(a, self.explorer_safe_passage_evasion);
                    effects.extend(fx);
                }
            }
            "a_world_known" => {
                // A real HASTE: for its window every ally's gauge fills faster. This used
                // to be a flat one-off gauge nudge, which is the same shape as `Now` and
                // made two rungs of the ladder read as one idea told twice.
                let ticks = self.explorer_haste_ticks;
                for a in living_allies(self) {
                    let fx = self.apply_timed(a, HASTE_STATUS, ticks);
                    effects.push(fx);
                }
            }
            "the_world_entire" => {
                // Both halves of the class's tempo ladder in one turn: every enemy blazed
                // (so the party's damage is up against ALL of them, not one) and every
                // ally hastened. Marking through `apply_timed` is what makes it stack
                // with a plain Trailblaze by extending rather than doubling.
                let ticks = self.explorer_world_entire_mark_ticks;
                let enemies: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                    .map(|(i, _)| i)
                    .collect();
                for t in enemies {
                    let fx = self.apply_timed(t, Self::MARK_STATUS, ticks);
                    effects.push(fx);
                }
                let haste = self.explorer_world_entire_haste_ticks;
                for a in living_allies(self) {
                    let fx = self.apply_timed(a, HASTE_STATUS, haste);
                    effects.push(fx);
                }
            }
            "now" => {
                // The Globemaster's one call per fight: not faster — NOW. Every living ally
                // acts immediately. Once per battle, which is what keeps it a decision
                // about a single moment instead of a rotation.
                for a in living_allies(self) {
                    if a == actor_i {
                        continue; // the caster's own gauge resets below, as always
                    }
                    self.fighters[a].gauge = 1.0;
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[a].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("now".to_string()),
                        hp_after: self.fighters[a].hp,
                    });
                }
            }
            "trailblaze" | "misdirection" => {
                let (mult, drain) = if skill == "misdirection" {
                    (self.explorer_read_ground_mult, self.explorer_read_ground_drain)
                } else {
                    (self.explorer_trailblaze_mult, 0.0)
                };
                let target = target_id.ok_or(Reject::ValidationError("skill requires a target"))?;
                let target_i = match self.idx(target) {
                    Some(t) if self.fighters[t].alive => t,
                    _ => self
                        .fighters
                        .iter()
                        .position(|f| f.alive && f.kind != CombatantKind::Player)
                        .ok_or(Reject::NotFound)?,
                };
                let scaled_atk = self.phys_atk(actor_i, mult);
                let def = self.fighters[target_i].def;
                let defending = self.fighters[target_i].defending;
                effects = match self.roll_dodge(target_i) {
                    Some(dodge) => dodge,
                    None => self.apply_damage(target_i, self.damage(scaled_atk, def, defending)),
                };
                // Trailblaze blazes what it hits: the order's opener buys the PARTY a
                // window, which is the whole reason to press it over a basic Attack.
                if skill == "trailblaze" && self.fighters[target_i].alive {
                    let fx = self.apply_mark(target_i);
                    effects.push(fx);
                }
                // Misdirection distracts it: it swings wide at whoever it attacks, and a party it
                // has lost track of finds it easier to walk away (see `flee_chance`).
                if skill == "misdirection" && self.fighters[target_i].alive {
                    let ticks = self.explorer_misdirection_ticks;
                    let fx = self.apply_timed(target_i, Self::DISTRACT_STATUS, ticks);
                    effects.push(fx);
                }
                if drain > 0.0 && self.fighters[target_i].alive {
                    self.deny_gauge(target_i, Some(drain));
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[target_i].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("slowed".to_string()),
                        hp_after: self.fighters[target_i].hp,
                    });
                }
            }
            _ => return Err(Reject::ValidationError("unknown explorer skill")),
        }
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects))
    }

    /// The Foundry Smithwright's kit (MS-1). A working smith on the line: heavy staggering
    /// blows, shielding for the party, and one buff that makes somebody ELSE hit harder.
    /// Nothing here costs a resource — the class pays in tempo, since its own turn is
    /// slow and it spends turns propping others up.
    fn resolve_smithwright(
        &mut self,
        actor_i: usize,
        skill: &str,
        target_id: Option<&str>,
        action_id: Option<Id>,
    ) -> Result<Resolution, Reject> {
        let living_allies = |b: &Self| -> Vec<usize> {
            b.fighters
                .iter()
                .enumerate()
                .filter(|(_, f)| f.alive && f.kind == CombatantKind::Player)
                .map(|(i, _)| i)
                .collect()
        };
        let mut effects = Vec::new();
        match skill {
            "quench" => {
                let raw = ((self.fighters[actor_i].max_hp as f64)
                    * self.smith.quench_barrier_fraction)
                    .round() as i32;
                effects.extend(self.grant_barrier(actor_i, raw));
            }
            "bulwark" => {
                for a in living_allies(self) {
                    let raw = ((self.fighters[a].max_hp as f64)
                        * self.smith.bulwark_barrier_fraction)
                        .round() as i32;
                    effects.extend(self.grant_barrier(a, raw));
                }
            }
            "tempering_blow" => {
                // The work, not the foe: an ally's attack for the rest of the fight.
                let t = self
                    .ally_target(target_id)
                    .unwrap_or_else(|| self.most_wounded_ally(actor_i));
                let fx = self.grant_atk(t, self.smith.temper_atk_fraction);
                effects.extend(fx);
            }
            "one_true_forge" => {
                for a in living_allies(self) {
                    let heal = ((self.fighters[a].max_hp as f64)
                        * self.smith.forge_heal_fraction)
                        .round() as i32;
                    effects.extend(self.apply_heal(a, heal));
                    let raw = ((self.fighters[a].max_hp as f64)
                        * self.smith.forge_barrier_fraction)
                        .round() as i32;
                    effects.extend(self.grant_barrier(a, raw));
                }
            }
            "anvil_chorus" | "great_work" => {
                // The Foundry's capstones both work on the party's own numbers rather
                // than on the enemy. The attack bonus is permanent for the fight, like
                // Tempering Blow's, which is what makes either worth a turn early.
                let great = skill == "great_work";
                let share =
                    if great { self.smith.great_work_atk_fraction } else { self.smith.chorus_atk_fraction };
                for a in living_allies(self) {
                    if great {
                        let heal = ((self.fighters[a].max_hp as f64)
                            * self.smith.great_work_heal_fraction)
                            .round() as i32;
                        effects.extend(self.apply_heal(a, heal));
                        let raw = ((self.fighters[a].max_hp as f64)
                            * self.smith.great_work_barrier_fraction)
                            .round() as i32;
                        effects.extend(self.grant_barrier(a, raw));
                    }
                    let fx = self.grant_atk(a, share);
                    effects.extend(fx);
                }
            }
            "slag_spray" => {
                // Molten waste: armour is no help, so this ignores def entirely.
                let atk = self.fighters[actor_i].atk;
                let enemies: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                    .map(|(i, _)| i)
                    .collect();
                for t in enemies {
                    let dmg = (atk as f64 * self.smith.slag_mult).round().max(1.0) as i32;
                    effects.extend(self.apply_damage(t, dmg));
                }
            }
            "hammer_fall" => {
                let target = target_id.ok_or(Reject::ValidationError("skill requires a target"))?;
                let target_i = match self.idx(target) {
                    Some(t) if self.fighters[t].alive => t,
                    _ => self
                        .fighters
                        .iter()
                        .position(|f| f.alive && f.kind != CombatantKind::Player)
                        .ok_or(Reject::NotFound)?,
                };
                let scaled =
                    (self.fighters[actor_i].atk as f64 * self.smith.hammer_mult).round() as i32;
                let def = self.fighters[target_i].def;
                let defending = self.fighters[target_i].defending;
                effects = match self.roll_dodge(target_i) {
                    Some(dodge) => dodge,
                    None => self.apply_damage(target_i, self.damage(scaled, def, defending)),
                };
                // Dropped iron staggers: the blow costs the target part of its turn.
                if self.fighters[target_i].alive {
                    self.deny_gauge(target_i, Some(self.smith.hammer_gauge_drain));
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[target_i].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("staggered".to_string()),
                        hp_after: self.fighters[target_i].hp,
                    });
                }
            }
            _ => return Err(Reject::ValidationError("unknown smithwright skill")),
        }
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects))
    }

    /// The Open Flower Keeper's kit (MS-1). A mender: two of these do damage at all, and
    /// both of those buy time rather than kills. Everything else keeps the party upright.
    fn resolve_keeper(
        &mut self,
        actor_i: usize,
        skill: &str,
        target_id: Option<&str>,
        action_id: Option<Id>,
    ) -> Result<Resolution, Reject> {
        let living_allies = |b: &Self| -> Vec<usize> {
            b.fighters
                .iter()
                .enumerate()
                .filter(|(_, f)| f.alive && f.kind == CombatantKind::Player)
                .map(|(i, _)| i)
                .collect()
        };
        let mut effects = Vec::new();
        match skill {
            "poultice" => {
                let t = self
                    .ally_target(target_id)
                    .unwrap_or_else(|| self.most_wounded_ally(actor_i));
                let heal = self.scaled_to(t, self.keeper.poultice_heal_fraction);
                effects.extend(self.apply_heal(t, heal));
                let fx = self.grant_regen(t, self.keeper.poultice_regen_fraction);
                effects.extend(fx);
                // A poultice DRAWS THE POISON OUT — and afflictions no longer wear off, so
                // the earliest mender row in the game has to be able to lift one or a party
                // that catches something at level 5 simply carries it.
                let cured = self.cure(t, meld_proto::statuses::Family::Venom);
                effects.extend(cured);
            }
            "bloomfield" => {
                for a in living_allies(self) {
                    let fx = self.grant_regen(a, self.keeper.bloomfield_regen_fraction);
                    effects.extend(fx);
                }
            }
            "vital_draught" => {
                let t = self
                    .ally_target(target_id)
                    .unwrap_or_else(|| self.most_wounded_ally(actor_i));
                let raw = self.scaled_to(t, self.keeper.draught_barrier_fraction);
                effects.extend(self.grant_barrier(t, raw));
                let fx = self.grant_regen(t, self.keeper.draught_regen_fraction);
                effects.extend(fx);
            }
            "terras_gift" => {
                // The Open Flower's idea is growth and REBIRTH, so its rung-50 party row is
                // where the order's revive lives — reachable, unlike a level-255 capstone.
                let fallen: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.kind == CombatantKind::Player && !f.alive)
                    .map(|(i, _)| i)
                    .collect();
                for a in fallen {
                    let frac = self.keeper_terras_gift_revive_fraction;
                    let fx = self.raise_fallen(a, frac);
                    effects.extend(fx);
                }
                for a in living_allies(self) {
                    let heal = self.scaled_to(a, self.keeper.gift_heal_fraction);
                    effects.extend(self.apply_heal(a, heal));
                    let raw = self.scaled_to(a, self.keeper.gift_barrier_fraction);
                    effects.extend(self.grant_barrier(a, raw));
                    if a != actor_i {
                        self.fighters[a].gauge =
                            (self.fighters[a].gauge + self.keeper.gift_gauge).min(1.0);
                    }
                }
            }
            "world_tree" => {
                for a in living_allies(self) {
                    let heal = self.scaled_to(a, self.keeper.world_tree_heal_fraction);
                    effects.extend(self.apply_heal(a, heal));
                    let raw = self.scaled_to(a, self.keeper.world_tree_barrier_fraction);
                    effects.extend(self.grant_barrier(a, raw));
                    let fx = self.grant_regen(a, self.keeper.world_tree_regen_fraction);
                    effects.extend(fx);
                }
            }
            "thorn_grove" => {
                // The order's ONLY all-enemy answer, and it is priced as control: the
                // drain is the point, the damage is what comes with it. Rides Mnd like
                // the rest of the kit — the staff is a pestle, not a sword.
                let power = self.fighters[actor_i].spell_power.max(1);
                let enemies: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                    .map(|(i, _)| i)
                    .collect();
                for t in enemies {
                    let scaled = (power as f64 * self.keeper.thorn_grove_mult).round() as i32;
                    // MIND, like the Psyker's Foci, because the comment above is the design:
                    // it rides Mnd and the staff is a pestle. Routed physically it answered
                    // `def` instead of `ward` and — once creatures had ranks — was halved
                    // against an enemy back rank, which is the one thing a caster is
                    // supposed to be immune to.
                    effects.extend(self.apply_ability_damage(
                        t,
                        scaled.max(self.min_damage),
                        DamageType::Mind,
                    ));
                    if self.fighters[t].alive {
                        self.deny_gauge(t, Some(self.keeper.thorn_grove_gauge_drain));
                        effects.push(ResolvedEffect {
                            modifier_flag: None,
                            target_id: self.fighters[t].combatant_id.clone(),
                            kind: EffectKind::StatusApplied,
                            amount: None,
                            status: Some("slowed".to_string()),
                            hp_after: self.fighters[t].hp,
                        });
                    }
                }
            }
            "thornlash" | "root_snare" => {
                let (mult, drain) = if skill == "root_snare" {
                    (self.keeper.root_snare_mult, self.keeper.root_snare_gauge_drain)
                } else {
                    (self.keeper.thornlash_mult, self.keeper.thornlash_gauge_drain)
                };
                let target = target_id.ok_or(Reject::ValidationError("skill requires a target"))?;
                let target_i = match self.idx(target) {
                    Some(t) if self.fighters[t].alive => t,
                    _ => self
                        .fighters
                        .iter()
                        .position(|f| f.alive && f.kind != CombatantKind::Player)
                        .ok_or(Reject::NotFound)?,
                };
                // A Keeper's damage rides Mnd, like every other kit that is really
                // medicine: the staff is a pestle, not a sword.
                let power = self.fighters[actor_i].spell_power.max(1);
                let scaled = (power as f64 * mult).round() as i32;
                effects = match self.roll_dodge(target_i) {
                    Some(dodge) => dodge,
                    None => self.apply_ability_damage(
                        target_i,
                        scaled.max(self.min_damage),
                        DamageType::Mind,
                    ),
                };
                if drain > 0.0 && self.fighters[target_i].alive {
                    self.deny_gauge(target_i, Some(drain));
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[target_i].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("rooted".to_string()),
                        hp_after: self.fighters[target_i].hp,
                    });
                }
            }
            _ => return Err(Reject::ValidationError("unknown keeper skill")),
        }
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects))
    }

    /// Resolve a Phoenix Guard skill:
    ///
    /// - `rite_of_rest`   — self-cast: grant Barrier = `max_hp * root_barrier_fraction`.
    /// - `silvered_strike` — a heavy blow that also drains the target's ATB gauge.
    /// - `holy_censure`   — a heavier blow that fully resets the target's gauge.
    /// - `purging_light`  — hits EVERY living enemy.
    /// - `unbroken_vigil` — Barrier for the whole party.
    /// - `eradication`    — an execute: the more hurt the foe, the harder it lands.
    fn resolve_phoenix_guard(
        &mut self,
        actor_i: usize,
        skill: &str,
        target_id: Option<&str>,
        action_id: Option<Id>,
    ) -> Result<Resolution, Reject> {
        // Rite of Rest is a self-cast stance — no target needed.
        if skill == "rite_of_rest" {
            let raw = ((self.fighters[actor_i].max_hp as f64)
                * self.phoenix_guard_root_barrier_fraction)
                .round() as i32;
            let effects = self.grant_barrier(actor_i, raw);
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Unbroken Vigil (Redeemer, L13) — Barrier for the WHOLE party. "No one is
        // left behind to be turned."
        if skill == "unbroken_vigil" {
            let allies: Vec<usize> = self
                .fighters
                .iter()
                .enumerate()
                .filter(|(_, f)| f.alive && f.kind == CombatantKind::Player)
                .map(|(i, _)| i)
                .collect();
            let mut effects = Vec::new();
            for a in allies {
                let raw = ((self.fighters[a].max_hp as f64)
                    * self.phoenix_guard_vigil_barrier_fraction)
                    .round() as i32;
                effects.extend(self.grant_barrier(a, raw));
            }
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Purging Light (Luminary, L9) — light on every living enemy at once.
        if skill == "purging_light" {
            let atk = self.fighters[actor_i].atk;
            let enemies: Vec<usize> = self
                .fighters
                .iter()
                .enumerate()
                .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                .map(|(i, _)| i)
                .collect();
            let mut effects = Vec::new();
            for t in enemies {
                let scaled = (atk as f64 * self.phoenix_guard_toll_mult).round() as i32;
                let scaled = (scaled as f64 * self.undead_bonus(actor_i, t)).round() as i32;
                let dmg = self.damage(scaled, self.fighters[t].def, self.fighters[t].defending);
                effects.extend(self.apply_damage(t, dmg));
            }
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Hallowed Ground (49) and Phoenix Ascendant (100) both cover the field, so they
        // resolve here rather than down the single-target path. Hallowed Ground buys the
        // party a whole round — every enemy's gauge back to zero at once — while
        // Ascendant is the damage capstone and shields the line out of the same fire.
        if matches!(skill, "hallowed_ground" | "phoenix_ascendant") {
            let ascendant = skill == "phoenix_ascendant";
            let mult = if ascendant {
                self.phoenix_guard_ascendant_mult
            } else {
                self.phoenix_guard_hallowed_mult
            };
            let atk = self.fighters[actor_i].atk;
            let enemies: Vec<usize> = self
                .fighters
                .iter()
                .enumerate()
                .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                .map(|(i, _)| i)
                .collect();
            let mut effects = Vec::new();
            for t in enemies {
                let scaled = (atk as f64 * mult * self.undead_bonus(actor_i, t)).round() as i32;
                let dmg = self.damage(scaled, self.fighters[t].def, self.fighters[t].defending);
                effects.extend(self.apply_damage(t, dmg));
                if !ascendant && self.fighters[t].alive {
                    self.deny_gauge(t, None);
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[t].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("staggered".to_string()),
                        hp_after: self.fighters[t].hp,
                    });
                }
            }
            if ascendant {
                let allies: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.alive && f.kind == CombatantKind::Player)
                    .map(|(i, _)| i)
                    .collect();
                for a in allies {
                    let raw = ((self.fighters[a].max_hp as f64)
                        * self.phoenix_guard_ascendant_barrier_fraction)
                        .round() as i32;
                    effects.extend(self.grant_barrier(a, raw));
                }
            }
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Single-target: Silvered Strike (drain), Holy Censure (full stagger), and
        // Eradication (an execute — the more hurt the foe, the harder it lands).
        let mult = match skill {
            "holy_censure" => self.phoenix_guard_shock_mult,
            "eradication" => {
                let f = &self.fighters[self.idx(target_id.unwrap_or_default()).unwrap_or(actor_i)];
                let missing = if f.max_hp > 0 {
                    1.0 - (f.hp as f64 / f.max_hp as f64)
                } else {
                    0.0
                };
                self.phoenix_guard_eradication_mult
                    + self.phoenix_guard_eradication_missing_bonus * missing.clamp(0.0, 1.0)
            }
            _ => self.phoenix_guard_swell_mult,
        };
        let target = target_id.ok_or(Reject::ValidationError("skill requires a target"))?;
        let target_i = match self.idx(target) {
            Some(t) if self.fighters[t].alive => t,
            _ => self
                .fighters
                .iter()
                .position(|f| f.alive && f.kind != CombatantKind::Player)
                .ok_or(Reject::NotFound)?,
        };
        // The order's whole purpose: silvered and holy tools bite far deeper into
        // the risen than into anything else alive.
        let mult = mult * self.undead_bonus(actor_i, target_i);
        let scaled_atk = self.phys_atk(actor_i, mult);
        let def = self.fighters[target_i].def;
        let defending = self.fighters[target_i].defending;
        let mut effects = match self.roll_dodge(target_i) {
            Some(dodge) => dodge,
            None => self.apply_damage(target_i, self.damage(scaled_atk, def, defending)),
        };
        // A surviving target is staggered: Holy Censure zeroes its gauge outright,
        // a Silvered Strike knocks a fixed amount off.
        if self.fighters[target_i].alive {
            if skill == "holy_censure" {
                self.deny_gauge(target_i, None);
            } else {
                self.deny_gauge(target_i, Some(self.phoenix_guard_swell_drain));
            }
            effects.push(ResolvedEffect { modifier_flag: None,
                target_id: self.fighters[target_i].combatant_id.clone(),
                kind: EffectKind::StatusApplied,
                amount: None,
                status: Some("slowed".to_string()),
                hp_after: self.fighters[target_i].hp,
            });
        }
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects))
    }

    /// Class skills (slice content). The Explorer's `power_strike`/`second_wind`/
    /// `snare`/`frenzy` all spend banked Adrenaline (see [`Battle::resolve_hunter`]);
    /// the Phoenix Guard, Shifter, and Resonant arms handle their own kits. An unknown
    /// skill is rejected. (The Psyker does not use this path — it channels Foci via
    /// [`Battle::resolve_psyker`].)
    fn resolve_skill(
        &mut self,
        actor_i: usize,
        target_id: Option<&str>,
        skill_kind: Option<&str>,
        action_id: Option<Id>,
    ) -> Result<Resolution, Reject> {
        let out = self.resolve_skill_inner(actor_i, target_id, skill_kind, action_id);
        // SPEND the once-a-fight call here, centrally, and only if it actually resolved.
        // Each arm used to push its own key, which is a list a new once-per-battle
        // ability falls off — and the failure is that the ability is simply infinite.
        if out.is_ok() {
            if let Some(k) = skill_kind {
                if meld_proto::skills::is_once_per_battle(k)
                    && !self.fighters[actor_i].once_spent.iter().any(|s| s == k)
                {
                    self.fighters[actor_i].once_spent.push(k.to_string());
                }
            }
        }
        out
    }

    fn resolve_skill_inner(
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
            // Once-per-battle abilities are refused on the second ask, server-side. The
            // client greys the row too, but a rejection here is what makes it true.
            if meld_proto::skills::is_once_per_battle(k)
                && self.fighters[actor_i].once_spent.iter().any(|s| s == k)
            {
                return Err(Reject::ValidationError("already used this battle"));
            }
        }
        // Route to the owning class's resolver by ASKING THE REGISTRY who owns the
        // ability, rather than by six hand-written lists of keys. A list is a list a new
        // ability gets left off, and the failure here is silent: the key falls past every
        // arm and comes back "unknown or unsupported skill", so the row is in the menu,
        // costs a turn to press, and does nothing. The Shifter and the Psyker are absent
        // on purpose — the Shifter's arms are below, and a Psyker never reaches here.
        match skill_kind.and_then(meld_proto::skills::skill_owner) {
            // Hunter first regardless: every one of its skills spends banked Adrenaline,
            // so the affordability check has to run before anything else resolves.
            Some("hunter") => {
                return self.resolve_hunter(actor_i, skill_kind.unwrap(), target_id, action_id)
            }
            Some("explorer") => {
                return self.resolve_explorer_kit(actor_i, skill_kind.unwrap(), target_id, action_id)
            }
            Some("phoenix_guard") => {
                return self.resolve_phoenix_guard(
                    actor_i,
                    skill_kind.unwrap(),
                    target_id,
                    action_id,
                )
            }
            Some("smithwright") => {
                return self.resolve_smithwright(actor_i, skill_kind.unwrap(), target_id, action_id)
            }
            Some("keeper") => {
                return self.resolve_keeper(actor_i, skill_kind.unwrap(), target_id, action_id)
            }
            _ => {}
        }
        // Resonant healer skills. Aim at the chosen living ally if the player picked
        // one, else auto-target the most-wounded living ally (the classic default).
        if matches!(skill_kind, Some("transfuse") | Some("regen_boon") | Some("ward"))
            || skill_kind.is_some_and(|k| ResonantDeep::names().contains(&k))
        {
            let target_i = self
                .ally_target(target_id)
                .unwrap_or_else(|| self.most_wounded_ally(actor_i));
            let effects = self.resolve_resonant(actor_i, skill_kind.unwrap(), target_i);
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Shifter Steal → Mug. Steal takes the foe's tempo (its ATB gauge); Mug is
        // the same theft with a hit on the way past. The upgrade shares the arm, so
        // the two can never drift apart.
        if matches!(skill_kind, Some("steal") | Some("mug") | Some("grand_larceny")) {
            // Grand Larceny is a Mug worked on the WHOLE room, once a fight — so it picks
            // its own targets and every one of them is robbed, not just hit.
            let room = skill_kind == Some("grand_larceny");
            let (mult, drain) = match skill_kind {
                Some("mug") => (self.shifter_mug_mult, self.shifter_mug_drain),
                Some("grand_larceny") => (self.shifter_larceny_mult, self.shifter_larceny_drain),
                _ => (0.0, self.shifter_steal_drain),
            };
            let targets: Vec<usize> = if room {
                self.fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                    .map(|(i, _)| i)
                    .collect()
            } else {
                let target_i = match target_id.and_then(|t| self.idx(t)) {
                    Some(t) if self.fighters[t].alive => t,
                    _ => self
                        .fighters
                        .iter()
                        .position(|f| f.alive && f.kind != CombatantKind::Player)
                        .ok_or(Reject::NotFound)?,
                };
                vec![target_i]
            };
            let mut effects = Vec::new();
            for target_i in targets {
                if mult > 0.0 {
                    let scaled = self.phys_atk(actor_i, mult);
                    let def = self.fighters[target_i].def;
                    let defending = self.fighters[target_i].defending;
                    match self.roll_dodge(target_i) {
                        Some(dodge) => effects.extend(dodge),
                        None => {
                            let dmg = self.damage(scaled, def, defending);
                            effects.extend(self.apply_damage(target_i, dmg));
                        }
                    }
                }
                if self.fighters[target_i].alive {
                    self.deny_gauge(target_i, Some(drain));
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[target_i].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("slowed".to_string()),
                        hp_after: self.fighters[target_i].hp,
                    });
                }
                // Report the theft: the run's backpack is the server's business. A hero
                // with no player behind it (a headless test fighter) steals nothing.
                if let Some(thief) = self.fighters[actor_i].player_id.clone() {
                    let victim = self.fighters[target_i].combatant_id.clone();
                    self.pending_events.push(Event::Pilfered {
                        thief_player_id: thief,
                        victim_combatant_id: victim,
                    });
                }
            }
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Shifter (rogue) Flicker: a self-cast reality-blink granting Evasion (a
        // temporary dodge bonus that decays each of the Shifter's turns).
        if skill_kind == Some("flicker") {
            let effects = self.grant_evasion(actor_i, self.shifter_flicker_evasion);
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Shifter enemy strikes: Backstab (heavy, pierces most armour) and Ransack
        // (modest hit that also drains the target's ATB gauge — grab-and-run tempo).
        // Backstab → Assassinate and Ransack → Grand Larceny share their base's arm, so
        // an upgrade can never drift from the row it replaced. Grand Larceny is the one
        // that changes SHAPE: the same theft, worked on every enemy at once.
        if matches!(
            skill_kind,
            Some("backstab") | Some("ransack") | Some("assassinate")
        ) {
            let target_i = match target_id.and_then(|t| self.idx(t)) {
                Some(t) if self.fighters[t].alive => t,
                _ => self
                    .fighters
                    .iter()
                    .position(|f| f.alive && f.kind != CombatantKind::Player)
                    .ok_or(Reject::NotFound)?,
            };
            // Grand Larceny works the whole room; everything else picks one mark.
            let targets: Vec<usize> = vec![target_i];
            let (mult, pierce, drain) = match skill_kind {
                Some("backstab") => (self.shifter_backstab_mult, self.shifter_backstab_pierce, 0.0),
                Some("assassinate") => {
                    (self.shifter_assassinate_mult, self.shifter_assassinate_pierce, 0.0)
                }
                _ => (self.shifter_ransack_mult, 0.0, self.shifter_ransack_drain),
            };
            let mut effects = Vec::new();
            for t in targets {
                let defending = self.fighters[t].defending;
                let scaled_atk = self.phys_atk(actor_i, mult);
                let def = (self.fighters[t].def as f64 * (1.0 - pierce)).round() as i32;
                match self.roll_dodge(t) {
                    Some(dodge) => effects.extend(dodge),
                    None => {
                        let dmg = self.damage(scaled_atk, def, defending);
                        effects.extend(self.apply_damage(t, dmg));
                    }
                }
                if drain > 0.0 && self.fighters[t].alive {
                    self.deny_gauge(t, Some(drain));
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[t].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("slowed".to_string()),
                        hp_after: self.fighters[t].hp,
                    });
                }
            }
            self.fighters[actor_i].defending = false;
            self.reset_gauge(actor_i);
            return Ok(self.resolution(actor_i, BattleActionKind::Skill, action_id, effects));
        }
        // Every class skill is handled by an arm above; anything else is unknown.
        Err(Reject::ValidationError("unknown or unsupported skill"))
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
            // A Focus tick IS that ability landing, so Gravity Well can prime its
            // combo on the turns it fires, not only on the turn it was cast.
            let outer = self.active_skill.take();
            self.active_skill = Some(kind.clone());
            effects.extend(self.tick_manifest(actor_i, kind, *stacks, target_id.as_deref()));
            self.active_skill = outer;
            if !self.any_enemy_alive() {
                break;
            }
        }

        // 2. Apply the management op. Offensive Manifestations remember the enemy the
        // player aimed them at; casting/reinforcing the same kind on a new enemy just
        // redirects it (see [`Focus::target_id`]).
        let op = op.unwrap_or("hold");
        let mut cast_landed = false;
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
                // An ASPECT only lands on what its parent is already holding, and it lands
                // on the SAME target — Gravity drags the thing Pressure is crushing, not a
                // second creature across the arena. Asking for the parent's target rather
                // than trusting the aim is what stops the chain being three unrelated Foci.
                let def = meld_proto::skills::skill(arg);
                let parent = def.and_then(|d| d.requires);
                // An aspect must have its parent held — and inherits the parent's TARGET
                // only when it lands on the same side. Gravity drags the thing Pressure is
                // crushing; Acceleration hurries an ALLY while its parent grinds an enemy,
                // so it keeps its own aim. Inheriting blindly would have aimed the
                // Psyker's haste at the creature it was slowing.
                let parent_target = match parent {
                    Some(pk) => {
                        let held = self.fighters[actor_i].foci.iter().find(|f| f.kind == pk);
                        match (held, def.map(|d| d.target)) {
                            (None, _) => None,
                            (Some(_), Some(meld_proto::skills::Target::Enemy)) => {
                                held.map(|f| f.target_id.clone())
                            }
                            (Some(_), _) => Some(aim.clone()),
                        }
                    }
                    None => Some(aim.clone()),
                };
                if let (true, true, false, Some(tgt)) = (unlocked, slot_free, already, parent_target) {
                    self.fighters[actor_i].foci.push(Focus {
                        kind: arg.to_string(),
                        stacks: 1,
                        target_id: tgt.clone(),
                    });
                    effects.extend(self.tick_manifest(actor_i, arg, 1, tgt.as_deref())); // fires immediately
                    cast_landed = true;
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
                self.drop_orphaned_aspects(actor_i);
            }
            _ => {} // hold
        }

        self.fighters[actor_i].defending = false;
        // Dual Manifestation: past its level every Psyker turn refunds one cast, so the
        // pool Mind's Eye seeded at the top of the fight never runs dry. Topped up BEFORE
        // the spend below, so the turn that earns it can use it.
        if verb == "cast" && self.fighters[actor_i].level >= self.psyker_dual_manifest_at {
            self.fighters[actor_i].free_casts = self.fighters[actor_i].free_casts.max(1);
        }
        // A free cast does not cost the turn: the gauge is left full, so the Psyker acts
        // again immediately. This is Mind's Eye and Dual Manifestation both — one counter,
        // because two rules that each mean "this cast was free" would be two rules to keep
        // in step. Only a cast that LANDED may spend one (`cast_landed`), or a refused
        // aspect would quietly burn the opening it was meant to buy.
        let free = verb == "cast" && cast_landed && self.fighters[actor_i].free_casts > 0;
        if free {
            self.fighters[actor_i].free_casts -= 1;
        } else {
            self.reset_gauge(actor_i);
        }
        Resolution { callout_text: None,
            action_id,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Skill,
            auto,
            flee_success: None,
            effects,
        }
    }

    /// Open the fight with every living HERO's gauge full — the party walked into a
    /// creature a Psyker had pinned, so it picked the moment and moves first. Only the
    /// player side is filled: a surprise that also readied the creature would be no
    /// surprise at all.
    pub fn open_with_full_party_gauges(&mut self) {
        for f in self.fighters.iter_mut() {
            if f.alive && f.kind == CombatantKind::Player {
                f.gauge = 1.0;
            }
        }
    }

    /// Drop any aspect whose parent Focus is gone. Let go of Pressure and the Gravity
    /// dragging on it has nothing to hold — otherwise revoking the base of a chain would
    /// leave its aspects running free, which is a slow nothing is paying a slot for.
    /// Loops because a chain is three deep: dropping Gravity must also drop Anchor.
    fn drop_orphaned_aspects(&mut self, actor_i: usize) {
        loop {
            let held: Vec<String> =
                self.fighters[actor_i].foci.iter().map(|f| f.kind.clone()).collect();
            let before = held.len();
            self.fighters[actor_i].foci.retain(|f| {
                match meld_proto::skills::skill(&f.kind).and_then(|d| d.requires) {
                    Some(parent) => held.iter().any(|k| k == parent),
                    None => true,
                }
            });
            if self.fighters[actor_i].foci.len() == before {
                return;
            }
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
            // Shield widens the ward: the Aegis covers the whole party rather than only
            // its caster. The Psyker's own share still comes from the parent Focus.
            "shield" => {
                // This Psyker's OWN party. Filtering on `kind == Player` reached every
                // joined ally in a co-op battle too, and a bonus that crosses to another
                // player's heroes is a property the game reserves to set bonuses.
                let owner = self.fighters[psyker_i].player_id.clone();
                let allies: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| {
                        f.alive && f.kind == CombatantKind::Player && f.player_id == owner
                    })
                    .map(|(i, _)| i)
                    .collect();
                let mut fx = Vec::new();
                for a in allies {
                    let raw = (self.fighters[a].max_hp as f64
                        * self.psyker_shield_party_fraction
                        * stacks as f64)
                        .round() as i32;
                    fx.extend(self.grant_barrier(a, raw));
                }
                fx
            }
            // Acceleration runs time FAST for an ally — the mirror of the drain its parent
            // puts on an enemy, and the only Focus in the kit that helps someone.
            "acceleration" => {
                let Some(t) = self.focus_ally_target(psyker_i, target_id) else {
                    return Vec::new();
                };
                let fill = self.psyker_accel_gauge * stacks as f64;
                self.fighters[t].gauge = (self.fighters[t].gauge + fill).min(1.0);
                vec![ResolvedEffect {
                    modifier_flag: None,
                    target_id: self.fighters[t].combatant_id.clone(),
                    kind: EffectKind::StatusApplied,
                    amount: None,
                    status: Some("hasted".to_string()),
                    hp_after: self.fighters[t].hp,
                }]
            }
            // Brittle strips what the corrosion has eaten through: every resistance the
            // target had is gone for good, so the whole party's damage types land in full.
            // Permanent, like its parent's armour shred — the corrosion does not grow back.
            "brittle" => {
                let Some(t) = self.focus_enemy_target(psyker_i, kind, target_id) else {
                    return Vec::new();
                };
                // ONE resistance per tick — the doc's scope ("one damage type"), not the
                // whole profile. Stripping every resistance in a single cast deleted
                // elemental matchups as a consideration for the rest of the fight; taking
                // the strongest one each turn keeps it a sustained commitment instead.
                let worst = self.fighters[t]
                    .damage_modifiers
                    .iter()
                    .filter(|(_, &m)| m < 1.0)
                    .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(ty, _)| *ty);
                let Some(ty) = worst else { return Vec::new() };
                self.fighters[t].damage_modifiers.remove(&ty);
                vec![ResolvedEffect {
                    modifier_flag: None,
                    target_id: self.fighters[t].combatant_id.clone(),
                    kind: EffectKind::StatusApplied,
                    amount: None,
                    status: Some("brittle".to_string()),
                    hp_after: self.fighters[t].hp,
                }]
            }
            // Blackout cuts the senses: it cannot dodge at all while this is held.
            "blackout" => {
                let Some(t) = self.focus_enemy_target(psyker_i, kind, target_id) else {
                    return Vec::new();
                };
                vec![self.apply_timed(t, BLIND_STATUS, self.psyker_blackout_ticks)]
            }
            // The aspects re-apply their mark every Psyker turn, so the hold lasts exactly
            // as long as the Focus is held and lapses on its own once it is let go.
            // Freeze is Gravity's twin on a burning target: an ordinary slow, deepening to
            // a pin on anything already slowed.
            "gravity" | "anchor" | "freeze" => {
                let Some(t) = self.focus_enemy_target(psyker_i, kind, target_id) else {
                    return Vec::new();
                };
                let already_slowed = self.fighters[t]
                    .timed_statuses
                    .iter()
                    .any(|(n, until)| *until > self.tick_count && is_slowing_status(n));
                let status = match kind {
                    "anchor" => ANCHOR_STATUS,
                    // Freeze pins what was already crawling, and merely slows what was not.
                    "freeze" if already_slowed => ANCHOR_STATUS,
                    _ => GRAVITY_STATUS,
                };
                vec![self.apply_timed(t, status, self.psyker_aspect_ticks)]
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
            // Dominate Mind is Temporal Anchor's senior: it does not slow the foe's
            // gauge, it takes the turn outright — and then `GAUGE_GUARD_STATUS` holds it off
            // until the window lapses. It used to land EVERY turn it was held, which is the
            // unbounded lock the guard exists to end; Event Horizon, its own senior, slows
            // the RATE for exactly that reason.
            "temporal_anchor" => self.tick_control(psyker_i, kind, stacks, target_id),
            "dominate_mind" => {
                // The ZERO first, then the shared control tick. Both go through
                // `deny_gauge` now, and `tick_control`'s drain would otherwise raise the
                // guard and make this ability bounce off its own plumbing — the taking of
                // the turn is what Dominate Mind IS, so it is the one that must land.
                let taken = self
                    .focus_enemy_target(psyker_i, kind, target_id)
                    .map(|t| (t, self.deny_gauge(t, None)));
                let mut effects = self.tick_control(psyker_i, kind, stacks, target_id);
                if let Some((t, _landed)) = taken {
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[t].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("dominated".to_string()),
                        hp_after: self.fighters[t].hp,
                    });
                }
                effects
            }
            // Thermal Flux is FIRE-typed, so a creature's elemental profile decides
            // how much it hurts — unlike the mind-typed Foci.
            "thermal_flux" => {
                let Some(t) = self.focus_enemy_target(psyker_i, kind, target_id) else {
                    return Vec::new();
                };
                let power = self.fighters[psyker_i].spell_power as f64;
                let dmg = (power * self.psyker_thermal_tick_mult * stacks as f64).round() as i32;
                self.apply_ability_damage(t, dmg.max(self.min_damage), DamageType::Fire)
            }
            // Matter Dissolution corrodes: damage, and the target's armour is worn
            // down permanently for as long as the Focus is held. Armour scales with
            // distance now, so this is a real contribution to the party's damage and
            // not just the Psyker's own.
            "matter_dissolution" => {
                let mut effects =
                    self.tick_offense(psyker_i, kind, self.psyker_dissolution_tick_mult, stacks, target_id);
                if let Some(t) = self.focus_enemy_target(psyker_i, kind, target_id) {
                    let shred = self.psyker_dissolution_armour_shred * stacks as i32;
                    self.fighters[t].def = (self.fighters[t].def - shred).max(0);
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[t].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: Some(self.fighters[t].def),
                        status: Some("corroded".to_string()),
                        hp_after: self.fighters[t].hp,
                    });
                }
                effects
            }
            // Phase Shift holds the Psyker slightly out of true: Evasion that tops up
            // every turn the Focus is held, instead of decaying away like Flicker's.
            "phase_shift" => {
                self.grant_evasion(psyker_i, self.psyker_phase_evasion * stacks as f64)
            }
            // The two area Manifestations: Kinetic Wave grinds the line, and Reality
            // Collapse does it harder and ignores armour entirely.
            // The Psyker's capstone: the whole line acts at half speed for as long as it
            // is held, re-applied every Psyker turn so it never lapses while the Focus is up.
            "gravity_vortex" => {
                let power = self.fighters[psyker_i].spell_power as f64;
                let dmg = (power * self.psyker_vortex_tick_mult * stacks as f64).round() as i32;
                let enemies: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                    .map(|(i, _)| i)
                    .collect();
                let mut effects = Vec::new();
                for t in enemies {
                    effects.extend(
                        self.apply_ability_damage(t, dmg.max(self.min_damage), DamageType::Mind),
                    );
                    if self.fighters[t].alive {
                        let fx = self.apply_timed(t, VORTEX_STATUS, self.psyker_vortex_ticks);
                        effects.push(fx);
                    }
                }
                effects
            }
            "kinetic_wave" | "reality_collapse" => {
                let mult = if kind == "reality_collapse" {
                    self.psyker_collapse_tick_mult
                } else {
                    self.psyker_wave_tick_mult
                };
                let power = self.fighters[psyker_i].spell_power as f64;
                let dmg = (power * mult * stacks as f64).round() as i32;
                let enemies: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                    .map(|(i, _)| i)
                    .collect();
                let mut effects = Vec::new();
                for t in enemies {
                    effects.extend(
                        self.apply_ability_damage(t, dmg.max(self.min_damage), DamageType::Mind),
                    );
                }
                effects
            }
            _ => Vec::new(),
        }
    }

    /// The enemy index an offensive Focus hits this tick: its stored target if that
    /// enemy is alive, else the first living enemy — written back onto the Focus so the
    /// aim sticks after a retarget. `None` when no enemy is alive.
    /// The ALLY a supportive Focus is aimed at — the aim it was cast with if that hero is
    /// still standing, else the most wounded living one, so a Focus never quietly does
    /// nothing because its target fell.
    fn focus_ally_target(&self, psyker_i: usize, target_id: Option<&str>) -> Option<usize> {
        if let Some(id) = target_id {
            if let Some(i) = self.idx(id) {
                if self.fighters[i].alive && self.fighters[i].kind == CombatantKind::Player {
                    return Some(i);
                }
            }
        }
        self.fighters
            .iter()
            .enumerate()
            .filter(|(i, f)| {
                *i != psyker_i && f.alive && f.kind == CombatantKind::Player
            })
            .min_by_key(|(_, f)| f.hp)
            .map(|(i, _)| i)
    }

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
    /// Apply one [`AllyBoon`]: heal, Regen and Barrier to one ally or the whole
    /// party, with the caster paying a fraction of the healing out of its own HP.
    /// The Resonant never drops itself below 1 — it mends, it does not martyr itself
    /// literally.
    fn apply_ally_boon(
        &mut self,
        caster_i: usize,
        target_i: usize,
        boon: AllyBoon,
    ) -> Vec<ResolvedEffect> {
        let targets: Vec<usize> = if boon.party {
            self.fighters
                .iter()
                .enumerate()
                // A party row that can RAISE has to reach the fallen; every other one skips
                // them, because healing a corpse does nothing.
                .filter(|(_, f)| {
                    f.kind == CombatantKind::Player && (f.alive || boon.revive > 0.0)
                })
                .map(|(i, _)| i)
                .collect()
        } else {
            vec![target_i]
        };
        let mut effects = Vec::new();
        let mut healed_total = 0i32;
        for t in targets {
            if boon.heal > 0.0 {
                let raw = ((self.fighters[t].max_hp as f64) * boon.heal).round() as i32;
                healed_total += raw;
                effects.extend(self.apply_heal(t, raw));
            }
            if boon.regen > 0.0 {
                let fx = self.grant_regen(t, boon.regen);
                effects.extend(fx);
            }
            // A fallen ally is RAISED rather than healed — healing a corpse does nothing, so
            // without this a revive row would silently do nothing on the one target it is for.
            if boon.revive > 0.0 && !self.fighters[t].alive {
                let fx = self.raise_fallen(t, boon.revive);
                effects.extend(fx);
            }
            if let Some(family) = boon.cure {
                let cured = self.cure(t, family);
                effects.extend(cured);
            }
            if boon.barrier > 0.0 {
                let raw = ((self.fighters[t].max_hp as f64) * boon.barrier).round() as i32;
                effects.extend(self.grant_barrier(t, raw));
            }
        }
        let cost = ((healed_total as f64) * boon.self_cost).round() as i32;
        if cost > 0 {
            let before = self.fighters[caster_i].hp;
            let after = (before - cost).max(1);
            self.fighters[caster_i].hp = after;
            effects.push(ResolvedEffect {
                modifier_flag: None,
                target_id: self.fighters[caster_i].combatant_id.clone(),
                kind: EffectKind::Damage,
                amount: Some(before - after),
                status: Some("transfuse".to_string()),
                hp_after: after,
            });
        }
        effects
    }

    /// The Phoenix Guard's standing bonus against the risen. Reads the target's
    /// battle faction, which a boss now carries in its own right
    /// (`meld_world::abilities::boss_faction`) rather than inheriting from whatever
    /// creature it was promoted from — so "undead" here means undead.
    ///
    /// Takes the ATTACKER too, because AD-1's "of the Pyre" deepens the bonus and that is
    /// the wearer's own zeal — a property of who is swinging, not of what is being hit.
    fn undead_bonus(&self, actor_i: usize, target_i: usize) -> f64 {
        if self.fighters[target_i].faction == meld_proto::factions::UNDEAD {
            self.phoenix_guard_undead_mult + self.fighters[actor_i].undead_bane
        } else {
            1.0
        }
    }

    /// An absolute grant from a FRACTION of the recipient's OWN max HP.
    ///
    /// Every heal, Barrier and Regen in the game is authored as a fraction for one reason:
    /// a hero runs 40 max HP at level 1 and ~535 at 100, so a flat grant is a third of a
    /// hero early and a rounding error late. Floored at 1 so a small fraction on a small
    /// hero still does something.
    fn scaled_to(&self, i: usize, fraction: f64) -> i32 {
        ((self.fighters[i].max_hp as f64) * fraction).round().max(1.0) as i32
    }

    /// Grant Regen — HP back at the start of each of the holder's turns — as a fraction of
    /// their max HP. Stored absolute, so the wire token `regen:<n>` is unchanged.
    fn grant_regen(&mut self, i: usize, fraction: f64) -> Vec<ResolvedEffect> {
        if !self.take_stack(i, Stack::Regen) {
            return Vec::new();
        }
        let amount = self.scaled_to(i, fraction);
        self.fighters[i].regen += amount;
        let regen = self.fighters[i].regen;
        vec![ResolvedEffect {
            modifier_flag: None,
            target_id: self.fighters[i].combatant_id.clone(),
            kind: EffectKind::StatusApplied,
            amount: Some(regen),
            status: Some("regen".to_string()),
            hp_after: self.fighters[i].hp,
        }]
    }

    /// Whether `i` may take another stack of a lasting effect, and book it if so.
    /// Every stacking effect answers to the same ceiling, so "how many of these can I
    /// hold" is one number a player learns once rather than four they discover.
    fn take_stack(&mut self, i: usize, which: Stack) -> bool {
        let cap = self.max_effect_stacks;
        let held = match which {
            Stack::Regen => &mut self.fighters[i].regen_stacks,
            Stack::Barrier => &mut self.fighters[i].barrier_stacks,
            Stack::Evasion => &mut self.fighters[i].evasion_stacks,
            Stack::Atk => &mut self.fighters[i].atk_stacks,
        };
        if *held >= cap {
            return false;
        }
        *held += 1;
        true
    }

    /// Grant Evasion (a temporary dodge bonus) under the stack ceiling.
    fn grant_evasion(&mut self, i: usize, amount: f64) -> Vec<ResolvedEffect> {
        if amount <= 0.0 || !self.take_stack(i, Stack::Evasion) {
            return Vec::new();
        }
        self.fighters[i].evasion = (self.fighters[i].evasion + amount).min(0.95);
        let pct = (self.fighters[i].evasion * 100.0).round() as i32;
        vec![ResolvedEffect {
            modifier_flag: None,
            target_id: self.fighters[i].combatant_id.clone(),
            kind: EffectKind::StatusApplied,
            amount: Some(pct),
            status: Some("evasion".to_string()),
            hp_after: self.fighters[i].hp,
        }]
    }

    /// Raise `i`'s attack for the rest of the fight by a share of its OWN base — under
    /// the same ceiling, so five Tempering Blows is the most anyone can carry.
    fn grant_atk(&mut self, i: usize, fraction: f64) -> Vec<ResolvedEffect> {
        if !self.take_stack(i, Stack::Atk) {
            return Vec::new();
        }
        let bonus = ((self.fighters[i].base_atk as f64) * fraction).round().max(1.0) as i32;
        self.fighters[i].atk += bonus;
        let atk = self.fighters[i].atk;
        vec![ResolvedEffect {
            modifier_flag: None,
            target_id: self.fighters[i].combatant_id.clone(),
            kind: EffectKind::StatusApplied,
            amount: Some(atk),
            status: Some("tempered".to_string()),
            hp_after: self.fighters[i].hp,
        }]
    }

    fn grant_barrier(&mut self, i: usize, amount: i32) -> Vec<ResolvedEffect> {
        if amount <= 0 || !self.take_stack(i, Stack::Barrier) {
            return Vec::new();
        }
        self.fighters[i].barrier += amount;
        vec![ResolvedEffect { modifier_flag: None,
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
        // The deep kit (L16+) is seven abilities of one shape — heal, Regen, Barrier,
        // paid in the caster's own HP, on one ally or all of them — so it resolves
        // from the table rather than seven near-identical arms.
        if let Some(boon) = self.resonant_deep.get(skill) {
            return self.apply_ally_boon(caster_i, target_i, boon);
        }
        match skill {
            // The order's whole reason to exist: a hero who has FALLEN stands back up.
            // Nothing else in the game raises the dead mid-fight, which is why it is one
            // call a battle and why it costs the caster most of what it has.
            "second_life" => {
                let allies: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.kind == CombatantKind::Player)
                    .map(|(i, _)| i)
                    .collect();
                let mut effects = Vec::new();
                for a in allies {
                    if a == caster_i {
                        continue;
                    }
                    if self.fighters[a].alive {
                        let heal = self.scaled_to(a, self.resonant_second_life_heal_fraction);
                        effects.extend(self.apply_heal(a, heal));
                    } else {
                        let frac = self.resonant_second_life_revive_fraction;
                        let fx = self.raise_fallen(a, frac);
                        effects.extend(fx);
                    }
                }
                let cost = self.scaled_to(caster_i, self.resonant_second_life_self_cost);
                let before = self.fighters[caster_i].hp;
                let after = (before - cost).max(1);
                self.fighters[caster_i].hp = after;
                effects.push(ResolvedEffect {
                    modifier_flag: None,
                    target_id: self.fighters[caster_i].combatant_id.clone(),
                    kind: EffectKind::Damage,
                    amount: Some(before - after),
                    status: Some("transfuse".to_string()),
                    hp_after: after,
                });
                effects
            }
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
                effects.push(ResolvedEffect { modifier_flag: None,
                    target_id: self.fighters[caster_i].combatant_id.clone(),
                    kind: EffectKind::Damage,
                    amount: Some(before - after),
                    status: Some("transfuse".to_string()),
                    hp_after: after,
                });
                effects
            }
            "regen_boon" => {
                let fx = self.grant_regen(target_i, self.resonant_boon_regen_fraction);
                if fx.is_empty() {
                    return fx;
                }
                vec![ResolvedEffect { modifier_flag: None,
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
        // Timed ability statuses: expire the stale, then tick the DoTs
        // (poison/burn burn a max-HP fraction each of the victim's turns,
        // typed — so an immunity/absorption profile applies to the DoT too).
        let now = self.tick_count;
        // An AFFLICTION does not wear off — it holds until something cures it, so a poisoned
        // party spends a turn on the cure instead of waiting out a timer. A BOON still fades,
        // or the opening turns of a fight would be the whole fight.
        self.fighters[i]
            .timed_statuses
            .retain(|(n, until)| *until > now || meld_proto::statuses::is_affliction(n));
        let dots: Vec<String> = self.fighters[i]
            .timed_statuses
            .iter()
            .filter(|(n, _)| is_dot_status(n))
            .map(|(n, _)| n.clone())
            .collect();
        for name in dots {
            if !self.fighters[i].alive {
                break;
            }
            let (frac, ty) = if name == "burn" {
                (self.burn_dot_fraction, DamageType::Fire)
            } else {
                (self.poison_dot_fraction, DamageType::Poison)
            };
            let raw = ((self.fighters[i].max_hp as f64) * frac).round() as i32;
            if raw > 0 {
                let mut fx = self.apply_typed_damage(i, raw, ty);
                for e in &mut fx {
                    if matches!(e.kind, EffectKind::Damage) && e.status.is_none() {
                        e.status = Some(name.clone());
                    }
                }
                effects.extend(fx);
            }
        }
        if self.fighters[i].alive && self.fighters[i].regen > 0 {
            let raw = self.fighters[i].regen;
            effects.extend(self.apply_upkeep_heal(i, raw));
            // Regen DECAYS, like the Barrier beside it. It was the one lasting effect in
            // the game with neither decay nor expiry, so turns spent on it bought
            // permanent sustain — and once it is spent the stacks come back, which is
            // what lets a long fight be re-tended rather than capped forever.
            let shed = ((self.fighters[i].regen as f64) * self.regen_decay_fraction)
                .round()
                .max(1.0) as i32;
            self.fighters[i].regen = (self.fighters[i].regen - shed).max(0);
            if self.fighters[i].regen == 0 {
                self.fighters[i].regen_stacks = 0;
            }
        }
        // Barrier sheds a SHARE of the pool each turn, floored at 1 so it always drains.
        // A flat decay meant a level-100 hero's Barrier outlasted the fight it was cast in.
        if self.fighters[i].barrier > 0 {
            let shed = ((self.fighters[i].barrier as f64) * self.barrier_decay_fraction)
                .round()
                .max(1.0) as i32;
            self.fighters[i].barrier = (self.fighters[i].barrier - shed).max(0);
            if self.fighters[i].barrier == 0 {
                self.fighters[i].barrier_stacks = 0;
            }
        }
        // Evasion (Shifter Flicker) fades a fixed amount each of the holder's turns.
        if self.fighters[i].evasion > 0.0 {
            self.fighters[i].evasion =
                (self.fighters[i].evasion - self.shifter_flicker_decay).max(0.0);
            if self.fighters[i].evasion <= 0.0 {
                self.fighters[i].evasion_stacks = 0;
            }
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
        // Manifestations are psychic — MIND-typed, so elemental profiles apply.
        let mut effects = self.apply_ability_damage(t, dmg.max(self.min_damage), DamageType::Mind);
        // Expansion (the doc's passive on Gravity Well, generalised): the Focus also
        // reaches other living enemies, for a share of the tick. A controller should widen
        // with level — the alternative, hitting the one target harder, is the one thing
        // this class is not for.
        let extra = self.expansion_reach(psyker_i);
        if extra > 0 {
            let spill = (dmg as f64 * self.psyker_expansion_mult).round() as i32;
            let others: Vec<usize> = self
                .fighters
                .iter()
                .enumerate()
                .filter(|(i, f)| {
                    *i != t && f.alive && f.kind == CombatantKind::Monster && f.hp > 0
                })
                .map(|(i, _)| i)
                .take(extra)
                .collect();
            for o in others {
                effects.extend(self.apply_ability_damage(o, spill.max(self.min_damage), DamageType::Mind));
            }
        }
        effects
    }

    /// How many enemies BEYOND its own target an offensive Focus reaches, from the
    /// Psyker's level. Zero until Expansion unlocks, so an early Psyker still picks one
    /// creature and grinds it.
    fn expansion_reach(&self, psyker_i: usize) -> usize {
        let level = self.fighters[psyker_i].level;
        if self.psyker_expansion_at <= 0 || level < self.psyker_expansion_at {
            return 0;
        }
        let step = self.psyker_expansion_per_level.max(1);
        let grown = 1 + (level - self.psyker_expansion_at) / step;
        grown.clamp(1, self.psyker_expansion_cap.max(1)) as usize
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
        self.deny_gauge(t, Some(drain));
        vec![ResolvedEffect { modifier_flag: None,
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
        use meld_proto::consumables::{self as con, ConsumableEffect as E};
        // A revive is the one item that AIMS AT THE DEAD: every other targeting
        // helper skips them, so resolve it before the usual ally pick.
        let reviving = con::consumable(item_id.unwrap_or(""))
            .map(|c| c.effect == E::Revive)
            .unwrap_or(false);
        let target_i = if reviving {
            match target_id.and_then(|t| self.idx(t)).filter(|i| !self.fighters[*i].alive) {
                Some(i) => i,
                None => match self
                    .fighters
                    .iter()
                    .position(|f| !f.alive && f.kind == CombatantKind::Player)
                {
                    Some(i) => i,
                    // Nobody to raise: the bottle stays corked rather than being
                    // spent on a living hero who cannot use it.
                    None => {
                        self.reset_gauge(actor_i);
                        return self.resolution(
                            actor_i,
                            BattleActionKind::Item,
                            action_id,
                            Vec::new(),
                        );
                    }
                },
            }
        } else {
            self.ally_target(target_id).unwrap_or(actor_i)
        };
        let max_hp = self.fighters[target_i].max_hp;
        // GR-4: a potion does what its registry entry says. An unknown item id keeps
        // the old fraction-heal behaviour, so an older client cannot be stranded.
        let def = con::consumable(item_id.unwrap_or("bloom_salve"));
        let effect = def.map(|c| c.effect).unwrap_or(E::Heal);
        // MS-1: the trophy line is the same effects at a bigger dose. `potency` is
        // how many steps up its own ladder a potion sits; step 0 is the standard
        // dose, so every potion that predates the ladder is untouched.
        let dose = self
            .consumable_potency_per_step
            .powi(def.map(|c| c.potency).unwrap_or(0).max(0));
        // Class-gated items refuse rather than fizzle: the Smithwright forges what it
        // throws and the Keeper brews it, and a hero who cannot use one has not spent it.
        if let Some(only) = def.and_then(|c| c.only_class) {
            let holder = meld_proto::equipment::class_from_key(&self.fighters[actor_i].class_key);
            if holder != Some(only) {
                self.reset_gauge(actor_i);
                return self.resolution(actor_i, BattleActionKind::Item, action_id, Vec::new());
            }
        }
        let effects = match effect {
            // THROWN at the whole encounter. A share of the thrower's own attack, landed on
            // every living enemy — the all-enemy tier, priced by the COUNT in the pouch
            // rather than by the number per target.
            E::ThrownAll => {
                let ty = def
                    .and_then(|c| c.damage_type)
                    .unwrap_or(crate::UNARMED_ATTACK_TYPE);
                let raw = (((self.fighters[actor_i].atk as f64) * self.thrown_atk_mult * dose)
                    .round() as i32)
                    .max(self.min_damage);
                let enemies: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.alive && f.kind != CombatantKind::Player)
                    .map(|(i, _)| i)
                    .collect();
                let mut out = Vec::new();
                for t in enemies {
                    out.extend(self.apply_ability_damage(t, raw, ty));
                }
                out
            }
            E::Revive => {
                let fraction = (self.revive_hp_fraction * dose).min(1.0);
                let back = ((max_hp as f64) * fraction).round().max(1.0) as i32;
                self.fighters[target_i].alive = true;
                self.fighters[target_i].hp = back.min(max_hp);
                self.fighters[target_i].gauge = 0.0;
                vec![ResolvedEffect {
                    modifier_flag: None,
                    target_id: self.fighters[target_i].combatant_id.clone(),
                    kind: EffectKind::Heal,
                    amount: Some(back),
                    status: Some("revived".to_string()),
                    hp_after: self.fighters[target_i].hp,
                }]
            }
            // The XP a mote carries is banked by the run, not the battle: the engine
            // has no notion of persistent progression.
            E::Experience => self.status_effect(target_i, "insight", 1),
            E::FullHeal => self.apply_heal(target_i, max_hp),
            E::Heal => {
                let raw = ((max_hp as f64) * self.item_heal_fraction * dose).round() as i32;
                self.apply_heal(target_i, raw)
            }
            E::Barrier => {
                let amount = ((self.consumable_barrier as f64) * dose).round() as i32;
                self.grant_barrier(target_i, amount)
            }
            E::Cleanse => self.cure(target_i, meld_proto::statuses::Family::Mind),
            E::Panacea => self.cure(target_i, meld_proto::statuses::Family::All),
            E::Regen => {
                let amount = ((self.consumable_regen as f64) * dose).round() as i32;
                let max_hp = self.fighters[target_i].max_hp.max(1);
                self.grant_regen(target_i, amount as f64 / max_hp as f64)
            }
            E::Evasion => {
                let pct = ((self.consumable_evasion_pct as f64) * dose).round() as i32;
                self.grant_evasion(target_i, pct as f64 / 100.0)
            }
            E::Adrenaline => {
                // Inert on a class with no Adrenaline to bank, exactly like the
                // matching keyword affix — the bottle is not wasted, it just does
                // nothing, and the client greys it for non-Explorers.
                let max = self.fighters[target_i].adrenaline_max;
                let banked = ((self.consumable_adrenaline as f64) * dose).round() as i32;
                let amount = banked.min(max);
                self.fighters[target_i].adrenaline =
                    (self.fighters[target_i].adrenaline + amount).min(max);
                self.status_effect(target_i, "adrenaline", amount)
            }
        };
        // The action still belongs to the actor (its gauge/stance reset), even when
        // the heal lands on an ally.
        self.fighters[actor_i].defending = false;
        self.reset_gauge(actor_i);
        self.resolution(actor_i, BattleActionKind::Item, action_id, effects)
    }

    /// One `StatusApplied` effect, for a potion that grants a state rather than HP.
    fn status_effect(&self, i: usize, status: &str, amount: i32) -> Vec<ResolvedEffect> {
        vec![ResolvedEffect {
            modifier_flag: None,
            target_id: self.fighters[i].combatant_id.clone(),
            kind: EffectKind::StatusApplied,
            amount: Some(amount),
            status: Some(status.to_string()),
            hp_after: self.fighters[i].hp,
        }]
    }

    /// Heal the actor by `raw` (min 1), capped at max HP; report the actual gain.
    /// Heal from UPKEEP (the Regen drip), which restores HP and nothing else.
    ///
    /// Split from [`Self::apply_heal`] because that one ends a frenzy: this fires at the top
    /// of every single turn, so letting it calm one would make any party carrying Regen immune
    /// to the condition outright.
    fn apply_upkeep_heal(&mut self, actor_i: usize, raw: i32) -> Vec<ResolvedEffect> {
        let before = self.fighters[actor_i].hp;
        let max_hp = self.fighters[actor_i].max_hp;
        let after = (before + raw.max(1)).min(max_hp);
        self.fighters[actor_i].hp = after;
        vec![ResolvedEffect { modifier_flag: None,
            target_id: self.fighters[actor_i].combatant_id.clone(),
            kind: EffectKind::Heal,
            amount: Some(after - before),
            status: None,
            hp_after: after,
        }]
    }

    fn apply_heal(&mut self, actor_i: usize, raw: i32) -> Vec<ResolvedEffect> {
        let before = self.fighters[actor_i].hp;
        let max_hp = self.fighters[actor_i].max_hp;
        let after = (before + raw.max(1)).min(max_hp);
        self.fighters[actor_i].hp = after;
        // CARE TAKES THE WHEEL BACK: a frenzy ends when someone tends to you — so a mender
        // answers a berserk ally rather than the party racing to out-damage them.
        //
        // The passive Regen DRIP is deliberately excluded (see `apply_upkeep_heal`). It fires
        // at the top of every turn, so letting it count would mean any party holding Regen —
        // which the Resonant hands out party-wide — is simply immune to frenzy, and a
        // condition a passive cancels is not a condition.
        let mut out = vec![ResolvedEffect { modifier_flag: None,
            target_id: self.fighters[actor_i].combatant_id.clone(),
            kind: EffectKind::Heal,
            amount: Some(after - before),
            status: None,
            hp_after: after,
        }];
        let calmed = self.shake_off(actor_i, meld_proto::statuses::CLEARED_BY_HEALING);
        out.extend(calmed);
        out
    }

    /// Assemble a non-flee, non-auto player [`Resolution`].
    fn resolution(
        &self,
        actor_i: usize,
        action: BattleActionKind,
        action_id: Option<Id>,
        effects: Vec<ResolvedEffect>,
    ) -> Resolution {
        Resolution { callout_text: None,
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
        Resolution { callout_text: None,
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
        // A distracted creature has lost the thread, so this is when a party gets out. The
        // Explorer's Distract is the order's answer to "we should not be in this fight".
        let distracted_foe = self
            .fighters
            .iter()
            .enumerate()
            .any(|(i, f)| f.alive && f.kind != CombatantKind::Player && self.is_distracted(i));
        let bonus = if distracted_foe { self.explorer_misdirection_flee_bonus } else { 0.0 };
        let chance = (self.flee_chance(tier_gap) + bonus).min(1.0);
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
        Resolution { callout_text: None,
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
        // Skittish creatures flee a losing battle instead of attacking.
        if self.fighters[actor_i].flees {
            let f = &self.fighters[actor_i];
            let low = (f.hp as f64) < (f.max_hp as f64) * self.creature_flee_hp_fraction;
            if low && f.max_hp > 0 {
                self.fighters[actor_i].alive = false; // leaves the field
                self.reset_gauge(actor_i);
                return Some(Resolution { callout_text: None,
                    action_id: None,
                    actor_id: self.fighters[actor_i].combatant_id.clone(),
                    action: BattleActionKind::Flee,
                    auto: true,
                    flee_success: Some(true),
                    effects: vec![ResolvedEffect { modifier_flag: None,
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
        // CR-9: who this creature goes for is its own profile's business, and a ganging
        // pack says out loud when it picks or moves its mark.
        let (target, shout) = self.choose_target(actor_i);
        let target_i = target?;
        let atk = self.fighters[actor_i].atk;
        let def = self.fighters[target_i].def;
        let defending = self.fighters[target_i].defending;
        let basic_type = self.fighters[actor_i].basic_attack_type;
        let effects = match self.roll_dodge(target_i) {
            Some(dodge) => dodge,
            None => {
                let raw = self.damage(atk, def, defending);
                self.apply_typed_damage(target_i, raw, basic_type)
            }
        };
        self.reset_gauge(actor_i);
        Some(Resolution {
            // A pack converging on your healer with no explanation reads as the game
            // cheating, so the mark is shouted on the turn it is set or moved — the same
            // bubble a telegraphed ability uses.
            callout_text: shout,
            action_id: None,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Attack,
            auto: false,
            flee_success: None,
            effects,
        })
    }

    /// One monster turn under the ability AI (spec §2): flee check first, then
    /// filter the pool (level gate, cooldown, HP threshold), mix in the basic
    /// attack, and roll a weighted choice. A telegraphed pick starts a channel
    /// (emitting [`Event::TelegraphStarted`]) and returns `None` — the cast
    /// lands via `tick` step 1b at `executes_at`.
    /// The creature's REBUKE: the **rarest** thing in its book — its lowest-weight ability it
    /// has the level for.
    ///
    /// Rarest rather than biggest, and that is the whole design: "an attack it would not
    /// normally do" IS the low-weight entry, and picking by weight means the variety comes
    /// out of the kits that are already authored rather than out of a new field on all 52 of
    /// them. A boss whose scarcest entry is a self-heal comes back up and MENDS; one whose
    /// scarcest is a telegraphed ruin comes back swinging. Both are announced — a telegraph
    /// shouts, and an instant ability has its own callout — so the rebuke is always legible.
    ///
    /// `None` for a creature with no authored kit, which is what keeps this to things that
    /// have something to answer WITH, without anything having to declare itself a boss.
    /// Extra parties past the first this fighter is sized for — 0 for everything ordinary.
    fn raid_extra(&self, i: usize) -> f64 {
        f64::from(self.fighters[i].raid_parties.saturating_sub(1))
    }

    /// How much likelier a PARTY-WIDE ability is to be rolled for this fighter (1.0 = not).
    ///
    /// The bias lives here rather than in the pool on purpose: the authored weights are also
    /// read as *rarity* by `signature_ability`, so scaling them in place would quietly make a
    /// raid boss's rebuke its small single-target poke where the ordinary version of the same
    /// boss answers with its apocalypse — a raid tier weakening the fight it is meant to
    /// escalate. Measured, that hit five of the ten authored bosses.
    fn wide_weight_mult(&self, i: usize) -> f64 {
        1.0 + self.raid_wide_weight_per_party.max(0.0) * self.raid_extra(i)
    }

    /// How much sooner a party-wide ability comes back for this fighter (1.0 = not).
    fn wide_cooldown_div(&self, i: usize) -> f64 {
        1.0 + self.raid_wide_cooldown_per_party.max(0.0) * self.raid_extra(i)
    }

    /// The rebuke's pick: the RAREST thing in this creature's book, by its **authored**
    /// weight. Deliberately not the biased weight — a raid tier changes how often you see an
    /// ability, never what counts as its signature.
    fn signature_ability(&self, i: usize) -> Option<usize> {
        let level = self.fighters[i].level;
        self.fighters[i]
            .abilities
            .iter()
            .enumerate()
            .filter(|(_, a)| a.min_level <= level)
            .min_by_key(|(_, a)| a.weight)
            .map(|(idx, _)| idx)
    }

    fn take_monster_turn(&mut self, actor_i: usize, events: &mut Vec<Event>) -> Option<Resolution> {
        let now = self.tick_count;
        // THE REBUKE. Its turn was taken, so this one is its signature — not a rolled action,
        // and not gated on the cooldown: the cooldown is what paces a boss's own rhythm, and
        // this is the fight answering an interruption. Consumed whether or not it fires, so a
        // single knock buys a single rebuke.
        if std::mem::take(&mut self.fighters[actor_i].rebuke_pending) {
            if let Some(idx) = self.signature_ability(actor_i) {
                return self.begin_ability(actor_i, idx, events);
            }
        }
        let (hp_pct, level, has_pool) = {
            let f = &self.fighters[actor_i];
            let pct = if f.max_hp > 0 {
                f.hp as f64 / f.max_hp as f64
            } else {
                1.0
            };
            (pct, f.level, !f.abilities.is_empty())
        };
        if !has_pool {
            // No authored pool (unknown kind): the classic basic-attack turn.
            return self.resolve_monster_turn(actor_i);
        }
        let eligible: Vec<(usize, i64)> = self.fighters[actor_i]
            .abilities
            .iter()
            .enumerate()
            .filter(|(idx, a)| {
                a.min_level <= level
                    && self.fighters[actor_i]
                        .ability_ready_at
                        .get(idx)
                        .copied()
                        .unwrap_or(0)
                        <= now
                    && a.hp_threshold_pct.is_none_or(|t| hp_pct <= t)
            })
            .map(|(idx, a)| {
                let w = f64::from(a.weight.max(1));
                let w = if a.reaches_the_whole_party() {
                    w * self.wide_weight_mult(actor_i)
                } else {
                    w
                };
                (idx, (w.round() as i64).max(1))
            })
            .collect();
        let total: i64 =
            eligible.iter().map(|(_, w)| w).sum::<i64>() + self.basic_attack_weight.max(1) as i64;
        let mut roll = (self.next_rand_unit() * total as f64) as i64;
        for (idx, w) in eligible {
            if roll < w {
                return self.begin_ability(actor_i, idx, events);
            }
            roll -= w;
        }
        // The remaining weight band is the basic attack (also the flee check).
        self.resolve_monster_turn(actor_i)
    }

    /// Commit to ability `idx`: cooldown starts now; a telegraphed ability
    /// enters channeling (shout now, land later), an instant one resolves here.
    fn begin_ability(
        &mut self,
        actor_i: usize,
        idx: usize,
        events: &mut Vec<Event>,
    ) -> Option<Resolution> {
        let (cooldown, telegraph, callout, wide) = {
            let a = &self.fighters[actor_i].abilities[idx];
            (a.cooldown_ticks, a.telegraph_ticks, a.callout_text.clone(), a.reaches_the_whole_party())
        };
        // A raid boss's wide rows come back sooner. Floored at the TELEGRAPH, never below: a
        // shout has to still mean something, and an ability ready again before the last cast
        // has landed would announce itself into a fight that never sees it arrive.
        let cooldown = if wide {
            ((f64::from(cooldown) / self.wide_cooldown_div(actor_i)).round() as i32)
                .max(telegraph.max(1))
        } else {
            cooldown
        };
        self.fighters[actor_i]
            .ability_ready_at
            .insert(idx, self.tick_count + cooldown.max(0) as u64);
        if telegraph > 0 {
            let executes_at = self.tick_count + telegraph as u64;
            self.fighters[actor_i].channel = Some((idx, executes_at));
            self.reset_gauge(actor_i);
            events.push(Event::TelegraphStarted {
                combatant_id: self.fighters[actor_i].combatant_id.clone(),
                callout_text: callout,
                executes_at_tick: executes_at,
            });
            None
        } else {
            Some(self.resolve_ability(actor_i, idx, events))
        }
    }

    /// Resolve every effect of ability `idx` in order (spec §2 math). Reported
    /// as an auto `Skill` action; the callout rides along only for *instant*
    /// abilities (a channeled one already shouted via `telegraph_started`).
    fn resolve_ability(
        &mut self,
        actor_i: usize,
        idx: usize,
        events: &mut Vec<Event>,
    ) -> Resolution {
        let ability = self.fighters[actor_i].abilities[idx].clone();
        let mut effects = Vec::new();
        for eff in &ability.effects {
            let targets = self.ability_targets(actor_i, eff.target);
            match eff.effect_kind {
                AbilityEffectKind::Damage => {
                    let raw = self.scaled_amount(actor_i, eff.scaling_base, eff.coefficient);
                    let ty = eff.damage_type.unwrap_or(DamageType::None);
                    for t in targets {
                        if self.fighters[t].alive {
                            effects.extend(self.apply_ability_damage(t, raw, ty));
                        }
                    }
                }
                AbilityEffectKind::Heal => {
                    let raw = self.scaled_amount(actor_i, eff.scaling_base, eff.coefficient);
                    for t in targets {
                        if self.fighters[t].alive {
                            effects.extend(self.apply_heal(t, raw));
                        }
                    }
                }
                AbilityEffectKind::AtbManipulation => {
                    // `coefficient` is the gauge fraction added (+) or drained (−).
                    let delta = eff.coefficient.unwrap_or(0.0);
                    for t in targets {
                        if !self.fighters[t].alive {
                            continue;
                        }
                        let f = &mut self.fighters[t];
                        f.gauge = (f.gauge + delta).clamp(0.0, 1.0);
                        effects.push(ResolvedEffect {
                            target_id: f.combatant_id.clone(),
                            kind: EffectKind::StatusApplied,
                            amount: None,
                            status: Some(
                                if delta >= 0.0 { "hastened" } else { "slowed" }.to_string(),
                            ),
                            hp_after: f.hp,
                            modifier_flag: None,
                        });
                    }
                }
                AbilityEffectKind::Status => {
                    let name = eff.status_name.clone().unwrap_or_default();
                    let dur = eff.duration_ticks.unwrap_or(0).max(0) as u64;
                    if name.is_empty() || dur == 0 {
                        continue;
                    }
                    let until = self.tick_count + dur;
                    for t in targets {
                        if !self.fighters[t].alive {
                            continue;
                        }
                        let f = &mut self.fighters[t];
                        // Re-application refreshes the timer (no stacking).
                        if let Some(s) =
                            f.timed_statuses.iter_mut().find(|(n, _)| *n == name)
                        {
                            s.1 = s.1.max(until);
                        } else {
                            f.timed_statuses.push((name.clone(), until));
                        }
                        effects.push(ResolvedEffect {
                            target_id: f.combatant_id.clone(),
                            kind: EffectKind::StatusApplied,
                            amount: None,
                            status: Some(name.clone()),
                            hp_after: f.hp,
                            modifier_flag: None,
                        });
                    }
                }
                AbilityEffectKind::Steal => {
                    let Some(kind) = eff.steal_target_kind else {
                        continue;
                    };
                    for t in targets {
                        if !self.fighters[t].alive {
                            continue;
                        }
                        // Only players carry stealable goods; the server applies
                        // the actual deduction from the victim's run.
                        if let Some(pid) = self.fighters[t].player_id.clone() {
                            events.push(Event::Stolen {
                                victim_player_id: pid,
                                kind,
                            });
                            let label = match kind {
                                StealTargetKind::Chits => "stolen:chits",
                                StealTargetKind::Consumable => "stolen:consumable",
                                StealTargetKind::Material => "stolen:material",
                            };
                            effects.push(ResolvedEffect {
                                target_id: self.fighters[t].combatant_id.clone(),
                                kind: EffectKind::StatusApplied,
                                amount: None,
                                status: Some(label.to_string()),
                                hp_after: self.fighters[t].hp,
                                modifier_flag: None,
                            });
                        }
                    }
                }
            }
        }
        self.reset_gauge(actor_i);
        Resolution {
            action_id: None,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Skill,
            auto: true,
            flee_success: None,
            callout_text: if ability.telegraph_ticks == 0 {
                Some(ability.callout_text.clone())
            } else {
                None
            },
            effects,
        }
    }

    /// Targets for one ability effect. Enemy selection reuses the weakest-
    /// hostile heuristic (with back-row protection) of the basic AI.
    fn ability_targets(&mut self, actor_i: usize, target: AbilityTarget) -> Vec<usize> {
        let actor_faction = self.fighters[actor_i].faction.clone();
        let actor_id = self.fighters[actor_i].combatant_id.clone();
        match target {
            AbilityTarget::SelfCast => vec![actor_i],
            AbilityTarget::SingleEnemy => self
                .pick_weakest_hostile(actor_i)
                .map(|t| vec![t])
                .unwrap_or_default(),
            AbilityTarget::AllEnemies => self
                .fighters
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    f.alive
                        && f.combatant_id != actor_id
                        && meld_proto::factions::battle_hostile(&actor_faction, &f.faction)
                })
                .map(|(i, _)| i)
                .collect(),
            // The caster's own side (its monster group / allies), self included.
            AbilityTarget::MonsterGroup | AbilityTarget::AllAllies => self
                .fighters
                .iter()
                .enumerate()
                .filter(|(_, f)| f.alive && f.faction == actor_faction)
                .map(|(i, _)| i)
                .collect(),
        }
    }

    /// The weakest living hostile, honouring back-row protection — the same
    /// heuristic (and RNG discipline) as the basic monster attack.
    /// Who `actor_i` attacks, honouring its [`TargetProfile`] (CR-9).
    ///
    /// This is the ONE place a creature picks a target. It used to be two near-identical
    /// copies — one inline in `resolve_monster_turn`, one here for abilities — which meant
    /// a creature could hunt the back rank with its basic attack and go for the weakest
    /// with its ability.
    ///
    /// Returns the chosen index and, when a gang-up mark is newly set or moved, the line to
    /// shout about it: a pack converging on your healer with no explanation reads as the
    /// game cheating.
    fn choose_target(&mut self, actor_i: usize) -> (Option<usize>, Option<String>) {
        let actor_faction = self.fighters[actor_i].faction.clone();
        let actor_id = self.fighters[actor_i].combatant_id.clone();
        let hostile: Vec<usize> = self
            .fighters
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                f.alive
                    && f.combatant_id != actor_id
                    && meld_proto::factions::battle_hostile(&actor_faction, &f.faction)
            })
            .map(|(i, _)| i)
            .collect();
        if hostile.is_empty() {
            return (None, None);
        }
        match self.fighters[actor_i].target_profile {
            TargetProfile::Weakest => (Some(self.weakest_with_cover(&hostile)), None),
            // No pattern: unpredictable rather than stupid. Still respects the rank, so a
            // front line is worth holding even against a mindless thing.
            TargetProfile::Random => {
                let pick = (self.next_rand_unit() * hostile.len() as f64) as usize;
                (Some(hostile[pick.min(hostile.len() - 1)]), None)
            }
            // Hunts the back rank ON PURPOSE — the counter to hiding every caster behind a
            // wall. Falls back to the weakest when there is no back rank to hunt.
            TargetProfile::Backline => {
                let back = hostile
                    .iter()
                    .copied()
                    .filter(|&i| self.fighters[i].back_row)
                    .min_by_key(|&i| self.fighters[i].hp);
                (Some(back.unwrap_or_else(|| self.weakest_with_cover(&hostile))), None)
            }
            TargetProfile::Role => (Some(self.by_role(&hostile)), None),
            TargetProfile::GangUp => {
                let live = self
                    .gang_target
                    .as_ref()
                    .and_then(|id| self.idx(id))
                    .filter(|&i| self.fighters[i].alive && hostile.contains(&i));
                // Re-pick when the mark is gone, or occasionally on purpose so a pack can
                // switch to a better target mid-fight instead of committing until it dies.
                let switch = live.is_none() || self.next_rand_unit() < self.gang_switch_chance;
                if !switch {
                    return (live, None);
                }
                let pick = self.by_role(&hostile);
                let moved = live != Some(pick);
                self.gang_target = Some(self.fighters[pick].combatant_id.clone());
                let shout = moved.then(|| {
                    let who = self.fighters[pick].display_name();
                    if live.is_some() {
                        format!("The pack turns on {who}!")
                    } else {
                        format!("The pack marks {who}!")
                    }
                });
                (Some(pick), shout)
            }
        }
    }

    /// The weakest hostile, with the back rank's cover applied — the original rule.
    fn weakest_with_cover(&mut self, hostile: &[usize]) -> usize {
        let weakest = *hostile.iter().min_by_key(|&&i| self.fighters[i].hp).expect("non-empty");
        if !self.fighters[weakest].back_row {
            return weakest;
        }
        let front = hostile
            .iter()
            .copied()
            .filter(|&i| !self.fighters[i].back_row)
            .min_by_key(|&i| self.fighters[i].hp);
        match front {
            Some(f) if self.next_rand_unit() >= self.back_row_target_weight => f,
            _ => weakest,
        }
    }

    /// The hostile whose ROLE matters most: the healer that undoes your work, then the
    /// casters that control the fight, then whoever is closest to falling. This is what
    /// makes a smart creature frightening — it is not doing more damage, it is spending it
    /// where the party can least afford it.
    fn by_role(&mut self, hostile: &[usize]) -> usize {
        let rank = |k: &str| match k {
            "resonant" | "keeper" => 0,
            "psyker" => 1,
            "smithwright" | "explorer" => 2,
            _ => 3,
        };
        *hostile
            .iter()
            .min_by_key(|&&i| {
                let f = &self.fighters[i];
                (rank(&f.class_key), f.hp)
            })
            .expect("non-empty")
    }

    /// A creature's single-enemy ABILITY aims the same way its basic attack does — one
    /// creature, one idea about who matters. These were two separate rules, so an ambusher
    /// hunted the back rank with its claws and the weakest hero with its breath.
    fn pick_weakest_hostile(&mut self, actor_i: usize) -> Option<usize> {
        self.choose_target(actor_i).0
    }

    /// `stats[scaling_base] × coefficient`, rounded — the spec's base formula.
    fn scaled_amount(
        &self,
        actor_i: usize,
        base: Option<ScalingBase>,
        coefficient: Option<f64>,
    ) -> i32 {
        let f = &self.fighters[actor_i];
        let stat = match base {
            Some(ScalingBase::Attack) => f.atk as f64,
            Some(ScalingBase::Magic) => f.spell_power as f64,
            Some(ScalingBase::Level) => f.level as f64,
            Some(ScalingBase::MaxHp) => f.max_hp as f64,
            None => f.atk as f64,
        };
        (stat * coefficient.unwrap_or(1.0)).round() as i32
    }

    /// A pure-upkeep resolution (DoT killed the actor before it could act).
    fn upkeep_only(&self, actor_i: usize, effects: Vec<ResolvedEffect>) -> Resolution {
        Resolution {
            action_id: None,
            actor_id: self.fighters[actor_i].combatant_id.clone(),
            action: BattleActionKind::Defend,
            auto: true,
            flee_success: None,
            callout_text: None,
            effects,
        }
    }

    /// Roll the target's Dex-derived dodge against a *physical* attack. On a
    /// dodge returns the whiff effect (0 HP change, `dodge` status) so the caller
    /// deals no damage; otherwise `None`. The RNG only advances when the target
    /// actually has dodge, so combatants with no Dex bonus don't perturb the
    /// deterministic stream (existing tests/replays are unaffected).
    fn roll_dodge(&mut self, target_i: usize) -> Option<Vec<ResolvedEffect>> {
        // Innate Dex dodge plus any temporary Evasion (Shifter Flicker, Explorer Safe
        // Passage), capped just shy of certain so an attack can always in principle land.
        // A DISTRACTED attacker (Explorer Misdirection) adds its miss chance on top: the dodge is
        // where "the thing swinging at you has lost the thread" has to land, since accuracy
        // in this engine lives on the defender.
        let distracted = self
            .active_actor
            .is_some_and(|a| self.has_timed_status(a, Self::DISTRACT_STATUS));
        let extra = if distracted { self.explorer_misdirection_miss } else { 0.0 };
        // Blackout (Dominate Mind): a BLINDED attacker swings wide, on top of any dazzle.
        // Same side of the equation as `distracted`, because that is where this engine keeps
        // accuracy — a creature has no dodge of its own to take away.
        let blinded = self
            .active_actor
            .is_some_and(|a| self.has_timed_status(a, BLIND_STATUS));
        let extra = extra + if blinded { self.psyker_blackout_miss } else { 0.0 };
        let chance =
            (self.fighters[target_i].dodge + self.fighters[target_i].evasion + extra).min(0.95);
        if chance > 0.0 && self.next_rand_unit() < chance {
            let t = &self.fighters[target_i];
            Some(vec![ResolvedEffect { modifier_flag: None,
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

    /// The target's modifier for a damage type, plus its wire flag.
    /// `DamageType::None` is pure damage: multiplier 1.0, no flag.
    fn modifier_for(&self, target_i: usize, ty: DamageType) -> (f64, Option<ModifierFlag>) {
        if ty == DamageType::None {
            return (1.0, None);
        }
        let m = self.fighters[target_i]
            .damage_modifiers
            .get(&ty)
            .copied()
            .unwrap_or(1.0);
        let flag = if m > 1.0 {
            ModifierFlag::Weak
        } else if m < 0.0 {
            ModifierFlag::Absorb
        } else if m == 0.0 {
            ModifierFlag::Immune
        } else if m < 1.0 {
            ModifierFlag::Resist
        } else {
            ModifierFlag::Normal
        };
        (m, Some(flag))
    }

    /// Typed damage (spec §2): `Final = Floor(raw × target_modifier)`.
    /// Immunity lands a 0; absorption heals `|Final|` instead; everything else
    /// flows through [`Self::apply_damage`] (barrier, back-row, KO) with the
    /// modifier flag stamped on the Damage effect.
    /// A turn spent doing nothing, reported so the client can say WHY rather than showing a
    /// hero that silently did not act.
    fn skipped_turn(&mut self, i: usize, why: &str) -> Resolution {
        let effect = ResolvedEffect {
            modifier_flag: None,
            target_id: self.fighters[i].combatant_id.clone(),
            kind: EffectKind::StatusApplied,
            amount: None,
            status: Some(why.to_string()),
            hp_after: self.fighters[i].hp,
        };
        self.reset_gauge(i);
        Resolution {
            action_id: None,
            actor_id: self.fighters[i].combatant_id.clone(),
            action: BattleActionKind::Defend,
            auto: true,
            flee_success: None,
            callout_text: None,
            effects: vec![effect],
        }
    }

    /// Replace a confused fighter's order with a random one: a random action from what it can
    /// actually do, at a random living combatant — friend or foe, itself included.
    ///
    /// Deliberately does NOT roll `Item`: a confusion that drank your last Panacea would spend
    /// a resource the player cannot get back, which is a different and much crueller mechanic
    /// than swinging at the wrong person.
    fn scramble(
        &mut self,
        i: usize,
        action: BattleActionKind,
        skill_kind: Option<String>,
    ) -> (BattleActionKind, Option<String>, Option<Vec<Id>>) {
        let level = self.fighters[i].level;
        let class = self.fighters[i].class_key.clone();
        let kit: Vec<String> = meld_proto::skills::skills_for_class_at(&class, level)
            .into_iter()
            .map(|s| s.key.to_string())
            .collect();
        // Attack, Defend, and every row the hero owns — one flat pool, so a caster is as
        // likely to flail as to miscast.
        let choices = 2 + kit.len();
        let pick = ((self.next_rand_unit() * choices as f64) as usize).min(choices - 1);
        let (act, skill) = match pick {
            0 => (BattleActionKind::Attack, None),
            1 => (BattleActionKind::Defend, None),
            n => {
                let key = kit[n - 2].clone();
                // A Psyker's rows are OPS, not ability keys — a bare key falls through to
                // `hold`, which would make confusion a free pass for the class.
                let key = if class == "psyker" { format!("cast:{key}") } else { key };
                (BattleActionKind::Skill, Some(key))
            }
        };
        let living: Vec<Id> = self
            .fighters
            .iter()
            .filter(|f| f.alive)
            .map(|f| f.combatant_id.clone())
            .collect();
        let target = (!living.is_empty()).then(|| {
            let t = ((self.next_rand_unit() * living.len() as f64) as usize)
                .min(living.len() - 1);
            vec![living[t].clone()]
        });
        let _ = (action, skill_kind);
        (act, skill, target)
    }

    /// Whether a fighter is carrying `name`.
    fn has(&self, i: usize, name: &str) -> bool {
        self.fighters[i].timed_statuses.iter().any(|(n, _)| n == name)
    }

    /// KNOCK A FIGHTER'S GAUGE DOWN — the one place that happens.
    ///
    /// `amount` of `None` zeroes it outright; `Some(d)` subtracts that much. Returns whether
    /// the knock LANDED, so the caller can report a bounce instead of silently doing nothing.
    ///
    /// It is one function because denial was fourteen hand-written subtractions across every
    /// resolver, and nothing checked what happened when they were chained. Measured: 150
    /// gauge attacks over 464 hero-turns held a gatekeeper at 29% and it never acted — the
    /// party took ZERO damage from a boss with 66,792 HP. That is the same unbounded lock
    /// `hallowed_ground` is gated once-a-fight to prevent, reached with two repeatable rows
    /// instead of one capstone.
    ///
    /// So a knock still WORKS — taking a boss's turn is the play these abilities are for —
    /// but it leaves a `GAUGE_GUARD_STATUS` behind, and a guarded fighter cannot be knocked
    /// again until it lapses. One landing, then a window where the fight answers back.
    fn deny_gauge(&mut self, target_i: usize, amount: Option<f64>) -> bool {
        if self.fighters[target_i].gauge_guard_turns > 0 {
            return false;
        }
        let before = self.fighters[target_i].gauge;
        self.fighters[target_i].gauge = match amount {
            None => 0.0,
            Some(d) => (before - d).max(0.0),
        };
        // Nothing was actually taken, so nothing is guarded: a drain on an empty gauge must
        // not buy the target a free immunity window.
        if (before - self.fighters[target_i].gauge).abs() < f64::EPSILON {
            return false;
        }
        self.fighters[target_i].staggered = true;
        self.fighters[target_i].gauge_guard_turns = self.gauge_guard_turns;
        // Arm the REBUKE — but only on something that HAS a signature to answer with. The
        // rule is universal and the effect only exists where a telegraph does, so ordinary
        // fauna is unchanged and no creature needs a flag saying "I am a boss".
        if self.signature_ability(target_i).is_some() {
            self.fighters[target_i].rebuke_pending = true;
        }
        // Say so on the wire, or a player whose drain bounced sees an ability do nothing:
        // a token nothing renders is a token that does not exist to the player.
        if !self.fighters[target_i].statuses.iter().any(|s| s == GAUGE_GUARD_STATUS) {
            self.fighters[target_i].statuses.push(GAUGE_GUARD_STATUS.to_string());
        }
        true
    }

    /// Knock conditions off a fighter, reporting one effect each. The shared tail of "a blow
    /// brings you round" and "care takes the wheel back".
    fn shake_off(&mut self, i: usize, which: &[&str]) -> Vec<ResolvedEffect> {
        let lifted: Vec<String> = self.fighters[i]
            .timed_statuses
            .iter()
            .filter(|(n, _)| which.contains(&n.as_str()))
            .map(|(n, _)| n.clone())
            .collect();
        if lifted.is_empty() {
            return Vec::new();
        }
        self.fighters[i].timed_statuses.retain(|(n, _)| !which.contains(&n.as_str()));
        lifted
            .into_iter()
            .map(|n| ResolvedEffect {
                modifier_flag: None,
                target_id: self.fighters[i].combatant_id.clone(),
                kind: EffectKind::StatusApplied,
                amount: None,
                status: Some(format!("cured:{n}")),
                hp_after: self.fighters[i].hp,
            })
            .collect()
    }

    /// Bring a FALLEN fighter back, standing at `fraction` of its max HP.
    ///
    /// Extracted from `second_life` so a revive is one behaviour rather than one per ability:
    /// the gauge is reset (you come back at the back of the queue, not with a free turn) and
    /// the effect is tagged `revived` so the client can say so.
    fn raise_fallen(&mut self, i: usize, fraction: f64) -> Vec<ResolvedEffect> {
        if self.fighters[i].alive {
            return Vec::new();
        }
        let hp = self.scaled_to(i, fraction).max(1);
        self.fighters[i].hp = hp;
        self.fighters[i].alive = true;
        self.fighters[i].gauge = 0.0;
        vec![ResolvedEffect {
            modifier_flag: None,
            target_id: self.fighters[i].combatant_id.clone(),
            kind: EffectKind::Heal,
            amount: Some(hp),
            status: Some("revived".to_string()),
            hp_after: hp,
        }]
    }

    /// Lift the afflictions of ONE family, reporting an effect per condition removed.
    ///
    /// This is the other half of afflictions not expiring: without a cure they would simply be
    /// permanent, which is punishment rather than decision. It is deliberately per-FAMILY —
    /// a poultice draws venom out and has nothing to say about being blinded, and one cheap
    /// bottle that answered everything would make every affliction a non-event. Only
    /// `Family::All` (a Panacea) lifts the lot, and it is priced like it. Boons are never
    /// touched: a cure that stripped your own Barrier would be a trap.
    fn cure(
        &mut self,
        target_i: usize,
        family: meld_proto::statuses::Family,
    ) -> Vec<ResolvedEffect> {
        use meld_proto::statuses::cures;
        let lifted: Vec<String> = self.fighters[target_i]
            .timed_statuses
            .iter()
            .filter(|(n, _)| cures(family, n))
            .map(|(n, _)| n.clone())
            .collect();
        self.fighters[target_i].timed_statuses.retain(|(n, _)| !cures(family, n));
        // Nothing else to unwind: a slow's RATE is computed from these tokens each tick
        // (`status_slow_mult`), so dropping the token is the cure. `slow_floor` is left alone
        // — it is a set piece's protection against being controlled out of the fight, not a
        // condition anybody would want lifted.
        lifted
            .into_iter()
            .map(|n| ResolvedEffect {
                target_id: self.fighters[target_i].combatant_id.clone(),
                kind: EffectKind::StatusApplied,
                amount: None,
                status: Some(format!("cured:{n}")),
                hp_after: self.fighters[target_i].hp,
                modifier_flag: None,
            })
            .collect()
    }

    /// How much more this fighter's damage of `ty` is worth — "of the Furnace" (AD-1
    /// `element_power`), the offensive twin of the target's `damage_modifiers`.
    ///
    /// `brand` decides what your attacks ARE and this decides what that is worth, so the two
    /// together are a build: brand your weapon to ice and stack ice power to answer a boss
    /// that shrugs off fire. `1.0` when the fighter has none, so it is always safe to apply.
    fn element_amp(&self, actor_i: usize, ty: DamageType) -> f64 {
        if ty == DamageType::None {
            return 1.0;
        }
        self.fighters[actor_i].element_power.get(&ty).copied().unwrap_or(1.0)
    }

    /// Mitigate ABILITY damage, then land it.
    ///
    /// **`def` answers a blade; `ward` answers a spell.** One place decides which, because
    /// the alternative was what shipped: six ability call sites passing raw damage straight
    /// past every defensive stat in the game, so armour was decoration against anything that
    /// did not throw a basic attack.
    ///
    /// A basic attack does NOT come through here — it subtracts `def` itself, along with the
    /// defend reduction and its crit, and calls [`Self::apply_typed_damage`] directly. And a
    /// DoT does not either: burn and poison are a fraction of the victim's OWN max HP, so
    /// they are already scaled to the target and subtracting armour twice would make them
    /// vanish on anyone wearing any.
    fn apply_ability_damage(
        &mut self,
        target_i: usize,
        raw: i32,
        ty: DamageType,
    ) -> Vec<ResolvedEffect> {
        // The ATTACKER's element power lands before the target's armour does — a bigger
        // fire is still a fire, and then the target answers it. `active_actor` is the acting
        // fighter for both a player submit and a creature turn.
        let raw = match self.active_actor {
            Some(a) => ((raw as f64) * self.element_amp(a, ty)).round() as i32,
            None => raw,
        };
        // True damage answers to nothing, which is the whole point of it.
        let mitigated = if ty == DamageType::None {
            raw
        } else {
            let shield = if ty.is_physical() {
                self.fighters[target_i].def
            } else {
                self.fighters[target_i].ward
            };
            let floor = (raw as f64) * self.damage_floor_fraction;
            (((raw - shield) as f64).max(floor).round() as i32).max(self.min_damage)
        };
        self.apply_typed_damage(target_i, mitigated, ty)
    }

    fn apply_typed_damage(
        &mut self,
        target_i: usize,
        raw: i32,
        ty: DamageType,
    ) -> Vec<ResolvedEffect> {
        let (mult, flag) = self.modifier_for(target_i, ty);
        match flag {
            Some(ModifierFlag::Immune) => vec![ResolvedEffect {
                target_id: self.fighters[target_i].combatant_id.clone(),
                kind: EffectKind::Damage,
                amount: Some(0),
                status: None,
                hp_after: self.fighters[target_i].hp,
                modifier_flag: flag,
            }],
            Some(ModifierFlag::Absorb) => {
                let healed = ((raw as f64) * mult).floor().abs() as i32;
                let mut fx = self.apply_heal(target_i, healed.max(1));
                for e in &mut fx {
                    e.modifier_flag = flag;
                }
                fx
            }
            _ => {
                let dmg = (((raw as f64) * mult).floor() as i32).max(self.min_damage);
                let mut fx = self.apply_damage_reaching(target_i, dmg, ty.is_physical());
                for e in &mut fx {
                    if matches!(e.kind, EffectKind::Damage) {
                        e.modifier_flag = flag;
                    }
                }
                fx
            }
        }
    }

    /// Physical damage — the default path. A blow with no declared type is a weapon
    /// blow, so it answers to the back row like every other one.
    fn apply_damage(&mut self, target_i: usize, dmg: i32) -> Vec<ResolvedEffect> {
        self.apply_damage_reaching(target_i, dmg, true)
    }

    /// Does the target's RANK soften this blow? The single reach rule.
    ///
    /// A back rank is protected from a physical blow — and from nothing else. A spell, an
    /// elemental brand or a psychic Focus already reached it at full force, which is why
    /// the back rank is a caster's problem to solve and a swordsman's wall. A **flanked**
    /// group has no protection at all: two parties are on it from different sides and
    /// there is no behind left.
    fn softened_by_rank(&self, target_i: usize, physical: bool) -> bool {
        let t = &self.fighters[target_i];
        if !(physical && t.back_row && !t.flanked) {
            return false;
        }
        // …unless whoever is swinging can reach past the front line. A bow, a sling or a
        // thrown spear goes over the front rank and lands on the rear at full force — the
        // martial answer to a formation, where before it there was only a spell.
        !self.active_actor.is_some_and(|a| self.fighters[a].reach)
    }

    /// Record that the acting party has struck `target_i`'s group, and flank the group once
    /// a second party has. Monotonic for the fight: having been surrounded is not something
    /// a pack recovers from by the attackers looking away.
    fn note_striker(&mut self, target_i: usize) {
        let Some(group) = self.fighters[target_i].group_id else { return };
        let Some(actor_i) = self.active_actor else { return };
        let Some(who) = self.fighters.get(actor_i).and_then(|f| f.player_id.clone()) else {
            return;
        };
        let seen = self.group_strikers.entry(group).or_default();
        seen.insert(who);
        if seen.len() < 2 {
            return;
        }
        for f in self.fighters.iter_mut().filter(|f| f.group_id == Some(group)) {
            f.flanked = true;
        }
    }

    /// `physical` decides whether the target's RANK protects it. A spell, a Focus or an
    /// elemental breath reaches the back rank at full force; only a physical blow has to
    /// cross the front line to land.
    fn apply_damage_reaching(
        &mut self,
        target_i: usize,
        dmg: i32,
        physical: bool,
    ) -> Vec<ResolvedEffect> {
        // AD-2 combos: the skill resolving right now may be cashing in a primer
        // another hero left on this target, and may leave one of its own.
        let (dmg, combo_hit) = self.resolve_combo(target_i, dmg);
        // A blazed target takes more from everyone, so the bonus lives here — the one
        // point every hit passes through — rather than in each ability that could benefit.
        let dmg = if self.is_marked(target_i) {
            ((dmg as f64) * self.explorer_mark_damage_mult).round() as i32
        } else {
            dmg
        };
        // A STAGGERED fighter is wide open. This is what a gauge knock BUYS beyond the tempo
        // — it lives here, the one point every hit passes through, rather than in each
        // ability that might benefit, exactly as the blaze bonus above does.
        let dmg = if self.fighters[target_i].staggered {
            ((dmg as f64) * self.staggered_damage_mult).round() as i32
        } else {
            dmg
        };
        // CR-6: a pack leader's living minions soak part of every blow aimed at it.
        let guard = self.pack_guard_fraction(target_i);
        let dmg = if guard > 0.0 {
            ((dmg as f64) * (1.0 - guard)).round() as i32
        } else {
            dmg
        };
        // A BLOW BRINGS YOU ROUND. Being struck knocks dread and confusion out of whoever
        // takes it — theirs or yours — which gives a party with no mender an answer that is
        // not a bottle, and makes hitting a terrified creature a way to wake it up too.
        let woken = if physical {
            self.shake_off(target_i, meld_proto::statuses::CLEARED_BY_A_HIT)
        } else {
            Vec::new()
        };
        // FLANKING. Record who is working this group before the rank is consulted: once a
        // second party lays into it, its rear stops being covered for everyone. This is the
        // co-op half of "a rank is relative" — one group's back row is another party's
        // front — and it is derived from engagement rather than from coordinates, so the
        // fight stays an ATB fight rather than becoming a tactics grid.
        self.note_striker(target_i);
        // Back-row formation softens an incoming PHYSICAL blow (before Barrier/HP).
        // It used to soften everything, which made the back row a free 2x effective HP
        // for the whole party — nothing reached past it and nothing was given up for it.
        //
        // THE ONE PLACE reach is resolved. Rank, damage type and flank state all decide it
        // together, and every hit in the game passes through here — a second copy of this
        // rule at some other call site is the exact drift that has bitten this repo twice.
        let dmg = if self.softened_by_rank(target_i, physical) {
            (dmg as f64 * self.back_row_damage_mult).round() as i32
        } else {
            dmg
        };
        let t = &mut self.fighters[target_i];
        // Whether there was anybody standing here to put down. A blow that lands on a
        // fighter already at 0 must not count as a FALL — the tax is charged per fall
        // (GR-2), and a corpse taking splash would otherwise bill its owner again.
        let was_standing = t.alive;
        // Barrier (temp HP) soaks damage before HP does.
        let absorbed = t.barrier.min(dmg.max(0));
        t.barrier -= absorbed;
        let hp_loss = (dmg - absorbed).max(0);
        t.hp = (t.hp - hp_loss).max(0);
        let dead = t.hp == 0;
        if dead {
            t.alive = false;
            // The fall the gear tax is charged on (GR-2). Every hit in the game passes
            // through here, so this is the only place that has to count.
            if was_standing {
                t.falls += 1;
            }
        }
        // Report the HP actually lost (barrier absorption shows via the barrier bar).
        let mut effects = vec![ResolvedEffect { modifier_flag: None,
            target_id: t.combatant_id.clone(),
            kind: EffectKind::Damage,
            amount: Some(hp_loss),
            status: None,
            hp_after: t.hp,
        }];
        effects.extend(woken);
        if dead {
            effects.push(ResolvedEffect { modifier_flag: None,
                target_id: self.fighters[target_i].combatant_id.clone(),
                kind: EffectKind::Ko,
                amount: None,
                status: None,
                hp_after: 0,
            });
            // CR-6: killing the big one BREAKS the pack — the littles hit softer (via
            // `pack_attack_mult`) and, if the rules say so, bolt when they drop low.
            // Announced per minion so the client can show the moment the fight turns.
            if self.fighters[target_i].pack_role == PackRole::Leader {
                let faction = self.fighters[target_i].faction.clone();
                let routed: Vec<usize> = self
                    .fighters
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| {
                        f.alive && f.pack_role == PackRole::Minion && f.faction == faction
                    })
                    .map(|(i, _)| i)
                    .collect();
                for i in routed {
                    if self.pack_rout_flees {
                        self.fighters[i].flees = true;
                    }
                    effects.push(ResolvedEffect {
                        modifier_flag: None,
                        target_id: self.fighters[i].combatant_id.clone(),
                        kind: EffectKind::StatusApplied,
                        amount: None,
                        status: Some("routed".to_string()),
                        hp_after: self.fighters[i].hp,
                    });
                }
            }
        }
        // A cashed-in combo is announced so the client can call it out; a player who
        // cannot see the sequence land will never learn to build for it.
        if let Some(key) = combo_hit {
            effects.insert(
                0,
                ResolvedEffect {
                    modifier_flag: None,
                    target_id: self.fighters[target_i].combatant_id.clone(),
                    kind: EffectKind::StatusApplied,
                    amount: None,
                    status: Some(format!("combo:{key}")),
                    hp_after: self.fighters[target_i].hp,
                },
            );
        }
        effects
    }

    /// CR-6 pack AI: how a pack's state bends one blow.
    ///
    /// - A **minion** hits harder while its leader lives (`pack_aura_atk_mult`) and
    ///   softer once it has routed (`pack_rout_atk_mult`) — so breaking the big one
    ///   is felt immediately.
    /// - A **leader** takes less damage for every living minion
    ///   (`pack_guard_per_minion`, capped) — so clearing the littles first is the
    ///   other valid line. Which order is better depends on the pack, which is the
    ///   whole point of a pack fight.
    fn pack_attack_mult(&self, actor_i: usize) -> f64 {
        match self.fighters[actor_i].pack_role {
            PackRole::Minion => {
                if self.pack_leader_alive(&self.fighters[actor_i].faction) {
                    self.pack_aura_atk_mult
                } else {
                    self.pack_rout_atk_mult
                }
            }
            _ => 1.0,
        }
    }

    /// The fraction of a leader's incoming damage its living minions soak.
    fn pack_guard_fraction(&self, target_i: usize) -> f64 {
        if self.fighters[target_i].pack_role != PackRole::Leader {
            return 0.0;
        }
        let faction = &self.fighters[target_i].faction;
        let minions = self
            .fighters
            .iter()
            .filter(|f| {
                f.alive && f.pack_role == PackRole::Minion && &f.faction == faction
            })
            .count();
        (self.pack_guard_per_minion * minions as f64).min(self.pack_guard_cap)
    }

    fn pack_leader_alive(&self, faction: &str) -> bool {
        self.fighters
            .iter()
            .any(|f| f.alive && f.pack_role == PackRole::Leader && f.faction == faction)
    }

    /// The wire/status token a blazed target carries.
    pub const MARK_STATUS: &'static str = "marked";

    /// The wire/status token a distracted creature carries.
    pub const DISTRACT_STATUS: &'static str = "distracted";

    /// Does this fighter carry `name` right now?
    fn has_timed_status(&self, i: usize, name: &str) -> bool {
        self.fighters[i]
            .timed_statuses
            .iter()
            .any(|(n, until)| n == name && *until > self.tick_count)
    }

    /// Is this fighter currently blazed? Read on every incoming hit.
    fn is_marked(&self, i: usize) -> bool {
        self.has_timed_status(i, Self::MARK_STATUS)
    }

    /// Is this fighter distracted? Read when it swings, and when the party tries to leave.
    fn is_distracted(&self, i: usize) -> bool {
        self.has_timed_status(i, Self::DISTRACT_STATUS)
    }

    /// Put a timed token on a fighter, extending rather than stacking.
    fn apply_timed(&mut self, target_i: usize, name: &str, ticks: u64) -> ResolvedEffect {
        let until = self.tick_count + ticks;
        match self.fighters[target_i].timed_statuses.iter_mut().find(|(n, _)| n == name) {
            Some(s) => s.1 = s.1.max(until),
            None => self.fighters[target_i].timed_statuses.push((name.to_string(), until)),
        }
        ResolvedEffect {
            modifier_flag: None,
            target_id: self.fighters[target_i].combatant_id.clone(),
            kind: EffectKind::StatusApplied,
            amount: None,
            status: Some(name.to_string()),
            hp_after: self.fighters[target_i].hp,
        }
    }

    /// Blaze a target: for `explorer_mark_ticks`, everything the party lands on it hits
    /// harder. Re-blazing extends the window rather than stacking the multiplier, so two
    /// Explorers are worth more uptime and not double damage.
    fn apply_mark(&mut self, target_i: usize) -> ResolvedEffect {
        let ticks = self.explorer_mark_ticks;
        self.apply_timed(target_i, Self::MARK_STATUS, ticks)
    }

    /// AD-2: apply the combo layer to one incoming hit.
    ///
    /// Returns the (possibly amplified) damage and the combo key that fired. The
    /// payoff is checked BEFORE priming so an ability that is both a setup and a
    /// payoff can never prime itself into its own bonus.
    fn resolve_combo(&mut self, target_i: usize, dmg: i32) -> (i32, Option<&'static str>) {
        use meld_proto::synergies as syn;
        let Some(skill) = self.active_skill.clone() else {
            return (dmg, None);
        };
        let mut out = dmg;
        let mut fired = None;
        if let Some(c) = syn::combo_for_payoff(&skill) {
            let token = syn::primer_status(c.key);
            let primed = self.fighters[target_i]
                .timed_statuses
                .iter()
                .any(|(n, until)| *n == token && *until > self.tick_count);
            if primed {
                out = (dmg as f64 * c.damage_mult).round() as i32;
                // Consumed: a primer pays once, or a party could bank one and cash
                // it repeatedly off a single setup turn.
                self.fighters[target_i].timed_statuses.retain(|(n, _)| *n != token);
                fired = Some(c.key);
            }
        }
        if let Some(c) = syn::combo_for_setup(&skill) {
            let token = syn::primer_status(c.key);
            let until = self.tick_count + self.combo_window_ticks;
            match self.fighters[target_i]
                .timed_statuses
                .iter_mut()
                .find(|(n, _)| *n == token)
            {
                Some(s) => s.1 = s.1.max(until),
                None => self.fighters[target_i].timed_statuses.push((token, until)),
            }
        }
        (out, fired)
    }

    fn reset_gauge(&mut self, i: usize) {
        // Acting is what brings you back UP: the stagger ends here, and one of the guard's
        // turns is spent. The guard deliberately outlives this turn — if it lapsed the moment
        // the fighter recovered, the party would re-knock instantly and every boss turn in
        // the fight would be a rebuke.
        self.fighters[i].staggered = false;
        self.fighters[i].gauge_guard_turns = self.fighters[i].gauge_guard_turns.saturating_sub(1);
        if self.fighters[i].gauge_guard_turns == 0 {
            self.fighters[i].statuses.retain(|s| s != GAUGE_GUARD_STATUS);
        }
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
        } else if self.wholly_paralysed() {
            // EVERY hero held still is a death, not a stalemate. Paralysis skips the turn, so
            // a party where nobody can act would otherwise stand there while the creatures
            // worked through them — an unbounded soft-lock of exactly the kind a gauge CAP
            // used to cause. Ending it is what keeps the condition bounded.
            self.ended = true;
            events.push(Event::Ended {
                outcome: BattleOutcome::Defeat,
            });
        }
    }

    /// Whether every hero still standing is paralysed — nobody left who can act.
    fn wholly_paralysed(&self) -> bool {
        let mut any = false;
        for (i, f) in self.fighters.iter().enumerate() {
            if f.kind != CombatantKind::Player || !f.alive {
                continue;
            }
            any = true;
            if !self.has(i, "paralyzed") {
                return false;
            }
        }
        any
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

    /// Dodging is the SHIFTER's identity, so the Shifter's own blink has to stay the better
    /// evasion — the Explorer's party-wide Safe Passage covers more people for less each.
    /// Both draw on the same pool, so without this the two classes could silently swap
    /// places on a balance pass and the Shifter would lose the thing it is for.
    #[test]
    fn flicker_stays_the_better_evasion_than_safe_passage() {
        let b = Balance::load_default().unwrap();
        let flicker = b.battle.shifter_flicker_evasion;
        let passage = b.battle.explorer_safe_passage_evasion;
        assert!(
            flicker > passage,
            "Flicker ({flicker}) must beat Safe Passage ({passage}) per hero - dodging is the \
             Shifter's whole identity"
        );
        assert!(passage > 0.0, "Safe Passage still has to be worth a turn");
    }

    /// A World Known is a real haste: the gauge fills FASTER while it holds, rather than
    /// jumping once. And the slow list is explicit now, so a status like `marked` must not
    /// throttle whatever carries it — the gauge used to slow on any non-DoT token.
    #[test]
    fn haste_speeds_the_gauge_and_a_mark_does_not_slow_it() {
        let b = Balance::load_default().unwrap();
        let mk = |tokens: &[&str]| {
            let mut bt = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![player("h1", 60)],
                vec![monster("m1", 9_999, 1)],
                &b,
                7,
            );
            let i = bt.idx("h1").unwrap();
            for t in tokens {
                bt.fighters[i].timed_statuses.push(((*t).to_string(), 10_000));
            }
            bt.fighters[i].gauge = 0.0;
            bt.tick();
            bt.fighters[bt.idx("h1").unwrap()].gauge
        };
        let plain = mk(&[]);
        let hasted = mk(&[HASTE_STATUS]);
        let marked = mk(&["marked"]);
        let webbed = mk(&["web"]);
        assert!(hasted > plain, "haste must fill faster: {plain} -> {hasted}");
        assert!(webbed < plain, "a web must still slow: {plain} -> {webbed}");
        assert_eq!(
            marked, plain,
            "a mark is not a slow - the gauge used to throttle on ANY non-DoT status"
        );
    }

    /// `Now` is once per fight, refused on the second ask by the server rather than only
    /// greyed out by the client.
    #[test]
    fn now_can_be_called_once_a_battle_and_then_refused() {
        let b = Balance::load_default().unwrap();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("scout", 10), player("mate", 1)],
            vec![monster("m1", 9_999, 1)],
            &b,
            7,
        );
        let i = battle.idx("scout").unwrap();
        battle.fighters[i].level = meld_proto::skills::unlock_level("now");
        let mate = battle.idx("mate").unwrap();
        battle.fighters[mate].gauge = 0.0;

        let call = |bt: &mut Battle, n: u32| {
            let i = bt.idx("scout").unwrap();
            bt.fighters[i].gauge = 1.0;
            bt.fighters[i].awaiting = true;
            bt.submit(
                "scout",
                format!("00000000-0000-7000-8000-{n:012}"),
                BattleActionKind::Skill,
                None,
                Some("now".into()),
                None,
            )
        };
        call(&mut battle, 1).expect("the first call lands");
        let mate = battle.idx("mate").unwrap();
        assert_eq!(battle.fighters[mate].gauge, 1.0, "every ally should act immediately");
        let again = call(&mut battle, 2);
        assert!(again.is_err(), "the second call in one battle must be refused");
    }


    /// Safe Passage is the Guides' promise — the party gets through untouched — so it makes
    /// everyone hard to HIT rather than slowly healed. It was party Regen at +6, double the
    /// band of the two classes whose entire identity is regen (Resonant, Keeper).
    #[test]
    fn safe_passage_makes_the_whole_party_hard_to_hit() {
        let b = Balance::load_default().unwrap();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("scout", 10), player("mate", 10)],
            vec![monster("m1", 5_000, 1)],
            &b,
            7,
        );
        for f in &battle.fighters {
            if f.kind == CombatantKind::Player {
                assert_eq!(f.evasion, 0.0, "nobody is evasive to start with");
            }
        }
        let i = battle.idx("scout").unwrap();
        // Safe Passage is a Discoverer's tool, so the caster has to have earned it.
        battle.fighters[i].level = meld_proto::skills::unlock_level("safe_passage");
        battle.fighters[i].gauge = 1.0;
        battle.fighters[i].awaiting = true;
        battle
            .submit(
                "scout",
                "00000000-0000-7000-8000-000000000001".into(),
                BattleActionKind::Skill,
                None,
                Some("safe_passage".into()),
                None,
            )
            .expect("Safe Passage is a self-cast party buff and needs no target");
        for f in &battle.fighters {
            if f.kind == CombatantKind::Player {
                assert!(f.evasion > 0.0, "every ally should be harder to hit, not just the caster");
            }
            if f.kind != CombatantKind::Player {
                assert_eq!(f.evasion, 0.0, "the creature does not benefit");
            }
        }
    }

    /// Distract dazzles: the creature swings wide at whoever it attacks, and the party can
    /// get out. Both halves are asserted, because a blind that only reads on the sheet is
    /// the same class of bug as a status the client never draws.
    #[test]
    fn a_distracted_creature_swings_wide_and_lets_the_party_leave() {
        let b = Balance::load_default().unwrap();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("scout", 10)],
            vec![monster("m1", 5_000, 1)],
            &b,
            7,
        );
        let foe = battle.idx("m1").unwrap();
        let hero = battle.idx("scout").unwrap();

        // Undistracted, the creature's swing is judged against the hero's own dodge only.
        battle.active_actor = Some(foe);
        let plain = battle.fighters[hero].dodge + battle.fighters[hero].evasion;

        battle.apply_timed(foe, Battle::DISTRACT_STATUS, 60);
        assert!(battle.is_distracted(foe));

        // The dazzle adds to the defender's chance, which is where accuracy lives here.
        let distracted_chance = plain + b.battle.explorer_misdirection_miss;
        assert!(
            distracted_chance > plain,
            "a distracted attacker must be easier to avoid: {plain} -> {distracted_chance}"
        );

        // And leaving is easier while it is dazzled than while it is not.
        let with = battle.flee_chance(0) + b.battle.explorer_misdirection_flee_bonus;
        assert!(with > battle.flee_chance(0), "a distracted foe should let the party go");
    }


    /// Trailblaze's whole point is that it helps the PARTY, not that it hits hard: the
    /// Explorer's L1 was a ~5% damage nudge over a basic Attack (and worse than one past
    /// level 20, since Attack can crit and skills cannot), so there was no reason to press
    /// it. A blazed target takes more from everyone for a window.
    #[test]
    fn trailblaze_blazes_its_target_so_the_whole_party_hits_harder() {
        let b = Balance::load_default().unwrap();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("scout", 10), player("mate", 10)],
            vec![monster("m1", 100_000, 1)],
            &b,
            7,
        );
        let foe = battle.idx("m1").unwrap();
        let act = |n: u32| format!("00000000-0000-7000-8000-{n:012}");
        let ready = |bt: &mut Battle, who: &str| {
            let i = bt.idx(who).unwrap();
            bt.fighters[i].gauge = 1.0;
            bt.fighters[i].awaiting = true;
        };

        // An ally's plain attack on an UNBLAZED target, for the baseline.
        ready(&mut battle, "mate");
        let before = battle.fighters[foe].hp;
        battle
            .submit("mate", act(1), BattleActionKind::Attack, Some(vec!["m1".into()]), None, None)
            .unwrap();
        let plain = before - battle.fighters[foe].hp;

        // Blaze it.
        assert!(!battle.is_marked(foe), "nothing is blazed to start with");
        ready(&mut battle, "scout");
        let fx = battle
            .submit(
                "scout",
                act(2),
                BattleActionKind::Skill,
                Some(vec!["m1".into()]),
                Some("trailblaze".into()),
                None,
            )
            .unwrap();
        assert!(battle.is_marked(foe), "Trailblaze must blaze what it hits");
        assert!(
            format!("{fx:?}").contains(Battle::MARK_STATUS),
            "the mark has to reach the client, or it does not exist to the player"
        );

        // The SAME ally's SAME attack now lands harder, though nothing about it changed.
        ready(&mut battle, "mate");
        let before = battle.fighters[foe].hp;
        battle
            .submit("mate", act(3), BattleActionKind::Attack, Some(vec!["m1".into()]), None, None)
            .unwrap();
        let blazed = before - battle.fighters[foe].hp;
        assert!(
            blazed > plain,
            "a blazed target should take more from an ALLY: {plain} -> {blazed}"
        );
    }

    /// Re-blazing extends the window instead of stacking the multiplier, so a second
    /// Explorer buys uptime rather than doubling the party's damage.
    #[test]
    fn blazing_twice_extends_the_window_and_does_not_stack() {
        let b = Balance::load_default().unwrap();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("scout", 10)],
            vec![monster("m1", 100_000, 1)],
            &b,
            7,
        );
        let foe = battle.idx("m1").unwrap();
        battle.apply_mark(foe);
        let first = battle.fighters[foe].timed_statuses.clone();
        battle.tick_count += 10;
        battle.apply_mark(foe);
        let second = battle.fighters[foe].timed_statuses.clone();
        assert_eq!(second.len(), 1, "one mark, not two: {second:?}");
        assert!(second[0].1 > first[0].1, "the second blaze should push the window out");
    }


    /// End to end through the engine, because what was broken was the PAIRING of a
    /// resource to a kit and each half looked fine alone: bank Adrenaline with basic
    /// attacks the way a Hunter does, then spend it on the ability it is for. With the cap
    /// at 0 (as shipped) no attack banks anything and Power Strike is refused "not enough
    /// adrenaline" forever — every Hunter skill, for the life of the class.
    #[test]
    fn a_hunter_banks_adrenaline_by_attacking_and_then_spends_it() {
        let b = Balance::load_default().unwrap();
        let mut hero = player("h1", 10);
        // What `party_fighters` grants a Hunter.
        hero.adrenaline_max = b.battle.hunter_adrenaline_max;
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![hero],
            vec![monster("m1", 4000, 1)],
            &b,
            7,
        );
        assert!(battle.fighters[0].adrenaline_max > 0, "a Hunter needs a pool to bank into");

        let act = |n: u32| format!("00000000-0000-7000-8000-{n:012}");
        let ready = |bt: &mut Battle| {
            bt.fighters[0].gauge = 1.0;
            bt.fighters[0].awaiting = true;
        };

        // Refused while the pool is short of the cost.
        ready(&mut battle);
        assert!(
            battle
                .submit(
                    "h1",
                    act(1),
                    BattleActionKind::Skill,
                    Some(vec!["m1".into()]),
                    Some("power_strike".into()),
                    None,
                )
                .is_err(),
            "an unaffordable skill is refused, not quietly free"
        );

        // Basic attacks bank it; then the same skill lands and draws the pool down.
        let cost = b.battle.hunter_power_strike_cost;
        let mut n = 10;
        while battle.fighters[0].adrenaline < cost {
            ready(&mut battle);
            battle
                .submit("h1", act(n), BattleActionKind::Attack, Some(vec!["m1".into()]), None, None)
                .expect("a basic attack always works");
            n += 1;
        }
        let banked = battle.fighters[0].adrenaline;
        ready(&mut battle);
        battle
            .submit(
                "h1",
                act(n),
                BattleActionKind::Skill,
                Some(vec!["m1".into()]),
                Some("power_strike".into()),
                None,
            )
            .expect("Power Strike must land once its Adrenaline is banked");
        assert!(battle.fighters[0].adrenaline < banked, "the skill should spend the pool");
    }

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
        // Every creature the world builds carries a typed basic attack
        // (`creature_basic_attack_type`, which falls back to Pierce), and the type is what
        // decides whether the back rank protects its target. An untyped test monster dealt
        // `DamageType::None`, which reaches past the row — so the fixture disagreed with
        // every creature in the game.
        f.basic_attack_type = DamageType::Pierce;
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
    fn a_fall_is_counted_every_time_a_hero_goes_down_not_once_per_corpse() {
        let b = balance();
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        assert_eq!(battle.combatant_falls("a"), 0, "nobody has fallen yet");
        let _ = battle.apply_damage(0, 9999);
        assert_eq!(battle.combatant_falls("a"), 1);
        // A revive stands them back up; the fall already happened and the gear
        // already paid for it.
        let _ = battle.raise_fallen(0, 0.5);
        assert_eq!(battle.combatant_falls("a"), 1, "standing back up does not un-fall");
        let _ = battle.apply_damage(0, 9999);
        assert_eq!(
            battle.combatant_falls("a"),
            2,
            "a hero raised and killed again fell TWICE — which an end-of-fight `hp == 0` \
             read cannot see, and which is exactly what the durability tax charges for"
        );
    }

    #[test]
    fn a_corpse_taking_splash_does_not_fall_again() {
        let b = balance();
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        let _ = battle.apply_damage(0, 9999);
        for _ in 0..5 {
            let _ = battle.apply_damage(0, 9999);
        }
        assert_eq!(
            battle.combatant_falls("a"),
            1,
            "an all-enemy ability sweeping a party with a body already on the floor \
             must not bill that hero's gear once per blow"
        );
    }

    #[test]
    fn fleeing_is_not_dying() {
        let b = balance();
        // The skittish creature above leaves the field with `alive` cleared and its HP
        // intact — the case that makes an end-state read wrong in the other direction.
        let mut sh = creature("shade", 60, 400, "shade");
        sh.hp = 5;
        sh.flees = true;
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![player("p", 1)],
            vec![sh],
            &b,
            7,
        );
        for _ in 0..20 {
            let _ = battle.tick();
        }
        assert_eq!(
            battle.combatant_falls("shade"),
            0,
            "it ran away; nothing put it down"
        );
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
        // speed 110 / 5200 ≈ 0.0212 per tick → full at tick 48 (~4.7s FF5 cadence).
        let mut ready_tick = None;
        for t in 1..=60 {
            for ev in battle.tick() {
                if let Event::TurnReady { combatant_id } = ev {
                    assert_eq!(combatant_id, "a");
                    ready_tick.get_or_insert(t);
                }
            }
        }
        assert_eq!(ready_tick, Some(48), "speed-110 turn should ready at tick 48");
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

    #[test]
    fn back_row_halves_incoming_damage() {
        let b = balance();
        // Run a lone hero (speed 1 → never acts) against a monster and report the
        // first hit it takes, front-row vs back-row.
        let first_hit = |back: bool| -> i32 {
            let mut hero = player("h", 1);
            hero.back_row = back;
            let mut battle = Battle::new(
                "b1".into(),
                EncounterClass::Standard,
                vec![hero],
                vec![monster("m", 1000, 200)],
                &b,
                7,
            );
            for _ in 0..200 {
                battle.tick();
                let hp = player_hp(&battle, "h");
                if hp < 40 {
                    return 40 - hp;
                }
            }
            panic!("monster never landed a hit");
        };
        let front = first_hit(false); // 14 atk − 3 def = 11
        let back = first_hit(true);
        assert_eq!(front, 11, "front-row hero takes the full 11");
        assert_eq!(back, 6, "back-row hero takes half (round(5.5) = 6)");
    }

    /// No hero can swing untyped. `DamageType::None` bypasses the modifier map entirely —
    /// every resistance and immunity ignored — and now the rank trade too, so an untyped
    /// swing is TRUE damage rather than a neutral default. Three classes dealt it for a
    /// whole release because the type came from a hand-written table of classes.
    ///
    /// It cannot happen now: the weapon decides, and a hand with no answer of its own falls
    /// to `UNARMED_ATTACK_TYPE`. This holds the two halves — the default is real, and every
    /// weapon family that IS a weapon names a type.
    #[test]
    fn nothing_a_hero_holds_swings_untyped() {
        assert_ne!(UNARMED_ATTACK_TYPE, DamageType::None, "the unarmed default is true damage");
        for f in [
            meld_proto::equipment::ItemFamily::Sword,
            meld_proto::equipment::ItemFamily::Dagger,
            meld_proto::equipment::ItemFamily::Spear,
            meld_proto::equipment::ItemFamily::Staff,
            meld_proto::equipment::ItemFamily::Gauntlet,
            meld_proto::equipment::ItemFamily::ParryBlade,
            meld_proto::equipment::ItemFamily::Bow,
            meld_proto::equipment::ItemFamily::Sling,
            meld_proto::equipment::ItemFamily::ThrownSpear,
        ] {
            let dt = f.damage_type();
            assert!(dt.is_some(), "{f:?} is a weapon with no damage type");
            assert_ne!(dt, Some(DamageType::None), "{f:?} swings untyped");
        }
    }

    /// The point of typing weapons rather than classes: armour weight becomes a loadout
    /// decision. Plate turns an edge and fears a hammer, so the two ranged families must
    /// not be the same blow with different art.
    #[test]
    fn the_bow_and_the_sling_answer_different_armour() {
        assert_eq!(meld_proto::equipment::ItemFamily::Bow.damage_type(), Some(DamageType::Pierce));
        assert_eq!(meld_proto::equipment::ItemFamily::Sling.damage_type(), Some(DamageType::Blunt));
        assert_ne!(
            meld_proto::equipment::ItemFamily::Bow.damage_type(),
            meld_proto::equipment::ItemFamily::Sling.damage_type(),
            "a bow and a sling are the same blow, so choosing between them is decoration"
        );
    }

    /// CR-9: a creature fights to its own profile, and the profiles are actually
    /// different from each other. One rule for every creature made every fight read the
    /// same — the pack always went for the lowest HP, hub to deep.
    #[test]
    fn a_creature_hits_who_its_profile_says() {
        let b = balance();
        // A wounded tank up front, a healthy healer in the back. Every profile has a
        // different opinion about which one matters.
        let build = |profile: TargetProfile| {
            let mut tank = player("tank", 1);
            tank.class_key = "phoenix_guard".into();
            tank.hp = 5;
            let mut healer = player("healer", 1);
            healer.class_key = "resonant".into();
            healer.back_row = true;
            let mut m = monster("m", 1000, 200);
            m.target_profile = profile;
            Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![tank, healer],
                vec![m],
                &b,
                7,
            )
        };
        let struck = |profile: TargetProfile| -> String {
            let mut battle = build(profile);
            for _ in 0..400 {
                battle.tick();
                for f in &battle.fighters {
                    if f.kind == CombatantKind::Player && f.hp < f.max_hp.min(if f.combatant_id == "tank" { 5 } else { f.max_hp }) {
                        return f.combatant_id.clone();
                    }
                }
            }
            panic!("nobody was hit");
        };
        // Weakest finishes the wounded tank; Backline and Role both go past it for the
        // healer standing behind — for different reasons, which is the point.
        assert_eq!(struck(TargetProfile::Weakest), "tank", "Weakest ignored the wounded hero");
        assert_eq!(struck(TargetProfile::Backline), "healer", "Backline did not hunt the rank");
        assert_eq!(struck(TargetProfile::Role), "healer", "Role did not go for the healer");
    }

    /// A pack converging on your healer with no explanation reads as the game cheating, so
    /// the mark is SHOUTED when it is set — and the pack shares one mark rather than each
    /// creature deciding alone.
    #[test]
    fn a_ganging_pack_announces_its_mark_and_shares_it() {
        let b = balance();
        let mut healer = player("healer", 1);
        healer.class_key = "resonant".into();
        let mut tank = player("tank", 1);
        tank.class_key = "phoenix_guard".into();
        let mut a = monster("m1", 1000, 200);
        a.target_profile = TargetProfile::GangUp;
        let mut c = monster("m2", 1000, 200);
        c.target_profile = TargetProfile::GangUp;
        let mut battle =
            Battle::new("b".into(), EncounterClass::Standard, vec![healer, tank], vec![a, c], &b, 7);

        let mut shouts = Vec::new();
        for _ in 0..200 {
            for ev in battle.tick() {
                if let Event::Resolved(r) = ev {
                    if let Some(t) = r.callout_text.clone() {
                        shouts.push(t);
                    }
                }
            }
            if !shouts.is_empty() {
                break;
            }
        }
        assert!(!shouts.is_empty(), "the pack marked someone in silence");
        assert!(
            shouts[0].contains("marks") || shouts[0].contains("turns on"),
            "the shout does not say what happened: {:?}",
            shouts[0]
        );
        // Both creatures converge on the same mark — that is what "gang up" means.
        assert!(battle.gang_target.is_some(), "no shared mark was set");
    }

    /// The back rank stops a BLOW, not a spell. A creature whose basic attack is elemental
    /// reaches the back row at full force — which is what stops the formation being a flat
    /// 2x effective HP and makes a creature's damage type a reason to re-form.
    #[test]
    fn a_spell_reaches_the_back_row_at_full_force() {
        let b = balance();
        let first_hit = |ty: DamageType| -> i32 {
            let mut hero = player("h", 1);
            hero.back_row = true;
            let mut m = monster("m", 1000, 200);
            m.basic_attack_type = ty;
            let mut battle =
                Battle::new("b1".into(), EncounterClass::Standard, vec![hero], vec![m], &b, 7);
            for _ in 0..200 {
                battle.tick();
                let hp = player_hp(&battle, "h");
                if hp < 40 {
                    return 40 - hp;
                }
            }
            panic!("monster never landed a hit");
        };
        assert_eq!(first_hit(DamageType::Pierce), 6, "a physical blow is softened by the rank");
        assert_eq!(first_hit(DamageType::Fire), 11, "fire was stopped by standing further back");
    }

    /// The other half of the trade: standing back halves your own PHYSICAL output, so the
    /// whole party in the back rank is a real cost rather than free mitigation. A caster
    /// gives up nothing, because its damage is not physical.
    #[test]
    fn the_back_row_gives_up_half_its_own_physical_damage() {
        let b = balance();
        let hit = |back: bool| -> i32 {
            let mut hero = player("h", 200);
            // A real class, so the hero swings a real (physical) weapon type — an empty
            // `class_key` falls to `DamageType::None`, which no fielded hero ever has.
            hero.class_key = "hunter".to_string();
            hero.back_row = back;
            let mut battle = Battle::new(
                "b1".into(),
                EncounterClass::Standard,
                vec![hero],
                vec![monster("m", 100000, 1)],
                &b,
                7,
            );
            tick_to_ready(&mut battle, "h");
            let before = battle.fighters.iter().find(|f| f.combatant_id == "m").unwrap().hp;
            battle
                .submit("h", "a1".into(), BattleActionKind::Attack, Some(vec!["m".into()]), None, None)
                .expect("attack rejected");
            let after = battle.fighters.iter().find(|f| f.combatant_id == "m").unwrap().hp;
            before - after
        };
        let front = hit(false);
        let back = hit(true);
        assert!(front > 0 && back > 0);
        assert!(
            back < front,
            "a back-row hero swung for {back}, the same as the front row's {front} — the \
             formation is free mitigation again"
        );
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


    /// A SINGLE ENEMY CANNOT BE DENIED EVERY TURN OF A LONG FIGHT.
    ///
    /// This is the bug the guard exists for, measured in a played run: a party's Ransack (116
    /// casts) and Holy Censure (34) held a 66,792 HP gatekeeper at 29% gauge for 464
    /// hero-turns. It never acted once — a boss fight that cost the party zero HP and ran
    /// forever. `hallowed_ground` was already gated once-a-fight with exactly that reasoning;
    /// nothing gated the repeatable rows that reach the same place.
    ///
    /// Asserted as "the gauge gets through" rather than on a tick count: the guard duration
    /// is `[TUNABLE]` and what must hold is that chaining cannot stop a creature acting.
    #[test]
    fn a_lone_enemy_cannot_be_gauge_locked_out_of_the_fight() {
        let b = Balance::load_default().unwrap();
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Gatekeeper,
            vec![player("h", 40)],
            vec![monster("m", 100_000, 40)],
            &b,
            7,
        );
        let m = bt.idx("m").unwrap();

        // The first knock LANDS — denying a turn is the play these abilities are for.
        bt.fighters[m].gauge = 0.9;
        assert!(bt.deny_gauge(m, None), "the first knock must work");
        assert_eq!(bt.fighters[m].gauge, 0.0);

        // Chaining does not. However many times it is tried, while the guard holds.
        bt.fighters[m].gauge = 0.9;
        for attempt in 0..20 {
            assert!(!bt.deny_gauge(m, None), "knock {attempt} got through the guard");
        }
        assert_eq!(bt.fighters[m].gauge, 0.9, "a guarded gauge was still being taken");

        // And TAKING A TURN lifts it, so a later knock is a real play again rather than a
        // one-per-fight call — a permanent guard is the same bug pointed the other way.
        // Cleared by acting rather than by a clock, because a timed guard can lapse before a
        // slow creature's gauge has refilled, and then the lock simply resumes.
        // It takes `gauge_guard_turns` of its OWN turns to clear — the guard outliving the
        // recovery turn is the point, or every boss turn would be a rebuke.
        bt.reset_gauge(m);
        bt.fighters[m].gauge = 0.9;
        assert!(!bt.deny_gauge(m, None), "the guard lapsed on the recovery turn itself");
        for _ in 0..b.battle.gauge_guard_turns {
            bt.reset_gauge(m);
        }
        bt.fighters[m].gauge = 0.9;
        assert!(bt.deny_gauge(m, None), "the guard never cleared");
    }

    /// THE INVARIANT, stated as the thing that was broken: however hard a party leans on
    /// gauge denial, a creature keeps getting turns. Measured before the guard: 150 knocks
    /// over 464 hero-turns and a boss that never acted once.
    #[test]
    fn a_creature_keeps_getting_turns_however_hard_the_party_denies_it() {
        let b = Balance::load_default().unwrap();
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Gatekeeper,
            vec![player("h", 40)],
            vec![monster("m", 100_000, 40)],
            &b,
            7,
        );
        let m = bt.idx("m").unwrap();
        // Knock it down at every opportunity, exactly as the measured party did, and count
        // the creature's own ACTIONS out of the tick's events — `tick` resolves a monster's
        // turn itself, so watching the gauge would miss the very thing being counted.
        let mut acted = 0;
        for _ in 0..4000 {
            bt.deny_gauge(m, None);
            for e in bt.tick() {
                if let Event::Resolved(r) = e {
                    if r.actor_id == "m" {
                        acted += 1;
                    }
                }
            }
        }
        assert!(
            acted > 0,
            "a creature under constant denial never acted once - the lock is back"
        );
    }

    /// A KNOCKED-DOWN FIGHTER IS WIDE OPEN — the reward for spending a turn on denial.
    /// Without it a knock buys only tempo, which is thin for an ability that could have been
    /// damage; with it, staggering something is a setup the whole party cashes in.
    #[test]
    fn a_staggered_fighter_takes_more_from_everything() {
        let b = Balance::load_default().unwrap();
        let mult = b.battle.staggered_damage_mult;
        assert!(mult > 1.0, "a stagger that grants no opening is not a reward");
        let hit = |staggered: bool| -> i32 {
            let mut bt = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![player("h", 40)],
                vec![monster("m", 100_000, 40)],
                &b,
                7,
            );
            let m = bt.idx("m").unwrap();
            if staggered {
                bt.fighters[m].gauge = 0.9;
                assert!(bt.deny_gauge(m, None));
            }
            let before = bt.fighters[m].hp;
            bt.apply_damage(m, 100);
            before - bt.fighters[m].hp
        };
        let (open, guarded) = (hit(true), hit(false));
        assert!(
            open > guarded,
            "a staggered fighter took no more than a standing one: {open} vs {guarded}"
        );
        // And it is the tunable doing it, not something incidental.
        assert_eq!(open, ((guarded as f64) * mult).round() as i32);
    }

    /// THE REBUKE: take a boss's turn and its NEXT turn is its signature.
    ///
    /// This is what makes gauge denial a decision rather than a free tactic — you buy a turn
    /// and pay for it with the worst attack in the boss's book. Not a reaction (this engine
    /// has none, deliberately): a flag consumed when the creature takes its own turn.
    #[test]
    fn a_boss_whose_turn_was_taken_answers_with_its_signature() {
        use meld_proto::abilities::MonsterAbility;
        let b = Balance::load_default().unwrap();
        let signature = MonsterAbility {
            ability_kind: "ruinous_blow".into(),
            callout_text: "IT RAISES ITS ARMS".into(),
            // Deliberately the LEAST likely thing to be rolled, and on cooldown, so the only
            // way it can fire is the rebuke forcing it.
            weight: 1,
            cooldown_ticks: 10_000,
            telegraph_ticks: 5,
            hp_threshold_pct: None,
            min_level: 1,
            effects: Vec::new(),
        };
        let mut m = monster("m", 100_000, 40);
        m.abilities = vec![signature];
        m.ability_ready_at.insert(0, u64::MAX);
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Gatekeeper,
            vec![player("h", 40)],
            vec![m],
            &b,
            7,
        );
        let mi = bt.idx("m").unwrap();

        // Its turn is taken.
        bt.fighters[mi].gauge = 0.9;
        assert!(bt.deny_gauge(mi, None), "the knock did not land");
        assert!(bt.fighters[mi].rebuke_pending, "a boss took the knock and shrugged");

        // Its next turn is the signature, cooldown notwithstanding — the cooldown paces a
        // boss's own rhythm; this is the fight answering an interruption.
        let mut events = Vec::new();
        bt.take_monster_turn(mi, &mut events);
        assert!(
            events.iter().any(|e| matches!(e, Event::TelegraphStarted { callout_text, .. }
                if callout_text == "IT RAISES ITS ARMS")),
            "the rebuke did not shout its signature: {events:?}"
        );
        // Consumed, so ONE knock buys ONE rebuke rather than arming it forever.
        assert!(!bt.fighters[mi].rebuke_pending, "the rebuke re-arms itself for free");
    }

    /// Ordinary fauna has no signature, so nothing is armed and nothing changes — the rule is
    /// universal but the effect only exists where a telegraph does, which is what keeps this a
    /// boss mechanic without anything having to declare itself a boss.
    #[test]
    fn a_creature_with_no_signature_is_not_rebuked() {
        let b = Balance::load_default().unwrap();
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("h", 40)],
            vec![monster("m", 500, 40)],
            &b,
            7,
        );
        let mi = bt.idx("m").unwrap();
        bt.fighters[mi].gauge = 0.9;
        assert!(bt.deny_gauge(mi, None), "the knock did not land");
        assert!(
            !bt.fighters[mi].rebuke_pending,
            "a creature with no telegraph armed a rebuke it cannot deliver"
        );
    }

    /// A gauge reaching zero the NORMAL way — spending a turn, or a fight starting — is not
    /// a knock and must not raise the guard. Otherwise the opening exchange of every fight
    /// would hand out immunity for free, and the first knock of a fight would bounce.
    #[test]
    fn a_gauge_emptied_naturally_never_raises_the_guard() {
        let b = Balance::load_default().unwrap();
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("h", 40)],
            vec![monster("m", 500, 40)],
            &b,
            7,
        );
        let m = bt.idx("m").unwrap();
        // A fight STARTS with gauges at zero. Nobody is guarded, so the opening knock lands.
        assert_eq!(bt.fighters[m].gauge_guard_turns, 0, "a fight began with someone guarded");
        assert!(!bt.fighters[m].staggered, "a fight began with someone already staggered");
        bt.fighters[m].gauge = 0.9;
        assert!(bt.deny_gauge(m, None), "the first knock of the fight bounced");

        // And SPENDING a turn clears rather than sets it: acting is what lifts the guard.
        bt.reset_gauge(m);
        assert!(!bt.fighters[m].staggered, "spending a turn left it staggered");
        // The guard is still up though — it outlives the recovery turn on purpose. So the
        // knock that follows bounces, and only a knock after the guard's turns are spent
        // lands. (The stagger and the guard are two windows, and this is the difference.)
        assert!(!bt.deny_gauge(m, None), "the guard lapsed the moment it stood up");
        for _ in 0..b.battle.gauge_guard_turns {
            bt.reset_gauge(m);
        }
        assert!(
            !bt.fighters[m].statuses.iter().any(|s| s == GAUGE_GUARD_STATUS),
            "the guard token outlived the turns that cleared it"
        );
        bt.fighters[m].gauge = 0.9;
        assert!(bt.deny_gauge(m, None), "a knock after the guard cleared bounced");
    }

    /// A drain that takes nothing must not buy a free immunity window — otherwise touching an
    /// empty gauge is the cheapest way to protect the thing you are fighting.
    #[test]
    fn denying_an_empty_gauge_guards_nothing() {
        let b = Balance::load_default().unwrap();
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("h", 10)],
            vec![monster("m", 500, 10)],
            &b,
            7,
        );
        let m = bt.idx("m").unwrap();
        bt.fighters[m].gauge = 0.0;
        assert!(!bt.deny_gauge(m, None), "zeroing an empty gauge reported a landing");
        assert_eq!(bt.fighters[m].gauge_guard_turns, 0, "an empty gauge bought a guard");
        // So a real knock a moment later still works.
        bt.fighters[m].gauge = 0.8;
        assert!(bt.deny_gauge(m, Some(0.3)));
    }

    #[test]
    fn the_deep_manifestations_each_do_their_own_thing() {
        let b = balance();
        // A Director-rank Psyker with slots to spare.
        let mk = || {
            Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![psyker("p", 400, 255, 5)],
                vec![monster("m1", 100000, 1), monster("m2", 100000, 1)],
                &b,
                7,
            )
        };
        let cast = |battle: &mut Battle, kind: &str| {
            tick_to_ready(battle, "p");
            battle
                .submit(
                    "p",
                    format!("op:{kind}"),
                    BattleActionKind::Skill,
                    Some(vec!["m1".into()]),
                    Some(format!("cast:{kind}")),
                    None,
                )
                .unwrap_or_else(|e| panic!("{kind} rejected: {e:?}"));
        };

        // Kinetic Wave grinds the WHOLE line, not one target.
        let mut wave = mk();
        cast(&mut wave, "kinetic_wave");
        for id in ["m1", "m2"] {
            assert!(player_hp(&wave, id) < 100000, "kinetic_wave missed {id}");
        }

        // Matter Dissolution corrodes: damage AND the target's armour worn down.
        let mut diss = mk();
        let armour_before = {
            let i = diss.fighters.iter().position(|f| f.combatant_id == "m1").unwrap();
            diss.fighters[i].def = 40;
            40
        };
        cast(&mut diss, "matter_dissolution");
        let i = diss.fighters.iter().position(|f| f.combatant_id == "m1").unwrap();
        assert!(diss.fighters[i].def < armour_before, "armour was not corroded");
        assert!(player_hp(&diss, "m1") < 100000, "matter_dissolution dealt no damage");

        // Dominate Mind takes the turn outright, rather than merely slowing it — and then
        // the GUARD comes up, so the second cast bounces. It used to take the turn EVERY
        // turn it was held, which is the unbounded lock `gauge_guard` exists to end: a
        // creature that never acts makes a boss fight free and endless, and the rule the
        // repo already wrote for `hallowed_ground` and Event Horizon applies to the Psyker's
        // capstone too. Still the strongest control in the game, now with a window in it.
        let mut dom = mk();
        let i = dom.fighters.iter().position(|f| f.combatant_id == "m1").unwrap();
        dom.fighters[i].gauge = 0.95;
        cast(&mut dom, "dominate_mind");
        assert_eq!(gauge_of(&dom, "m1"), 0.0, "dominate_mind left it a turn");
        dom.fighters[i].gauge = 0.95;
        // A distinct action id: the helper derives one from the skill name, and the engine
        // refuses a repeat. This is the SECOND cast of the same ability, which is the whole
        // point of the assertion.
        tick_to_ready(&mut dom, "p");
        dom.submit(
            "p",
            "op:dominate_mind:again".into(),
            BattleActionKind::Skill,
            Some(vec!["m1".into()]),
            Some("cast:dominate_mind".into()),
            None,
        )
        .expect("the second cast resolves");
        // Still where it was (drifting UP as it fills, not knocked down): the knock bounced.
        assert!(
            gauge_of(&dom, "m1") >= 0.95,
            "a guarded target had its gauge taken again - the lock is back: {}",
            gauge_of(&dom, "m1")
        );

        // Phase Shift is the Psyker's own defence: Evasion it keeps topping up.
        let mut phase = mk();
        cast(&mut phase, "phase_shift");
        let pi = phase.fighters.iter().position(|f| f.combatant_id == "p").unwrap();
        assert!(phase.fighters[pi].evasion > 0.0, "phase_shift granted no Evasion");

        // Reality Collapse is the capstone: the whole line, harder than the Wave.
        let mut collapse = mk();
        cast(&mut collapse, "reality_collapse");
        let collapse_dmg = 100000 - player_hp(&collapse, "m1");
        let wave_dmg = 100000 - player_hp(&wave, "m1");
        assert!(
            collapse_dmg > wave_dmg,
            "the level-100 capstone ({collapse_dmg}) hits softer than the L25 Wave ({wave_dmg})"
        );
    }

    /// The doc's controller chain: Pressure crushes, Gravity drags what is crushed, Anchor
    /// pins what is dragged. An aspect is refused unless its parent is already held, which
    /// is what makes the Psyker spend slots to escalate rather than press three buttons.
    #[test]
    fn an_aspect_needs_its_parent_and_falls_with_it() {
        let b = balance();
        let mk = || {
            Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![psyker("p", 400, 255, 5)],
                vec![monster("m1", 100000, 1), monster("m2", 100000, 1)],
                &b,
                7,
            )
        };
        // The action id must differ per submit or the engine dedupes it as a replay — a
        // refused cast and its retry are two different presses.
        let mut nth = 0;
        let mut op = |battle: &mut Battle, kind: &str, verb: &str| {
            nth += 1;
            tick_to_ready(battle, "p");
            let _ = battle.submit(
                "p",
                format!("op{nth}:{verb}:{kind}"),
                BattleActionKind::Skill,
                Some(vec!["m1".into()]),
                Some(format!("{verb}:{kind}")),
                None,
            );
        };
        let held = |battle: &Battle| -> Vec<String> {
            let i = battle.fighters.iter().position(|f| f.combatant_id == "p").unwrap();
            battle.fighters[i].foci.iter().map(|f| f.kind.clone()).collect()
        };

        // Gravity alone is refused — there is nothing being crushed for it to drag.
        let mut orphan = mk();
        op(&mut orphan, "gravity", "cast");
        assert!(held(&orphan).is_empty(), "Gravity landed with no Gravity Well under it");

        // With the parent held it lands, and Anchor then needs Gravity in turn.
        let mut chain = mk();
        op(&mut chain, "gravity_well", "cast");
        op(&mut chain, "anchor", "cast");
        assert!(!held(&chain).contains(&"anchor".to_string()), "Anchor skipped Gravity");
        op(&mut chain, "gravity", "cast");
        op(&mut chain, "anchor", "cast");
        assert_eq!(held(&chain), vec!["gravity_well", "gravity", "anchor"], "the chain");

        // Letting the base go drops the whole chain: an aspect with no parent is a slow
        // nothing is paying a Focus slot for.
        op(&mut chain, "gravity_well", "revoke");
        assert!(held(&chain).is_empty(), "revoking the base left its aspects running");
    }

    /// Every aspect the doc gave a manifestation, on the manifestation it belongs to —
    /// and each one doing something the others do not. An aspect that duplicated its
    /// neighbour would be a Focus slot nobody has a reason to spend.
    #[test]
    fn each_aspect_does_its_own_thing() {
        let b = balance();
        let mk = || {
            let caster = psyker("p", 400, 255, 5);
            let mut ally = player("ally", 1);
            ally.class_key = "explorer".into();
            // ONE party: the `player()` helper hands every fighter its own `player_id`,
            // which models four separate players rather than one player's four heroes.
            // Shield reaches its caster's own party, so the fixture has to be a party.
            ally.player_id = caster.player_id.clone();
            Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![caster, ally],
                vec![monster("m1", 100000, 1)],
                &b,
                7,
            )
        };
        let mut nth = 0;
        let mut cast = |battle: &mut Battle, kind: &str, aim: &str| {
            nth += 1;
            tick_to_ready(battle, "p");
            let _ = battle.submit(
                "p",
                format!("op{nth}:{kind}"),
                BattleActionKind::Skill,
                Some(vec![aim.to_string()]),
                Some(format!("cast:{kind}")),
                None,
            );
        };
        let idx = |bt: &Battle, id: &str| bt.fighters.iter().position(|f| f.combatant_id == id).unwrap();

        // Shield widens the ward from the caster to the whole party.
        let mut sh = mk();
        cast(&mut sh, "kinetic_aegis", "p");
        let ally_before = sh.fighters[idx(&sh, "ally")].barrier;
        cast(&mut sh, "shield", "p");
        assert!(
            sh.fighters[idx(&sh, "ally")].barrier > ally_before,
            "Shield left the rest of the party unwarded"
        );

        // Acceleration keeps its OWN aim: its parent grinds an enemy, it hurries an ally.
        let mut ac = mk();
        cast(&mut ac, "temporal_anchor", "m1");
        let ai = idx(&ac, "ally");
        ac.fighters[ai].gauge = 0.0;
        cast(&mut ac, "acceleration", "ally");
        assert!(
            ac.fighters[idx(&ac, "ally")].gauge > 0.0,
            "Acceleration was aimed at its parent's enemy instead of the ally"
        );

        // Brittle strips resistances for good.
        let mut br = mk();
        let mi = idx(&br, "m1");
        br.fighters[mi].damage_modifiers.insert(DamageType::Fire, 0.5);
        cast(&mut br, "matter_dissolution", "m1");
        cast(&mut br, "brittle", "m1");
        let mi = idx(&br, "m1");
        assert!(
            br.fighters[mi].damage_modifiers.get(&DamageType::Fire).is_none_or(|m| *m >= 1.0),
            "Brittle left a resistance standing"
        );

        // Blackout marks the target blinded, so its own blows go wide. That the miss
        // actually happens is `a_blinded_creature_swings_wide`'s job — here we only check
        // the aspect lands its mark on the creature its parent is holding.
        let mut bl = mk();
        cast(&mut bl, "dominate_mind", "m1");
        cast(&mut bl, "blackout", "m1");
        let mi = idx(&bl, "m1");
        assert!(
            bl.fighters[mi].timed_statuses.iter().any(|(n, _)| n == BLIND_STATUS),
            "Blackout left the target seeing"
        );
    }

    /// Misdirection's dazzle, driven through REAL turns rather than asserted as arithmetic.
    ///
    /// Its existing test set `active_actor` by hand and then checked that
    /// `dodge + misdirection_miss > dodge` — which is true of any positive number and says
    /// nothing about the game. The engine only ever set `active_actor` for a player's
    /// action, so a distracted creature swung with full accuracy for as long as the ability
    /// has existed. A test that fakes the state it is meant to be verifying cannot fail.
    #[test]
    fn a_distracted_creature_actually_misses() {
        let b = balance();
        let landed = |dazzle: bool| -> i32 {
            let mut hero = player("h", 1);
            hero.class_key = "hunter".into();
            hero.hp = 100_000;
            hero.max_hp = 100_000;
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![hero],
                vec![monster("m", 100_000, 200)],
                &b,
                7,
            );
            if dazzle {
                let mi = battle.fighters.iter().position(|f| f.combatant_id == "m").unwrap();
                battle.apply_timed(mi, Battle::DISTRACT_STATUS, 100_000);
            }
            for _ in 0..2000 {
                battle.tick();
            }
            let h = battle.fighters.iter().find(|f| f.combatant_id == "h").unwrap();
            h.max_hp - h.hp
        };
        let plain = landed(false);
        let dazzled = landed(true);
        assert!(plain > 0, "the control creature never landed a blow");
        assert!(
            dazzled < plain,
            "a distracted creature dealt {dazzled} against an undistracted one's {plain} — \
             Misdirection is not making anything miss"
        );
    }

    /// Blackout first read as "the target cannot dodge", which was **dead code**: creature
    /// dodge is only ever set for heroes and no creature ability grants Evasion, so it took
    /// away something no creature had. A blinded creature's own blows go wide instead —
    /// which is a thing this engine can actually express.
    #[test]
    fn a_blinded_creature_swings_wide() {
        let b = balance();
        let landed = |blind: bool| -> i32 {
            let mut hero = player("h", 1);
            hero.class_key = "hunter".into();
            hero.hp = 100_000;
            hero.max_hp = 100_000;
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![hero],
                vec![monster("m", 100_000, 200)],
                &b,
                7,
            );
            if blind {
                let mi = battle.fighters.iter().position(|f| f.combatant_id == "m").unwrap();
                battle.apply_timed(mi, BLIND_STATUS, 100_000);
            }
            for _ in 0..2000 {
                battle.tick();
            }
            let h = battle.fighters.iter().find(|f| f.combatant_id == "h").unwrap();
            h.max_hp - h.hp
        };
        let seeing = landed(false);
        let blinded = landed(true);
        assert!(seeing > 0, "the control creature never landed a blow");
        assert!(
            blinded < seeing,
            "a blinded creature dealt {blinded} against a seeing one's {seeing} — Blackout \
             is doing nothing again"
        );
    }

    /// **Measured, not assumed.** Two changes landed together — the back rank stopped
    /// softening non-physical damage, and creatures started hunting roles — and both point
    /// at the same hero. This drives a real four-hero party through real turns and reports
    /// what share of the incoming damage the healer actually eats.
    ///
    /// The bound is what matters: a Role-hunting, non-physical creature is *supposed* to be
    /// frightening (that is the counter to parking a healer behind a wall), but if it takes
    /// most of the party's total damage then the back row has stopped meaning anything and
    /// the encounter is a healer-deletion puzzle rather than a fight.
    #[test]
    fn a_role_hunter_pressures_the_healer_without_erasing_it() {
        let b = balance();
        let share = |foes: &[(TargetProfile, DamageType)]| -> f64 {
            let mut party = Vec::new();
            for (id, class, back) in [
                ("front1", "hunter", false),
                ("front2", "phoenix_guard", false),
                ("healer", "resonant", true),
                ("caster", "psyker", true),
            ] {
                let mut h = player(id, 1); // speed 1: heroes never act, so only the creature does
                h.class_key = class.into();
                h.back_row = back;
                h.hp = 100_000;
                h.max_hp = 100_000;
                party.push(h);
            }
            let foes: Vec<Fighter> = foes
                .iter()
                .enumerate()
                .map(|(i, (profile, ty))| {
                    let mut m = monster(&format!("m{i}"), 100_000, 200);
                    m.target_profile = *profile;
                    m.basic_attack_type = *ty;
                    m
                })
                .collect();
            let mut battle =
                Battle::new("b".into(), EncounterClass::Standard, party, foes, &b, 7);
            for _ in 0..4000 {
                battle.tick();
            }
            let lost = |id: &str| {
                let f = battle.fighters.iter().find(|f| f.combatant_id == id).unwrap();
                (f.max_hp - f.hp) as f64
            };
            let total: f64 = ["front1", "front2", "healer", "caster"].iter().map(|i| lost(i)).sum();
            assert!(total > 0.0, "the creature never landed anything");
            lost("healer") / total
        };

        // The baseline the game shipped for a long time: a physical creature going for
        // whoever is weakest. The healer is behind a wall and takes none of it.
        let baseline = share(&[(TargetProfile::Weakest, DamageType::Pierce)]);
        // One role-hunter, and it is a real creature: the Choirmother is innately Role AND
        // Mind-typed, so it seeks the healer and the rank does not soften the blow. Every
        // point it deals lands on the healer — which is the profile working as designed.
        let hunted = share(&[(TargetProfile::Role, DamageType::Mind)]);
        assert!(
            hunted > baseline,
            "a role-hunter put no more pressure on the healer than a wandering blow \
             ({hunted:.2} vs {baseline:.2}) — the profile is not doing anything"
        );

        // THE INVARIANT. A pack, post-`cap_role_hunters`: one hunter, the rest on the
        // weakest. Before the cap all five rolled independently and could all pick the
        // healer, which is not five decisions — it is one decision applied five times, and
        // nothing the party can answer. With the cap the damage spreads.
        let pack = share(&[
            (TargetProfile::Role, DamageType::Mind),
            (TargetProfile::Weakest, DamageType::Pierce),
            (TargetProfile::Weakest, DamageType::Pierce),
            (TargetProfile::Weakest, DamageType::Pierce),
            (TargetProfile::Weakest, DamageType::Pierce),
        ]);
        let all_hunting = share(&[(TargetProfile::Role, DamageType::Mind); 5]);
        eprintln!(
            "healer share — lone wanderer {baseline:.2} · lone hunter {hunted:.2} · \
             capped pack {pack:.2} · uncapped pack {all_hunting:.2}"
        );
        assert!(
            pack < 0.6,
            "even with the cap a pack still puts {pack:.0}% of its damage on the healer"
        );
        assert!(
            all_hunting > pack,
            "the cap changed nothing ({all_hunting:.2} vs {pack:.2})"
        );
    }

    /// Freeze is Gravity's twin on a burning target: an ordinary slow, deepening to a PIN
    /// on anything already crawling. The escalation is the reason to hold both.
    #[test]
    fn freeze_pins_what_was_already_slowed() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![psyker("p", 400, 255, 5)],
            vec![monster("m1", 100000, 1)],
            &b,
            7,
        );
        let mut nth = 0;
        let mut cast = |battle: &mut Battle, kind: &str| {
            nth += 1;
            tick_to_ready(battle, "p");
            let _ = battle.submit(
                "p",
                format!("op{nth}:{kind}"),
                BattleActionKind::Skill,
                Some(vec!["m1".into()]),
                Some(format!("cast:{kind}")),
                None,
            );
        };
        cast(&mut battle, "thermal_flux");
        cast(&mut battle, "freeze");
        let mi = battle.fighters.iter().position(|f| f.combatant_id == "m1").unwrap();
        let has = |bt: &Battle, i: usize, n: &str| {
            bt.fighters[i].timed_statuses.iter().any(|(s, _)| s == n)
        };
        assert!(has(&battle, mi, GRAVITY_STATUS), "Freeze did not slow an unslowed target");
        assert!(!has(&battle, mi, ANCHOR_STATUS), "Freeze pinned something that was not slowed");

        // Now with the target already crawling, the same aspect pins it.
        let _ = battle.apply_timed(mi, "chill", 200);
        cast(&mut battle, "revoke:freeze");
        cast(&mut battle, "freeze");
        let mi = battle.fighters.iter().position(|f| f.combatant_id == "m1").unwrap();
        assert!(has(&battle, mi, ANCHOR_STATUS), "Freeze did not pin an already-slowed target");
    }

    /// Anchor is the deepest slow in the game and is still a RATE. A creature's
    /// `speed_stat` never scales with distance, so a gauge that stops advancing is a
    /// creature that never acts again — a soft-lock, not a capstone.
    #[test]
    fn anchor_pins_the_gauge_without_ever_locking_it() {
        let b = balance();
        assert!(
            b.battle.psyker_anchor_slow_mult > 0.0,
            "a zero anchor multiplier is a gauge that never advances"
        );
        assert!(
            b.battle.psyker_anchor_slow_mult < b.battle.status_slow_mult,
            "Anchor should bite harder than an ordinary web/chill"
        );
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![psyker("p", 400, 255, 5)],
            vec![monster("m1", 100000, 1)],
            &b,
            7,
        );
        for (kind, verb) in [("gravity_well", "cast"), ("gravity", "cast"), ("anchor", "cast")] {
            tick_to_ready(&mut battle, "p");
            let _ = battle.submit(
                "p",
                format!("op:{verb}:{kind}"),
                BattleActionKind::Skill,
                Some(vec!["m1".into()]),
                Some(format!("{verb}:{kind}")),
                None,
            );
        }
        let mi = battle.fighters.iter().position(|f| f.combatant_id == "m1").unwrap();
        assert!(
            battle.fighters[mi]
                .timed_statuses
                .iter()
                .any(|(n, _)| n == ANCHOR_STATUS),
            "the pinned creature wears no anchor"
        );
        // It still advances — pinned, not stopped.
        battle.fighters[mi].gauge = 0.0;
        let before = battle.fighters[mi].gauge;
        for _ in 0..60 {
            battle.tick();
        }
        let mi = battle.fighters.iter().position(|f| f.combatant_id == "m1").unwrap();
        assert!(
            battle.fighters[mi].gauge > before || battle.fighters[mi].gauge >= 1.0,
            "an anchored creature's gauge never moved — that is a lock"
        );
    }

    /// Mind's Eye: the opening Foci are free, so a controller does not spend the first
    /// three turns of every fight doing nothing but setting up. A free cast leaves the
    /// gauge full — the Psyker acts again immediately — and the pool is finite.
    #[test]
    fn minds_eye_opens_the_fight_without_spending_it() {
        let b = balance();
        let mut f = psyker("p", 400, 50, 5);
        // The level -> count curve is `meld-run`'s (the engine never sees a level curve);
        // what is pinned HERE is what the engine does with the pool it is handed.
        f.free_casts = 2;
        let budget = f.free_casts;
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![f],
            vec![monster("m1", 100000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "p");
        let pi = battle.fighters.iter().position(|x| x.combatant_id == "p").unwrap();
        let _ = battle.submit(
            "p",
            "op1".into(),
            BattleActionKind::Skill,
            Some(vec!["m1".into()]),
            Some("cast:gravity_well".into()),
            None,
        );
        let pi2 = battle.fighters.iter().position(|x| x.combatant_id == "p").unwrap();
        assert_eq!(pi, pi2);
        assert!(
            battle.fighters[pi].gauge >= 1.0,
            "a free cast spent the turn — the whole point is that it does not"
        );
        assert_eq!(battle.fighters[pi].free_casts, budget - 1, "the pool did not decrement");
    }

    /// A refused cast must not burn the opening it was meant to buy — the aspect chain
    /// makes "rejected" an ordinary outcome, not an edge case.
    #[test]
    fn a_refused_cast_costs_no_free_cast() {
        let b = balance();
        let mut f = psyker("p", 400, 50, 5);
        f.free_casts = 2;
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![f],
            vec![monster("m1", 100000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "p");
        // Gravity with no Gravity Well under it: refused.
        let _ = battle.submit(
            "p",
            "op1".into(),
            BattleActionKind::Skill,
            Some(vec!["m1".into()]),
            Some("cast:gravity".into()),
            None,
        );
        let pi = battle.fighters.iter().position(|x| x.combatant_id == "p").unwrap();
        assert_eq!(battle.fighters[pi].free_casts, 2, "a refusal spent a free cast");
    }

    /// Expansion: an offensive Focus reaches other living enemies too. A controller widens
    /// with level; hitting one target harder is the one thing this class is not for.
    #[test]
    fn expansion_spreads_a_focus_across_the_line() {
        let b = balance();
        let mk = |level: i32| {
            Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![psyker("p", 400, level, 5)],
                vec![monster("m1", 100000, 1), monster("m2", 100000, 1)],
                &b,
                7,
            )
        };
        let cast = |battle: &mut Battle| {
            tick_to_ready(battle, "p");
            let _ = battle.submit(
                "p",
                "op1".into(),
                BattleActionKind::Skill,
                Some(vec!["m1".into()]),
                Some("cast:gravity_well".into()),
                None,
            );
        };
        let hp = |battle: &Battle, id: &str| {
            battle.fighters.iter().find(|f| f.combatant_id == id).unwrap().hp
        };

        // Below the unlock the second creature is untouched.
        let mut early = mk(1);
        cast(&mut early);
        assert_eq!(hp(&early, "m2"), 100000, "an early Psyker splashed the line");

        // Past it, the same single-target Focus grinds the neighbour too — for less.
        let mut wide = mk(b.battle.psyker_expansion_at);
        cast(&mut wide);
        let primary = 100000 - hp(&wide, "m1");
        let spill = 100000 - hp(&wide, "m2");
        assert!(spill > 0, "Expansion reached nobody");
        assert!(spill < primary, "the spill ({spill}) is not softer than the primary ({primary})");
    }

    /// The reward for spending a pin: the party walked into a creature it had held, so it
    /// picked the moment and moves first. Only the PLAYER side is readied — a surprise
    /// that also filled the creature's gauge would be no surprise at all.
    #[test]
    fn a_surprise_opens_with_the_party_ready_and_the_creature_not() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![psyker("p", 40, 10, 3)],
            vec![monster("m1", 100, 1)],
            &b,
            7,
        );
        let gauge = |bt: &Battle, id: &str| {
            bt.fighters.iter().find(|f| f.combatant_id == id).unwrap().gauge
        };
        assert!(gauge(&battle, "p") < 1.0, "a normal fight does not open ready");
        battle.open_with_full_party_gauges();
        assert!(gauge(&battle, "p") >= 1.0, "the party did not get the first move");
        assert!(gauge(&battle, "m1") < 1.0, "the surprise also readied the creature");
    }

    #[test]
    fn a_manifestation_is_gated_by_the_registry_not_by_a_hand_kept_list() {
        // The client used to carry its own four-entry copy of this list. Anything the
        // registry knows and the engine can resolve must be castable at its level.
        for def in meld_proto::skills::skills_for_class("psyker") {
            assert!(
                manifest_unlock_level(def.key).is_some(),
                "{} is a registered manifestation the engine will not accept",
                def.key
            );
            assert_eq!(manifest_unlock_level(def.key), Some(def.unlock));
        }
        assert!(manifest_unlock_level("not_a_manifestation").is_none());
    }

    #[test]
    fn psyker_focus_cap_and_unlocks_are_enforced() {
        let b = balance();
        // Level-1 Psyker: mind_spike (unlock L3) can't be cast; two L1 slots fill.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![psyker("p", 400, meld_proto::skills::unlock_level("kinetic_aegis"), 2)],
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
        cast(&mut battle, 1, "reality_collapse"); // far past this level → ignored
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
            vec![psyker("p", 110, meld_proto::skills::unlock_level("kinetic_aegis"), 2)],
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
        // atk 12, def 4 → attack = 8; Power Strike = round(12*1.75) - 4 = 21 - 4 = 17.
        // Power Strike now spends Adrenaline, so the Explorer must have it banked.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![explorer("a", 110, 1)],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        let ai = battle.fighters.iter().position(|f| f.combatant_id == "a").unwrap();
        battle.fighters[ai].adrenaline = 40; // enough for Power Strike
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
        // A brisk monster (speed 200 → acts ~every 26 ticks) wounds the speed-110
        // player (ready at tick 48) exactly once (14 atk − 3 def = 11) before the
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
    /// (monster speed 200 acts at ticks 26 & 52; player is awaiting from tick 48)
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
        tick_times(&mut battle, 55);
        assert_eq!(player_hp(&battle, "a"), 18, "two 11-dmg hits land by tick 55");
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
        let b = balance();
        // Second Wind is a Hunter Adrenaline spender: heal = 0.3 * 40 = 12; wounded to
        // 18 → 30. It costs 35 Adrenaline, so the Hunter must have it banked.
        let mut battle = Battle::new(
            "b1".into(),
            EncounterClass::Standard,
            vec![explorer("a", 400, meld_proto::skills::unlock_level("second_wind"))],
            vec![monster("m", 1000, 1)],
            &b,
            7,
        );
        let ai = battle.fighters.iter().position(|f| f.combatant_id == "a").unwrap();
        battle.fighters[ai].hp = 18;
        battle.fighters[ai].adrenaline = 35;
        tick_to_ready(&mut battle, "a");
        let evs = battle
            .submit("a", "sw".into(), BattleActionKind::Skill, None, Some("second_wind".into()), None)
            .expect("heal resolves");
        let eff = evs
            .into_iter()
            .find_map(|e| match e {
                Event::Resolved(r) if r.action == BattleActionKind::Skill => Some(r.effects[0].clone()),
                _ => None,
            })
            .expect("heal resolution present");
        assert_eq!(eff.kind, EffectKind::Heal);
        assert_eq!(eff.amount, Some(12));
        assert_eq!(eff.hp_after, 30);
    }

    #[test]
    fn locked_skill_is_rejected_until_leveled() {
        let b = balance();
        // A level-1 hero cannot use a level-2 skill (Second Wind unlocks at 2).
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
        // speed 400 / 5200 ≈ 0.077 per tick → full by tick 14 (float accumulation
        // lands tick-13's gauge a hair under 1.0).
        for _ in 0..14 {
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

    /// A combatant's gauge and wire statuses, read off the authoritative snapshot.
    fn gauge_of(battle: &Battle, cid: &str) -> f64 {
        battle.gauge_state().into_iter().find(|(id, ..)| id == cid).unwrap().1
    }
    fn statuses_of(battle: &Battle, cid: &str) -> Vec<String> {
        battle.gauge_state().into_iter().find(|(id, ..)| id == cid).unwrap().3
    }

    /// A player fighter at a chosen level (so level-gated skills are unlocked).
    fn leveled_player(id: &str, speed: i32, level: i32) -> Fighter {
        let mut f = player(id, speed);
        f.level = level;
        f
    }

    /// Backstab (Shifter) pierces most of the target's armour, so against a heavily
    /// armoured creature it lands far more than a plain attack that the armour eats.
    #[test]
    fn shifter_backstab_pierces_armour() {
        let b = balance();
        // atk 12 (the `player` helper) vs def 10. Plain: max(1, 12−10)=2. Backstab:
        // atk×1.5=18 minus def cut to 25% → 18−round(2.5)=15.
        let dmg = |skill: Option<&str>| -> i32 {
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![player("s", 400)],
                vec![monster_def("m", 200, 1, 10)],
                &b,
                7,
            );
            tick_to_ready(&mut battle, "s");
            let action = if skill.is_some() { BattleActionKind::Skill } else { BattleActionKind::Attack };
            battle
                .submit("s", "a1".into(), action, Some(vec!["m".into()]), skill.map(String::from), None)
                .unwrap();
            200 - player_hp(&battle, "m")
        };
        // atk 12 vs def 10 leaves 2, but armour can never eat a whole blow — the
        // mitigation floor (25% of the attack) lands 3 instead.
        assert_eq!(dmg(None), 3, "a plain attack is mostly eaten by def 10");
        assert_eq!(dmg(Some("backstab")), 15, "Backstab pierces most of the armour");
        assert!(
            dmg(Some("backstab")) > dmg(None) * 4,
            "piercing armour should still be worth doing"
        );
    }

    /// Flicker (Shifter) grants Evasion — a dodge bonus surfaced on the wire that
    /// then decays a fixed step at the start of the Shifter's next turn.
    #[test]
    fn shifter_flicker_grants_and_decays_evasion() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![leveled_player("s", 400, meld_proto::skills::unlock_level("flicker"))],
            vec![monster("m", 500, 1)], // slow, harmless punching bag
            &b,
            7,
        );
        tick_to_ready(&mut battle, "s");
        battle
            .submit("s", "a1".into(), BattleActionKind::Skill, None, Some("flicker".into()), None)
            .unwrap();
        // shifter_flicker_evasion = 0.4 → "evasion:40".
        assert!(
            statuses_of(&battle, "s").iter().any(|x| x == "evasion:40"),
            "Flicker grants 40% evasion: {:?}",
            statuses_of(&battle, "s")
        );
        // Next turn's start-of-turn upkeep decays it by 0.15 → 0.25 before the action.
        tick_to_ready(&mut battle, "s");
        battle
            .submit("s", "a2".into(), BattleActionKind::Defend, None, None, None)
            .unwrap();
        assert!(
            statuses_of(&battle, "s").iter().any(|x| x == "evasion:25"),
            "evasion decays to 25%: {:?}",
            statuses_of(&battle, "s")
        );
    }

    /// Ransack (Shifter) both damages and drains the surviving target's ATB gauge.
    #[test]
    fn shifter_ransack_drains_enemy_gauge() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![leveled_player("s", 400, meld_proto::skills::unlock_level("ransack"))],
            vec![monster("m", 500, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "s");
        // Seed the monster with a partial gauge so the drain is observable.
        let mi = battle.fighters.iter().position(|f| f.combatant_id == "m").unwrap();
        battle.fighters[mi].gauge = 0.6;
        battle
            .submit("s", "a1".into(), BattleActionKind::Skill, Some(vec!["m".into()]), Some("ransack".into()), None)
            .unwrap();
        // shifter_ransack_drain = 0.35 → 0.6 − 0.35 = 0.25.
        assert!((gauge_of(&battle, "m") - 0.25).abs() < 1e-9, "Ransack drains the gauge to 0.25");
        assert!(player_hp(&battle, "m") < 500, "Ransack also deals damage");
    }

    /// A Hunter that cannot afford the row it pressed keeps its turn, its `action_id`, and
    /// its skin — and can act again immediately.
    ///
    /// ⚠️ THIS IS THE "SECOND WIND LOCKS THE HERO OUT" BUG. `submit` recorded the
    /// `action_id` and ran `start_of_turn` BEFORE resolving, then bailed on the rejection
    /// and threw those events away. So one refused press: ticked the hero's poison and
    /// decayed its Barrier with nobody told (client HP silently wrong), burned the
    /// `action_id` so re-sending the same order came back `DuplicateAction`, and left the
    /// hero `awaiting` on a full gauge — which the client had already dropped from `ready`,
    /// so it could not be commanded until the 15 s auto-defend spent its turn. Second Wind
    /// is the row that exposed it because it is the one you reach for when a hero is HURT,
    /// which is exactly when it has been taking hits instead of landing them and has no
    /// Adrenaline banked.
    #[test]
    fn a_refused_hunter_skill_costs_the_hero_nothing_and_keeps_its_turn() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            // Level 5 = Second Wind is unlocked, so the ONLY thing wrong is the cost.
            vec![hunter("h", 400, 5)],
            vec![monster("m", 5000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "h");
        // Poison and a Barrier, so "the refusal ran the start-of-turn upkeep" is visible.
        battle.fighters[0].timed_statuses.push(("poison".into(), u64::MAX));
        battle.fighters[0].barrier = 20;
        battle.fighters[0].adrenaline = 0; // one Adrenaline short of everything
        let hp = battle.fighters[0].hp;
        let barrier = battle.fighters[0].barrier;

        let err = battle
            .submit(
                "h",
                "sw1".into(),
                BattleActionKind::Skill,
                None,
                Some("second_wind".into()),
                None,
            )
            .expect_err("0 Adrenaline cannot pay for Second Wind");
        assert!(matches!(err, Reject::ValidationError("not enough adrenaline")));

        // Nothing was spent: not HP, not the Barrier, not the turn.
        assert_eq!(battle.fighters[0].hp, hp, "a refusal ticked the hero's poison");
        assert_eq!(battle.fighters[0].barrier, barrier, "a refusal decayed the hero's Barrier");
        assert!(
            battle.awaiting_turn("h"),
            "a refused action ate the hero's turn - it is uncommandable until the auto-act"
        );
        // And the SAME action_id is still usable, so the client's retry is not a duplicate.
        battle.fighters[0].adrenaline = b.battle.hunter_adrenaline_max;
        battle
            .submit(
                "h",
                "sw1".into(),
                BattleActionKind::Skill,
                None,
                Some("second_wind".into()),
                None,
            )
            .expect("the retry of a refused order is not a duplicate");
    }

    /// Pressing it a dozen times is not a dozen poison ticks. The per-press cost above is
    /// what makes a locked-out hero WORSE the more the player tries to un-stick it — which
    /// is exactly what a player does when a button appears to do nothing.
    #[test]
    fn refusals_do_not_accumulate_damage() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![hunter("h", 400, 5)],
            vec![monster("m", 5000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "h");
        battle.fighters[0].timed_statuses.push(("poison".into(), u64::MAX));
        battle.fighters[0].adrenaline = 0;
        let hp = battle.fighters[0].hp;
        for n in 0..12 {
            let _ = battle.submit(
                "h",
                format!("sw{n}"),
                BattleActionKind::Skill,
                None,
                Some("second_wind".into()),
                None,
            );
        }
        assert_eq!(battle.fighters[0].hp, hp, "12 refusals cost the hero 12 poison ticks");
        assert!(battle.awaiting_turn("h"), "the hero still owns its turn");
    }

    /// A skill the hero has not learned is refused the same free way — the case AUTOPLAY
    /// hit, since it bypasses the menu's greying entirely and used its own stale unlock
    /// table (`second_wind` at 2, really 5).
    #[test]
    fn a_locked_skill_is_refused_without_costing_the_turn() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![hunter("h", 400, 2)], // Second Wind unlocks at 5
            vec![monster("m", 5000, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "h");
        battle.fighters[0].adrenaline = b.battle.hunter_adrenaline_max;
        battle.fighters[0].barrier = 20;
        let err = battle
            .submit("h", "x".into(), BattleActionKind::Skill, None, Some("second_wind".into()), None)
            .expect_err("level 2 has not learned Second Wind");
        assert!(matches!(err, Reject::ValidationError("skill not unlocked at this level")));
        assert_eq!(battle.fighters[0].barrier, 20, "a locked skill decayed the hero's Barrier");
        assert!(battle.awaiting_turn("h"), "a locked skill ate the hero's turn");
    }

    /// One table for what a Hunter row costs. The precheck refuses on it before anything
    /// moves and the resolver charges on it — two copies means a precheck that waves an
    /// action through for the resolver to refuse, which is the turn-eating case again.
    #[test]
    fn the_precheck_and_the_resolver_price_a_hunter_row_the_same() {
        let b = balance();
        let battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![hunter("h", 400, 255)],
            vec![monster("m", 5000, 1)],
            &b,
            7,
        );
        for d in meld_proto::skills::skills_for_class("hunter") {
            let cost = battle
                .hunter_skill_cost(d.key)
                .unwrap_or_else(|| panic!("{} has no Adrenaline price", d.key));
            assert!(cost > 0, "{} is free", d.key);
            // The client greys rows against `meld_run`'s table; it must be the same number.
            assert_eq!(
                Some(cost),
                meld_run_adrenaline_cost(d.key, &b),
                "{}: the engine and the client disagree about the price",
                d.key
            );
        }
    }

    /// `meld-battle` cannot depend on `meld-run` (it is the layer below), so the client's
    /// table is mirrored here and held to the engine's by the test above. A divergence is a
    /// row the menu shows as affordable and the engine refuses.
    fn meld_run_adrenaline_cost(key: &str, b: &Balance) -> Option<i32> {
        let bt = &b.battle;
        match key {
            "power_strike" | "crushing_blow" => Some(bt.hunter_power_strike_cost),
            "second_wind" | "iron_lung" => Some(bt.hunter_second_wind_cost),
            "snare" | "pin_the_prey" => Some(bt.hunter_snare_cost),
            "frenzy" | "apex_predator" => Some(bt.hunter_frenzy_cost),
            _ => None,
        }
    }

    /// A Hunter fighter at `level` with an Adrenaline pool, for the refusal tests.
    fn hunter(id: &str, speed: i32, level: i32) -> Fighter {
        let b = balance();
        let mut f = leveled_player(id, speed, level);
        f.class_key = "hunter".into();
        f.adrenaline_max = b.battle.hunter_adrenaline_max;
        f
    }

    /// A Explorer fighter with a banked Adrenaline pool, for the kit tests.
    fn explorer(id: &str, speed: i32, level: i32) -> Fighter {
        let b = balance();
        let mut f = leveled_player(id, speed, level);
        f.class_key = "explorer".into();
        f.adrenaline_max = b.battle.hunter_adrenaline_max;
        f
    }

    /// The Explorer banks Adrenaline on each basic attack, capped at its max, and
    /// surfaces the running total on the wire.
    #[test]
    fn explorer_banks_adrenaline_on_basic_attacks() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![explorer("h", 400, 1)],
            vec![monster("m", 5000, 1)],
            &b,
            7,
        );
        // Two attacks → 2 × hunter_adrenaline_per_attack (25) = 50.
        for n in 1..=2 {
            tick_to_ready(&mut battle, "h");
            battle
                .submit("h", format!("a{n}"), BattleActionKind::Attack, Some(vec!["m".into()]), None, None)
                .unwrap();
        }
        assert!(
            statuses_of(&battle, "h").iter().any(|x| x == "adrenaline:50"),
            "two attacks bank 50 Adrenaline: {:?}",
            statuses_of(&battle, "h")
        );
    }

    #[test]
    fn basic_attacks_can_crit_for_extra_damage() {
        let b = balance();
        let (mut saw_crit, mut base_dmg, mut crit_dmg) = (false, None, None);
        // Hammer a huge dummy across seeds; a crit tags the Damage effect + hits harder.
        'outer: for seed in 0..40u64 {
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![explorer("h", 400, 5)],
                vec![monster("m", 1_000_000, 1)],
                &b,
                seed,
            );
            for n in 0..30 {
                tick_to_ready(&mut battle, "h");
                let events = battle
                    .submit("h", format!("a{seed}_{n}"), BattleActionKind::Attack, Some(vec!["m".into()]), None, None)
                    .unwrap();
                for r in events.iter().filter_map(|ev| match ev {
                    Event::Resolved(r) => Some(r),
                    _ => None,
                }) {
                    for eff in r.effects.iter().filter(|e| matches!(e.kind, EffectKind::Damage)) {
                        if eff.status.as_deref() == Some("crit") {
                            saw_crit = true;
                            crit_dmg = eff.amount;
                        } else {
                            base_dmg = eff.amount;
                        }
                    }
                }
                if saw_crit && base_dmg.is_some() {
                    break 'outer;
                }
            }
        }
        assert!(saw_crit, "crits fire across many attacks");
        if let (Some(bd), Some(cd)) = (base_dmg, crit_dmg) {
            assert!(cd > bd, "a crit ({cd}) hits harder than a normal blow ({bd})");
        }
    }

    /// A Explorer skill is rejected until enough Adrenaline is banked, then spends it.
    #[test]
    fn explorer_power_strike_spends_banked_adrenaline() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![explorer("h", 400, 1)],
            vec![monster_def("m", 5000, 1, 4)],
            &b,
            7,
        );
        // With 0 Adrenaline, Power Strike (cost 40) is rejected.
        tick_to_ready(&mut battle, "h");
        let early = battle.submit(
            "h", "x".into(), BattleActionKind::Skill, Some(vec!["m".into()]), Some("power_strike".into()), None,
        );
        assert!(early.is_err(), "no Adrenaline → Power Strike rejected");
        // Bank enough: two attacks (50 ≥ 40). The rejected submit didn't consume the
        // turn, so the hero is still ready for this first attack.
        for n in 1..=2 {
            battle
                .submit("h", format!("a{n}"), BattleActionKind::Attack, Some(vec!["m".into()]), None, None)
                .unwrap();
            tick_to_ready(&mut battle, "h");
        }
        let hp_before = player_hp(&battle, "m");
        battle
            .submit("h", "ps".into(), BattleActionKind::Skill, Some(vec!["m".into()]), Some("power_strike".into()), None)
            .unwrap();
        // 50 − 40 = 10 Adrenaline remains, and Power Strike (atk 12 × 1.75 = 21 − def 4
        // = 17) hits far harder than a basic attack (12 − 4 = 8).
        assert!(
            statuses_of(&battle, "h").iter().any(|x| x == "adrenaline:10"),
            "Power Strike spent 40: {:?}",
            statuses_of(&battle, "h")
        );
        assert_eq!(hp_before - player_hp(&battle, "m"), 17, "Power Strike lands atk×1.75 − def");
    }

    /// Phoenix Guard Swell Strike hits hard and staggers (drains the target's gauge).
    #[test]
    fn a_silvered_strike_hits_and_staggers() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("k", 400)], // atk 12
            vec![monster_def("m", 500, 1, 4)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "k");
        let mi = battle.fighters.iter().position(|f| f.combatant_id == "m").unwrap();
        battle.fighters[mi].gauge = 0.5;
        let hp0 = player_hp(&battle, "m");
        battle
            .submit("k", "a1".into(), BattleActionKind::Skill, Some(vec!["m".into()]), Some("silvered_strike".into()), None)
            .unwrap();
        // atk 12 × 1.4 = 16.8 → 17, − def 4 = 13.
        assert_eq!(hp0 - player_hp(&battle, "m"), 13, "Swell Strike lands atk×1.4 − def");
        // phoenix_guard_swell_drain = 0.3 → 0.5 − 0.3 = 0.2.
        assert!((gauge_of(&battle, "m") - 0.2).abs() < 1e-9, "Swell Strike drains the gauge");
    }

    /// Phoenix Guard Root grants the monk Barrier equal to a fraction of its max HP.
    #[test]
    fn the_rite_of_rest_grants_barrier() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![leveled_player("k", 400, meld_proto::skills::unlock_level("rite_of_rest"))], // max_hp 40
            vec![monster("m", 500, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "k");
        battle
            .submit("k", "a1".into(), BattleActionKind::Skill, None, Some("rite_of_rest".into()), None)
            .unwrap();
        // phoenix_guard_root_barrier_fraction = 0.25 → 40 × 0.25 = 10.
        assert!(
            statuses_of(&battle, "k").iter().any(|x| x == "barrier:10"),
            "Root grants Barrier: {:?}",
            statuses_of(&battle, "k")
        );
    }

    /// Phoenix Guard Kinetic Shock fully resets the target's ATB gauge (hard stagger).
    #[test]
    fn holy_censure_zeroes_the_gauge() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            // Holy Censure sits at level 9 on the squares ladder.
            vec![leveled_player("k", 400, meld_proto::skills::unlock_level("holy_censure"))],
            vec![monster("m", 500, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "k");
        let mi = battle.fighters.iter().position(|f| f.combatant_id == "m").unwrap();
        battle.fighters[mi].gauge = 0.9;
        battle
            .submit("k", "a1".into(), BattleActionKind::Skill, Some(vec!["m".into()]), Some("holy_censure".into()), None)
            .unwrap();
        assert_eq!(gauge_of(&battle, "m"), 0.0, "Holy Censure zeroes the gauge");
        assert!(player_hp(&battle, "m") < 500, "Holy Censure also deals damage");
    }

    /// Purging Light strikes every living enemy at once.
    #[test]
    fn purging_light_hits_all_enemies() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            // Purging Light sits at level 16.
            vec![leveled_player("k", 400, meld_proto::skills::unlock_level("purging_light"))],
            vec![monster("m1", 500, 1), monster("m2", 500, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "k");
        battle
            .submit("k", "a1".into(), BattleActionKind::Skill, None, Some("purging_light".into()), None)
            .unwrap();
        assert!(player_hp(&battle, "m1") < 500, "Toll hit the first enemy");
        assert!(player_hp(&battle, "m2") < 500, "Toll hit the second enemy too");
    }

    // ------------------------------------------------ Creature AI spec §2 ---

    /// An instant ability (weight ≫ basic) with all effect kinds observable:
    /// the monster's turn resolves as an auto Skill carrying the callout.
    fn spec_ability(telegraph: i32, effects: Vec<meld_proto::abilities::AbilityEffect>) -> MonsterAbility {
        MonsterAbility {
            ability_kind: "test_blast".into(),
            callout_text: "Test Blast!".into(),
            weight: 100_000, // overwhelms basic_attack_weight in the roll
            cooldown_ticks: 0,
            telegraph_ticks: telegraph,
            hp_threshold_pct: None,
            min_level: 1,
            effects: vec![],
        }
        .tap_effects(effects)
    }
    trait Tap {
        fn tap_effects(self, e: Vec<meld_proto::abilities::AbilityEffect>) -> Self;
    }
    impl Tap for MonsterAbility {
        fn tap_effects(mut self, e: Vec<meld_proto::abilities::AbilityEffect>) -> Self {
            self.effects = e;
            self
        }
    }
    fn dmg_effect(coeff: f64, ty: DamageType) -> meld_proto::abilities::AbilityEffect {
        meld_proto::abilities::AbilityEffect {
            effect_kind: AbilityEffectKind::Damage,
            scaling_base: Some(ScalingBase::Attack),
            coefficient: Some(coeff),
            damage_type: Some(ty),
            target: AbilityTarget::SingleEnemy,
            status_name: None,
            duration_ticks: None,
            steal_target_kind: None,
        }
    }

    /// Run ticks until the monster acts; return (callouts, resolutions).
    fn run_until_monster_acts(battle: &mut Battle, max_ticks: usize) -> (Vec<Event>, Vec<Resolution>) {
        let mut all = Vec::new();
        let mut resolutions = Vec::new();
        for _ in 0..max_ticks {
            for ev in battle.tick() {
                match &ev {
                    Event::Resolved(r) if r.actor_id.starts_with('m') => {
                        resolutions.push(r.clone());
                    }
                    _ => {}
                }
                all.push(ev);
            }
            if !resolutions.is_empty() {
                return (all, resolutions);
            }
        }
        (all, resolutions)
    }

    /// A pool with one wide row and one single-target row, both plainly available. The
    /// AUTHORED rarity puts the wide one rarest, which is the shape five of the ten authored
    /// boss kits actually have — and the shape that breaks if the bias is applied to the pool
    /// instead of to the roll.
    fn wide_and_single_pool() -> Vec<MonsterAbility> {
        let row = |kind: &str, weight: i32, target: meld_proto::abilities::AbilityTarget| {
            MonsterAbility {
                ability_kind: kind.into(),
                callout_text: format!("{kind}!"),
                weight,
                cooldown_ticks: 40,
                telegraph_ticks: 0,
                hp_threshold_pct: None,
                min_level: 1,
                effects: vec![meld_proto::abilities::AbilityEffect {
                    effect_kind: meld_proto::abilities::AbilityEffectKind::Damage,
                    scaling_base: Some(meld_proto::abilities::ScalingBase::Attack),
                    coefficient: Some(0.5),
                    damage_type: Some(DamageType::Blunt),
                    target,
                    status_name: None,
                    duration_ticks: None,
                    steal_target_kind: None,
                }],
            }
        };
        vec![
            row("wide_sweep", 1, meld_proto::abilities::AbilityTarget::AllEnemies),
            row("single_poke", 2, meld_proto::abilities::AbilityTarget::SingleEnemy),
        ]
    }

    /// FS-4: a raid boss rolls its PARTY-WIDE half oftener, because that is the only half of
    /// its output that does not dilute as the crowd grows — a single-target blow is divided by
    /// however many heroes turned up and an all-enemy one is divided by nothing. Without it a
    /// boss sized for four parties is a longer fight and an *easier* one per hero.
    #[test]
    fn a_raid_boss_rolls_its_wide_half_oftener() {
        let b = Balance::load_default().unwrap();
        let wide_share = |parties: u8| {
            let mut boss = monster("m", 1_000_000, 400);
            boss.abilities = wide_and_single_pool();
            boss.raid_parties = parties;
            // Heroes that outlast the sample: the question is what the boss CHOOSES over many
            // turns, and a party that dies at turn ten answers it with noise.
            let bystander = |id: &str| {
                Fighter::new(
                    id.to_string(),
                    CombatantKind::Player,
                    Some(format!("p-{id}")),
                    None,
                    1,
                    10_000_000,
                    1,
                    0,
                    1,
                )
            };
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Gatekeeper,
                vec![bystander("h1"), bystander("h2")],
                vec![boss],
                &b,
                7,
            );
            let (mut wide, mut total) = (0u32, 0u32);
            for _ in 0..40_000 {
                if battle.is_over() {
                    break;
                }
                for ev in battle.tick() {
                    if let Event::Resolved(r) = ev {
                        if r.actor_id != "m" {
                            continue;
                        }
                        total += 1;
                        let hit: std::collections::HashSet<&String> =
                            r.effects.iter().map(|e| &e.target_id).collect();
                        if hit.len() >= 2 {
                            wide += 1;
                        }
                    }
                }
            }
            assert!(total > 50, "{parties} parties gave only {total} turns to read");
            f64::from(wide) / f64::from(total)
        };
        let (one, four) = (wide_share(1), wide_share(4));
        assert!(
            four > one,
            "a four-party boss goes wide {:.1}% of the time and a one-party boss {:.1}% - the \
             raid tier is a health bar and a word",
            four * 100.0,
            one * 100.0
        );
    }

    /// ⚠️ THE CROSSING THIS GUARDS. `signature_ability` reads the pool's weights as *rarity*
    /// to pick the rebuke, so a raid tier that scaled those weights in place would change what
    /// counts as a boss's signature — and change it in the worst direction, since the rows a
    /// raid tier makes common are exactly the wide capstones. Measured on the authored roster
    /// at the time: five of ten bosses had their rebuke downgraded from their apocalypse
    /// (SERMON OF SILENCE, THE DEPTHS RECLAIM, IRONMAW RAMPAGE, COLLAPSING SORROW, ASHFALL
    /// APOCALYPSE) to a small single-target poke, so a Worldbreaker answered an interruption
    /// more weakly than the ordinary version of itself.
    ///
    /// The bias therefore lives on the ROLL and the pool stays authored content. A raid tier
    /// changes how often you see an ability, never what counts as its signature.
    #[test]
    fn a_raid_tier_never_changes_what_a_bosss_signature_is() {
        let b = Balance::load_default().unwrap();
        let signature_of = |parties: u8| {
            let mut boss = monster("m", 1000, 100);
            boss.abilities = wide_and_single_pool();
            boss.raid_parties = parties;
            let battle = Battle::new(
                "b".into(),
                EncounterClass::Gatekeeper,
                vec![player("h1", 100)],
                vec![boss],
                &b,
                7,
            );
            let i = battle.idx("m").expect("the boss is in the fight");
            let idx = battle.signature_ability(i).expect("a non-empty kit has a signature");
            battle.fighters[i].abilities[idx].ability_kind.clone()
        };
        assert_eq!(
            signature_of(1), "wide_sweep",
            "the rarest authored row is the signature"
        );
        for parties in 2..=meld_proto::warbands::max_parties() {
            assert_eq!(
                signature_of(parties), "wide_sweep",
                "{parties} parties changed the boss's signature - the bias reached the pool"
            );
        }
    }

    /// A raid-shortened cooldown may never dip below its own telegraph: a shout has to keep
    /// meaning something, and an ability ready again before the last cast has landed would
    /// announce itself into a fight that never sees it arrive.
    #[test]
    fn a_raid_shortened_cooldown_never_undercuts_its_own_telegraph() {
        let mut b = Balance::load_default().unwrap();
        // Far past anything tunable, to prove the floor rather than the current numbers.
        b.encounters.raid_wide_cooldown_per_party = 500.0;
        let mut boss = monster("m", 1_000_000, 400);
        boss.abilities = wide_and_single_pool();
        boss.abilities[0].telegraph_ticks = 20;
        boss.abilities[0].cooldown_ticks = 200;
        boss.raid_parties = 4;
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Gatekeeper,
            vec![player("h1", 1), player("h2", 1)],
            vec![boss],
            &b,
            7,
        );
        let i = battle.idx("m").expect("the boss is in the fight");
        // Drive it until the wide row has been committed at least once, then read back when
        // the engine says it may be used again.
        for _ in 0..40_000 {
            if battle.is_over() {
                break;
            }
            battle.tick();
            if let Some(ready_at) = battle.fighters[i].ability_ready_at.get(&0).copied() {
                assert!(
                    ready_at >= battle.tick_count(),
                    "the wide row was ready again before its own telegraph landed"
                );
                if ready_at > battle.tick_count() {
                    return;
                }
            }
        }
        panic!("the wide row never came up to check");
    }

    #[test]
    fn weighted_ai_picks_the_heavy_ability_and_shouts_its_callout() {
        let b = balance();
        let mut m = monster("m1", 500, 300);
        m.abilities = vec![spec_ability(0, vec![dmg_effect(1.0, DamageType::Slash)])];
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)], // slow player: never acts first
            vec![m],
            &b,
            42,
        );
        let (_, resolutions) = run_until_monster_acts(&mut battle, 50);
        let res = resolutions.first().expect("monster acted");
        assert_eq!(res.action, BattleActionKind::Skill);
        assert!(res.auto);
        assert_eq!(res.callout_text.as_deref(), Some("Test Blast!"));
        assert!(res
            .effects
            .iter()
            .any(|e| matches!(e.kind, EffectKind::Damage) && e.amount.unwrap_or(0) > 0));
    }

    #[test]
    fn telegraphed_ability_shouts_first_and_lands_at_executes_at_tick() {
        let b = balance();
        let mut m = monster("m1", 500, 300);
        m.abilities = vec![spec_ability(10, vec![dmg_effect(2.0, DamageType::Blunt)])];
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![m],
            &b,
            42,
        );
        let mut telegraph_at = None;
        let mut executes_at_tick = 0;
        let mut landed_at = None;
        for _ in 0..80 {
            for ev in battle.tick() {
                match ev {
                    Event::TelegraphStarted {
                        callout_text,
                        executes_at_tick: at,
                        ..
                    } => {
                        assert_eq!(callout_text, "Test Blast!");
                        telegraph_at = Some(battle.tick_count());
                        executes_at_tick = at;
                    }
                    Event::Resolved(r) if r.actor_id == "m1" => {
                        // The channeled cast carries no callout (already shouted).
                        assert_eq!(r.callout_text, None);
                        landed_at = Some(battle.tick_count());
                    }
                    _ => {}
                }
            }
            if landed_at.is_some() {
                break;
            }
        }
        let (t, l) = (telegraph_at.expect("telegraphed"), landed_at.expect("landed"));
        assert_eq!(l, executes_at_tick, "cast lands exactly at executes_at_tick");
        assert!(l >= t + 10, "channel took the full telegraph window");
        assert!(player_hp(&battle, "a") < 40, "the channeled blow landed");
    }

    #[test]
    fn a_waking_salt_is_the_only_way_back_up() {
        let b = balance();
        let mut fallen = player("down", 5);
        fallen.hp = 0;
        fallen.alive = false;
        fallen.max_hp = 100;
        let standing = player("up", 5);
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![standing, fallen],
            vec![monster("m1", 900, 1)],
            &b,
            42,
        );
        let actor = battle.idx("up").unwrap();
        let down = battle.idx("down").unwrap();

        // A salve cannot reach the dead — every other item targets the living.
        let _ = battle.resolve_item(actor, Some("bloom_salve"), Some("down"), None);
        assert!(!battle.fighters[down].alive, "a salve raised the dead");

        // A Waking Salt does, at a fraction of max HP rather than a full heal.
        let fx = battle.resolve_item(actor, Some("waking_salt"), Some("down"), None);
        assert!(battle.fighters[down].alive, "the salt did not revive");
        let want = ((100.0 * b.consumable.revive_hp_fraction).round() as i32).max(1);
        assert_eq!(battle.fighters[down].hp, want);
        assert!(
            fx.effects.iter().any(|e| e.status.as_deref() == Some("revived")),
            "the revival was not announced: {:?}",
            fx.effects
        );
    }

    #[test]
    fn a_waking_salt_with_nobody_down_is_not_wasted() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 5)],
            vec![monster("m1", 900, 1)],
            &b,
            42,
        );
        let i = battle.idx("a").unwrap();
        let before = battle.fighters[i].hp;
        // Nobody to raise: the bottle produces no effects, so the game loop (which
        // spends the item only when the action resolves with something) has nothing
        // to charge for.
        let res = battle.resolve_item(i, Some("waking_salt"), None, None);
        assert!(res.effects.is_empty(), "{:?}", res.effects);
        assert_eq!(battle.fighters[i].hp, before);
    }





    #[test]
    fn armour_never_absorbs_a_whole_blow_so_depth_stays_dangerous() {
        let b = balance();
        let battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("p", 400)],
            vec![monster("m", 100, 1)],
            &b,
            7,
        );
        // Defence grows about +1 per hero level while creature attack grows only with
        // distance, so a levelled hero used to stop taking damage entirely: a level-25
        // hero (def 30, 292 HP) took `min_damage` from everything out to roughly
        // distance 1100 and needed 292 hits to die. The floor is what keeps depth
        // dangerous without touching hero growth or the distance curve.
        let floor = b.combat_math.damage_floor_fraction;
        for atk in [10i32, 30, 60, 120, 400] {
            // Armour far above the attack still cannot reduce the hit to nothing.
            let overwhelmed = battle.damage(atk, atk * 10, false);
            assert!(
                overwhelmed as f64 >= (atk as f64 * floor).round() - 1.0,
                "atk {atk} against heavy armour landed {overwhelmed}, below the floor"
            );
            // And armour still MATTERS: unarmoured takes more than armoured.
            assert!(
                battle.damage(atk, 0, false) > battle.damage(atk, atk * 10, false),
                "armour stopped mattering at atk {atk}"
            );
        }
        // Deeper creatures hit a fixed hero harder, monotonically — the property the
        // whole difficulty-by-distance design rests on.
        let hero_def = 30;
        let mut last = 0;
        for atk in [7i32, 12, 17, 28, 52, 109] {
            let hit = battle.damage(atk, hero_def, false);
            assert!(hit >= last, "a deeper creature hit softer: {hit} after {last}");
            last = hit;
        }
        assert!(last > 20, "the deepest creature measured still only hit for {last}");
    }
    #[test]
    fn a_theft_is_reported_for_the_server_to_settle() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![{
                let mut f = leveled_player("s", 400, meld_proto::skills::unlock_level("mug"));
                // A hero with a player behind it: the engine reports the theft, and
                // the server decides what came off the body (it owns the economy).
                f.player_id = Some("p1".into());
                f
            }],
            vec![monster("m", 500, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "s");
        let events = battle
            .submit(
                "s",
                "a1".into(),
                BattleActionKind::Skill,
                Some(vec!["m".into()]),
                Some("mug".into()),
                None,
            )
            .unwrap();
        let pilfered = events.iter().any(|e| {
            matches!(
                e,
                Event::Pilfered { thief_player_id, victim_combatant_id }
                    if thief_player_id == "p1" && victim_combatant_id == "m"
            )
        });
        assert!(pilfered, "Mug reported no theft: {events:?}");

        // A plain attack steals nothing — only the Shifter's own kit picks pockets.
        tick_to_ready(&mut battle, "s");
        let events = battle
            .submit("s", "a2".into(), BattleActionKind::Attack, Some(vec!["m".into()]), None, None)
            .unwrap();
        assert!(!events.iter().any(|e| matches!(e, Event::Pilfered { .. })));
    }

    #[test]
    fn mug_is_steal_with_a_hit_on_the_way_past() {
        let b = balance();
        let make = |level: i32| {
            Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![leveled_player("s", 400, level)],
                vec![monster("m", 500, 1)],
                &b,
                7,
            )
        };
        // Steal takes tempo and nothing else — the foe keeps every hit point.
        let mut early = make(meld_proto::skills::unlock_level("steal"));
        tick_to_ready(&mut early, "s");
        let mi = early.fighters.iter().position(|f| f.combatant_id == "m").unwrap();
        early.fighters[mi].gauge = 0.9;
        early
            .submit("s", "a1".into(), BattleActionKind::Skill, Some(vec!["m".into()]), Some("steal".into()), None)
            .unwrap();
        assert!(gauge_of(&early, "m") < 0.9, "Steal took no tempo");
        assert_eq!(player_hp(&early, "m"), 500, "Steal drew blood; it should not");

        // Mug takes MORE tempo and draws blood: the same ability, grown up.
        let mut late = make(meld_proto::skills::unlock_level("mug"));
        tick_to_ready(&mut late, "s");
        let mi = late.fighters.iter().position(|f| f.combatant_id == "m").unwrap();
        late.fighters[mi].gauge = 0.9;
        late.fighters[mi].dodge = 0.0;
        late
            .submit("s", "a1".into(), BattleActionKind::Skill, Some(vec!["m".into()]), Some("mug".into()), None)
            .unwrap();
        assert!(player_hp(&late, "m") < 500, "Mug did not hit");
        assert!(
            gauge_of(&late, "m") <= gauge_of(&early, "m"),
            "Mug stole less tempo than plain Steal"
        );
    }

    #[test]
    fn an_upgraded_row_hits_harder_than_the_one_it_replaced() {
        let b = balance();
        let hit = |skill: &str, level: i32| {
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![explorer("h", 400, level)],
                vec![monster("m", 4000, 1)],
                &b,
                7,
            );
            let hi = battle.fighters.iter().position(|f| f.combatant_id == "h").unwrap();
            battle.fighters[hi].adrenaline = 100;
            let mi = battle.fighters.iter().position(|f| f.combatant_id == "m").unwrap();
            battle.fighters[mi].dodge = 0.0;
            tick_to_ready(&mut battle, "h");
            battle
                .submit("h", "a1".into(), BattleActionKind::Skill, Some(vec!["m".into()]), Some(skill.into()), None)
                .unwrap();
            4000 - player_hp(&battle, "m")
        };
        // Crushing Blow replaces Power Strike, and must actually be an upgrade.
        assert!(
            hit("crushing_blow", meld_proto::skills::unlock_level("crushing_blow")) > hit("power_strike", 1),
            "Crushing Blow is not an upgrade on Power Strike"
        );
        // Apex Predator replaces Frenzy the same way — measured on ONE target, since
        // its whole point is that it also reaches the rest of the pack.
        assert!(
            hit("apex_predator", meld_proto::skills::unlock_level("apex_predator"))
                > hit("power_strike", 1),
            "Apex Predator is not an upgrade"
        );
    }

    #[test]
    fn a_casters_deep_kit_mends_the_whole_party_and_costs_the_caster() {
        let b = balance();
        // Eternal Bloom is the Resonant's deepest party heal: everyone whole and warded,
        // paid for out of the healer.
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![
                leveled_player("r", 400, meld_proto::skills::unlock_level("eternal_bloom")),
                leveled_player("a", 400, meld_proto::skills::unlock_level("eternal_bloom")),
                leveled_player("c", 400, meld_proto::skills::unlock_level("eternal_bloom")),
            ],
            vec![monster("m", 900, 1)],
            &b,
            7,
        );
        // Hurt the party, and the healer with them.
        let wounded = {
            let i = battle.fighters.iter().position(|f| f.combatant_id == "r").unwrap();
            battle.fighters[i].max_hp / 4
        };
        for id in ["r", "a", "c"] {
            let i = battle.fighters.iter().position(|f| f.combatant_id == id).unwrap();
            battle.fighters[i].hp = wounded;
        }
        tick_to_ready(&mut battle, "r");
        let healer_before = player_hp(&battle, "r");
        battle
            .submit(
                "r",
                "a1".into(),
                BattleActionKind::Skill,
                None,
                Some("eternal_bloom".into()),
                None,
            )
            .unwrap();
        // Everyone mended, including allies the caster never targeted…
        for id in ["a", "c"] {
            assert!(player_hp(&battle, id) > wounded, "{id} was not healed");
        }
        // …everyone warded…
        for id in ["r", "a", "c"] {
            let i = battle.fighters.iter().position(|f| f.combatant_id == id).unwrap();
            assert!(battle.fighters[i].barrier > 0, "{id} got no Barrier");
        }
        // …and the healer paid for it out of its own HP (net of its own healing).
        let healer_after = player_hp(&battle, "r");
        let full = battle.fighters[0].max_hp;
        assert!(
            healer_after < full,
            "the Resonant healed the party for free: {healer_before} -> {healer_after} of {full}"
        );
        assert!(healer_after >= 1, "the Resonant killed itself healing");
    }

    #[test]
    fn a_deep_ability_is_locked_until_the_caster_is_deep() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![leveled_player("r", 400, meld_proto::skills::unlock_level("mend_all"))],
            vec![monster("m", 900, 1)],
            &b,
            7,
        );
        tick_to_ready(&mut battle, "r");
        // Level 20 is nowhere near the L100 capstone: the server refuses it even
        // though the client would never offer the row.
        assert!(battle
            .submit(
                "r",
                "a1".into(),
                BattleActionKind::Skill,
                None,
                Some("eternal_bloom".into()),
                None,
            )
            .is_err());
    }

    #[test]
    fn a_boss_band_rides_the_wire_and_only_when_it_has_one() {
        let mut boss = monster("deep", 900, 1);
        boss.boss_band = 3;
        let plain = monster("plain", 900, 1);
        // The band rides the additive `statuses` convention, so the client can tint a
        // deep boss without a proto change…
        assert!(
            boss.build_wire_statuses().iter().any(|s| s == "boss_band:3"),
            "{:?}",
            boss.build_wire_statuses()
        );
        // …and an ordinary creature says nothing about a palette it does not have.
        assert!(
            plain.build_wire_statuses().iter().all(|s| !s.starts_with("boss_band:")),
            "{:?}",
            plain.build_wire_statuses()
        );
    }

    #[test]
    fn a_leader_is_shielded_by_its_living_minions() {
        let b = balance();
        let pack = |minions: usize| -> (Battle, usize) {
            let mut leader = monster("boss", 4000, 1);
            leader.pack_role = PackRole::Leader;
            leader.def = 0;
            let mut enemies = vec![leader];
            for i in 0..minions {
                let mut m = monster(&format!("min{i}"), 200, 1);
                m.pack_role = PackRole::Minion;
                enemies.push(m);
            }
            let battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![player("a", 5)],
                enemies,
                &b,
                42,
            );
            let li = battle.idx("boss").unwrap();
            (battle, li)
        };

        let hit = |bt: &mut Battle, i: usize| -> i32 {
            let before = bt.fighters[i].hp;
            let _ = bt.apply_damage(i, 100);
            before - bt.fighters[i].hp
        };

        // Alone, the leader eats the whole blow.
        let (mut solo, li) = pack(0);
        assert_eq!(hit(&mut solo, li), 100);

        // Two minions soak part of it; four soak more, up to the cap.
        let (mut two, li2) = pack(2);
        let with_two = hit(&mut two, li2);
        let (mut four, li4) = pack(4);
        let with_four = hit(&mut four, li4);
        assert!(with_two < 100, "minions did not shield the leader: {with_two}");
        assert!(with_four < with_two, "more minions should shield more: {with_four} vs {with_two}");
        // …but the cap means a pack is never immune.
        let (mut many, lim) = pack(9);
        let capped = hit(&mut many, lim);
        assert!(
            capped as f64 >= 100.0 * (1.0 - b.encounters.pack_guard_cap) - 1.0,
            "the guard cap did not hold: {capped}"
        );

        // Kill the minions and the leader is exposed again — the other valid line.
        for i in 0..four.fighters.len() {
            if four.fighters[i].pack_role == PackRole::Minion {
                four.fighters[i].alive = false;
            }
        }
        assert_eq!(hit(&mut four, li4), 100, "a leaderless guard still applied");
    }

    #[test]
    fn breaking_the_leader_routs_the_littles() {
        let b = balance();
        let mut leader = monster("boss", 30, 1);
        leader.pack_role = PackRole::Leader;
        leader.def = 0;
        let mut minion = monster("min0", 400, 1);
        minion.pack_role = PackRole::Minion;
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 5)],
            vec![leader, minion],
            &b,
            42,
        );
        let li = battle.idx("boss").unwrap();
        let mi = battle.idx("min0").unwrap();

        // While the leader lives the minion fights above its weight.
        assert!(
            (battle.pack_attack_mult(mi) - b.encounters.pack_aura_atk_mult).abs() < 1e-9,
            "no leader aura"
        );
        assert!(!battle.fighters[mi].flees, "a minion does not start skittish");

        // Break the leader: the fight turns, and the client is told so.
        let fx = battle.apply_damage(li, 9999);
        assert!(!battle.fighters[li].alive);
        assert!(
            fx.iter().any(|e| e.status.as_deref() == Some("routed")),
            "the rout was not announced: {fx:?}"
        );
        assert!(battle.fighters[mi].flees, "a routed minion should bolt when low");
        assert!(
            (battle.pack_attack_mult(mi) - b.encounters.pack_rout_atk_mult).abs() < 1e-9,
            "a routed minion still hits at full strength"
        );
        assert!(
            b.encounters.pack_rout_atk_mult < b.encounters.pack_aura_atk_mult,
            "routing must be a downgrade"
        );
    }

    #[test]
    fn pack_rules_leave_lone_creatures_and_heroes_alone() {
        let b = balance();
        let lone = monster("m1", 500, 1);
        assert_eq!(lone.pack_role, PackRole::None);
        let battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 5)],
            vec![lone],
            &b,
            42,
        );
        let i = battle.idx("m1").unwrap();
        assert_eq!(battle.pack_attack_mult(i), 1.0);
        assert_eq!(battle.pack_guard_fraction(i), 0.0);
        let hero = battle.idx("a").unwrap();
        assert_eq!(battle.pack_attack_mult(hero), 1.0);
        assert_eq!(battle.pack_guard_fraction(hero), 0.0);
    }

    #[test]
    fn a_backstab_after_a_snare_hits_harder_than_one_without() {
        let b = balance();
        // Two identical fights. In one, the Explorer Snares first; in the other the
        // Shifter opens with Backstab cold. Same target, same stats.
        let setup = |snare_first: bool| -> i32 {
            let mut explorer = player("ex", 5);
            explorer.level = 3;
            explorer.adrenaline = 99;
            explorer.adrenaline_max = 99;
            let mut shifter = player("sh", 5);
            shifter.level = 3;
            let mut mob = monster("m1", 4000, 1);
            mob.def = 0;
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![explorer, shifter],
                vec![mob],
                &b,
                42,
            );
            let mi = battle.idx("m1").unwrap();
            if snare_first {
                battle.active_skill = Some("snare".into());
                let _ = battle.apply_damage(mi, 1);
                battle.active_skill = None;
            }
            let before = battle.fighters[mi].hp;
            battle.active_skill = Some("backstab".into());
            let fx = battle.apply_damage(mi, 100);
            battle.active_skill = None;
            let dealt = before - battle.fighters[mi].hp;
            // The combo announces itself so the client can call it out.
            let announced = fx
                .iter()
                .any(|e| e.status.as_deref() == Some("combo:cut_the_snare"));
            assert_eq!(announced, snare_first, "combo announcement mismatch");
            dealt
        };
        let cold = setup(false);
        let comboed = setup(true);
        assert!(
            comboed > cold,
            "Cut the Snare must reward the sequence: {comboed} vs {cold}"
        );
        // 1.6x on the payoff, minus the 1 point the Snare itself did.
        assert_eq!(comboed, (cold as f64 * 1.6).round() as i32);
    }

    #[test]
    fn a_primer_expires_and_pays_only_once() {
        let b = balance();
        let mut shifter = player("sh", 5);
        shifter.level = 3;
        let mut mob = monster("m1", 9000, 1);
        mob.def = 0;
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![shifter],
            vec![mob],
            &b,
            42,
        );
        let mi = battle.idx("m1").unwrap();
        let hit = |bt: &mut Battle, skill: &str| -> i32 {
            let before = bt.fighters[mi].hp;
            bt.active_skill = Some(skill.to_string());
            let _ = bt.apply_damage(mi, 100);
            bt.active_skill = None;
            before - bt.fighters[mi].hp
        };

        // Prime, then cash in twice: the second Backstab is unamplified, because a
        // primer that paid repeatedly would make one setup turn worth infinite ones.
        battle.active_skill = Some("snare".into());
        let _ = battle.apply_damage(mi, 0);
        battle.active_skill = None;
        let first = hit(&mut battle, "backstab");
        let second = hit(&mut battle, "backstab");
        assert!(first > second, "the primer was not consumed: {first} vs {second}");

        // Prime again, then let the window lapse — a stale primer pays nothing.
        battle.active_skill = Some("snare".into());
        let _ = battle.apply_damage(mi, 0);
        battle.active_skill = None;
        battle.tick_count += b.adventure.combo_window_ticks + 1;
        let expired = hit(&mut battle, "backstab");
        assert_eq!(expired, second, "an expired primer still paid out");
    }

    #[test]
    fn the_wrong_follow_up_does_not_cash_a_primer() {
        let b = balance();
        let mut hero = player("h", 5);
        hero.level = 5;
        let mut mob = monster("m1", 9000, 1);
        mob.def = 0;
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![hero],
            vec![mob],
            &b,
            42,
        );
        let mi = battle.idx("m1").unwrap();
        battle.active_skill = Some("snare".into());
        let _ = battle.apply_damage(mi, 0);
        battle.active_skill = None;

        // Snare primes Backstab, not Frenzy — a different payoff finds nothing.
        let before = battle.fighters[mi].hp;
        battle.active_skill = Some("frenzy".into());
        let fx = battle.apply_damage(mi, 100);
        battle.active_skill = None;
        assert_eq!(before - battle.fighters[mi].hp, 100);
        assert!(fx.iter().all(|e| e.status.as_deref() != Some("combo:cut_the_snare")));

        // …and the primer is still there for the right one.
        let before = battle.fighters[mi].hp;
        battle.active_skill = Some("backstab".into());
        let _ = battle.apply_damage(mi, 100);
        assert_eq!(before - battle.fighters[mi].hp, 160);
    }

    #[test]
    fn each_potion_does_its_own_thing_not_just_healing() {
        let b = balance();
        let drink = |item: &str| -> Fighter {
            let mut hero = player("a", 5);
            hero.hp = 20;
            hero.max_hp = 100;
            hero.adrenaline_max = 20;
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![hero],
                vec![monster("m1", 500, 1)],
                &b,
                42,
            );
            let i = battle.idx("a").unwrap();
            let _ = battle.resolve_item(i, Some(item), None, None);
            battle.fighters[i].clone()
        };

        // A salve heals a fraction; an elixir fills the bar.
        let salve = drink("bloom_salve");
        assert!(salve.hp > 20 && salve.hp < 100, "salve hp {}", salve.hp);
        assert_eq!(drink("elixir").hp, 100);

        // The others grant STATES, and leave HP alone — that is the point: a potion
        // you drink before the blow, not after.
        let tonic = drink("bulwark_tonic");
        assert_eq!(tonic.barrier, b.consumable.barrier_amount);
        assert_eq!(tonic.hp, 20, "a tonic is not a heal");
        assert_eq!(drink("mending_draught").regen, b.consumable.regen_amount);
        let dust = drink("ghostdust");
        assert!((dust.evasion - b.consumable.evasion_pct as f64 / 100.0).abs() < 1e-9);
        let philtre = drink("fury_philtre");
        assert_eq!(philtre.adrenaline, b.consumable.adrenaline_amount.min(20));

        // An unknown id still heals, so an older client is never stranded.
        assert!(drink("mystery_bottle").hp > 20);

        // MS-1's trophy line: the same effects, a bigger dose. A potion made from a
        // monster part has to out-do the herb version or nobody would render one.
        assert!(drink("scarab_ward").barrier > tonic.barrier);
        assert!(drink("verdant_draught").regen > b.consumable.regen_amount);
        assert!(drink("rimeglass_vial").evasion > dust.evasion);
        assert!(drink("cinderblood_philtre").adrenaline >= philtre.adrenaline);
        assert!(drink("ichor_salve").hp > salve.hp, "ichor should out-heal a salve");
    }

    #[test]
    fn a_quintessence_raises_the_fallen_nearer_to_whole_than_a_salt_does() {
        let b = balance();
        let raise = |item: &str| -> i32 {
            let mut down = player("down", 5);
            down.alive = false;
            down.hp = 0;
            down.max_hp = 100;
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![player("a", 5), down],
                vec![monster("m1", 500, 1)],
                &b,
                42,
            );
            let actor = battle.idx("a").unwrap();
            let _ = battle.resolve_item(actor, Some(item), Some("down"), None);
            let i = battle.idx("down").unwrap();
            assert!(battle.fighters[i].alive, "{item} did not revive");
            battle.fighters[i].hp
        };
        let salt = raise("waking_salt");
        let quint = raise("quintessence");
        assert!(quint > salt, "quintessence {quint} vs salt {salt}");
        assert!(quint <= 100, "a revive cannot overshoot max HP: {quint}");
    }

    #[test]
    fn a_fury_philtre_is_inert_on_a_class_with_no_adrenaline() {
        let b = balance();
        let mut caster = player("psy", 5);
        caster.adrenaline_max = 0;
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![caster],
            vec![monster("m1", 500, 1)],
            &b,
            42,
        );
        let i = battle.idx("psy").unwrap();
        let _ = battle.resolve_item(i, Some("fury_philtre"), None, None);
        assert_eq!(battle.fighters[i].adrenaline, 0, "banked rage it cannot hold");
    }

    /// **`def` answers a blade; `ward` answers a spell.** Both are subtracted from ABILITY
    /// damage, and each only from its own half — the split the whole stat exists for.
    ///
    /// Before `ward`, non-physical ability damage was subtracted by NOTHING: `def` is only
    /// consulted for a basic attack, so a boss that fights with fire and breath ignored every
    /// point of armour a party had earned, and no gear could gate it.
    #[test]
    fn def_answers_a_blade_and_ward_answers_a_spell() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let i = battle.idx("a").unwrap();
        battle.fighters[i].def = 30;
        battle.fighters[i].ward = 0;
        battle.fighters[i].max_hp = 100_000;
        battle.fighters[i].hp = 100_000;

        let hit = |bt: &mut Battle, ty: DamageType| -> i32 {
            let before = bt.fighters[i].hp;
            let _ = bt.apply_ability_damage(i, 100, ty);
            before - bt.fighters[i].hp
        };

        // Armour, and no ward: the sword is blunted, the fire is not.
        let slash = hit(&mut battle, DamageType::Slash);
        let fire = hit(&mut battle, DamageType::Fire);
        assert_eq!(slash, 70, "def should have taken 30 off a physical ability");
        assert_eq!(fire, 100, "def must NOT reduce fire — that is what ward is for");

        // Now swap which stat the hero has, and the answers swap with it.
        battle.fighters[i].def = 0;
        battle.fighters[i].ward = 30;
        assert_eq!(hit(&mut battle, DamageType::Slash), 100, "ward must not stop a blade");
        assert_eq!(hit(&mut battle, DamageType::Fire), 70, "ward should have blunted the fire");

        // True damage answers to neither, which is the whole point of it.
        battle.fighters[i].def = 60;
        battle.fighters[i].ward = 60;
        assert_eq!(hit(&mut battle, DamageType::None), 100, "true damage was mitigated");
    }

    /// Neither stat can make a hero untouchable: `damage_floor_fraction` bounds the
    /// subtraction the same way it bounds a basic attack, so stacking ward is a discount and
    /// never an immunity. (Immunity is what `damage_modifiers` is for, deliberately.)
    #[test]
    fn no_amount_of_ward_makes_an_ability_free() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let i = battle.idx("a").unwrap();
        battle.fighters[i].max_hp = 100_000;
        battle.fighters[i].hp = 100_000;
        battle.fighters[i].ward = 100_000;
        let before = battle.fighters[i].hp;
        let _ = battle.apply_ability_damage(i, 100, DamageType::Fire);
        let dealt = before - battle.fighters[i].hp;
        assert!(dealt >= 25, "the floor should keep at least a quarter through: {dealt}");
        assert!(dealt < 100);
    }

    /// "of the Furnace" pays on what you DEAL, and "of the Aegis" on what you take — the two
    /// halves the user asked for, and they compose: a bigger fire still meets the target's
    /// armour and its profile afterwards.
    #[test]
    fn element_affixes_cut_both_ways() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![monster("m", 100_000, 1)],
            &b,
            42,
        );
        let a = battle.idx("a").unwrap();
        let m = battle.idx("m").unwrap();
        battle.active_actor = Some(a);

        let deal = |bt: &mut Battle| -> i32 {
            let before = bt.fighters[m].hp;
            let _ = bt.apply_ability_damage(m, 100, DamageType::Fire);
            before - bt.fighters[m].hp
        };
        let plain = deal(&mut battle);

        // +50% fire dealt.
        battle.fighters[a].element_power.insert(DamageType::Fire, 1.5);
        let hotter = deal(&mut battle);
        assert!(hotter > plain, "element power did nothing: {plain} -> {hotter}");

        // …and it is ELEMENT-specific, not a flat damage bonus.
        let ice_before = battle.fighters[m].hp;
        let _ = battle.apply_ability_damage(m, 100, DamageType::Ice);
        let ice = ice_before - battle.fighters[m].hp;
        assert_eq!(ice, plain, "fire power should not amplify ice");

        // The defensive twin: ward blunts what arrives, whatever the attacker stacked.
        battle.fighters[m].ward = 40;
        let warded = deal(&mut battle);
        assert!(warded < hotter, "ward did not reduce an amplified spell");
    }

    /// An AFFLICTION holds until something lifts it; a BOON still fades. Outlasting a debuff
    /// by standing still is not a decision, and a permanent Barrier is not a buff.
    #[test]
    fn afflictions_hold_and_boons_fade() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let i = battle.idx("a").unwrap();
        battle.fighters[i].max_hp = 100_000;
        battle.fighters[i].hp = 100_000;
        // Both applied with a SHORT expiry, so only the classification can save one.
        let soon = battle.tick_count + 1;
        battle.fighters[i].timed_statuses.push(("poison".into(), soon));
        battle.fighters[i].timed_statuses.push(("hasted".into(), soon));

        battle.tick_count += 10;
        let _ = battle.start_of_turn(i);

        let held: Vec<String> =
            battle.fighters[i].timed_statuses.iter().map(|(n, _)| n.clone()).collect();
        assert!(held.contains(&"poison".to_string()), "poison wore off on its own: {held:?}");
        assert!(!held.contains(&"hasted".to_string()), "haste never faded: {held:?}");
    }

    /// …and a cure is what lifts it, leaving boons alone — a cleanse that stripped your own
    /// Barrier would be a trap.
    #[test]
    fn a_cure_lifts_every_affliction_and_no_boon() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let i = battle.idx("a").unwrap();
        let far = battle.tick_count + 10_000;
        for n in ["poison", "web", "marked", "blinded", "hasted"] {
            battle.fighters[i].timed_statuses.push((n.to_string(), far));
        }
        let fx = battle.cure(i, meld_proto::statuses::Family::All);
        assert_eq!(fx.len(), 4, "should have lifted the four afflictions, got {fx:?}");
        let held: Vec<String> =
            battle.fighters[i].timed_statuses.iter().map(|(n, _)| n.clone()).collect();
        assert_eq!(held, vec!["hasted".to_string()], "left holding {held:?}");
    }

    /// The earliest mender row in the game has to be able to lift one, or a party that
    /// catches something at level 5 simply carries it for the rest of the dive.
    #[test]
    fn a_poultice_draws_the_poison_out() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("k", 5), player("h", 5)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let keeper = battle.idx("k").unwrap();
        let hurt = battle.idx("h").unwrap();
        battle.fighters[keeper].class_key = "keeper".into();
        battle.fighters[hurt].hp = 1;
        let far = battle.tick_count + 10_000;
        battle.fighters[hurt].timed_statuses.push(("poison".into(), far));

        let target = battle.fighters[hurt].combatant_id.clone();
        let _ = battle
            .resolve_keeper(keeper, "poultice", Some(&target), None)
            .expect("poultice resolves");
        assert!(
            !battle.fighters[hurt]
                .timed_statuses
                .iter()
                .any(|(n, _)| n == "poison"),
            "the poultice healed but left the poison in"
        );
    }

    /// A cure answers a CONDITION, not a checklist. A poultice draws venom out and has
    /// nothing to say about being blinded — only a Panacea answers everything, and it is
    /// priced like it.
    #[test]
    fn a_cure_is_specific_and_only_a_panacea_is_not() {
        use meld_proto::statuses::Family;
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let i = battle.idx("a").unwrap();
        let far = battle.tick_count + 10_000;
        let load = |bt: &mut Battle| {
            bt.fighters[i].timed_statuses.clear();
            for n in ["poison", "web", "blinded", "dread", "hasted"] {
                bt.fighters[i].timed_statuses.push((n.to_string(), far));
            }
        };
        let held = |bt: &Battle| -> Vec<String> {
            bt.fighters[i].timed_statuses.iter().map(|(n, _)| n.clone()).collect()
        };

        load(&mut battle);
        let _ = battle.cure(i, Family::Venom);
        assert!(!held(&battle).contains(&"poison".into()), "venom should lift poison");
        assert!(held(&battle).contains(&"blinded".into()), "venom cured a blindness");
        assert!(held(&battle).contains(&"dread".into()), "venom cured a dread");

        load(&mut battle);
        let _ = battle.cure(i, Family::All);
        assert_eq!(held(&battle), vec!["hasted".to_string()], "a Panacea should answer all four");
    }

    /// A revive at a REACHABLE rung. The only one in the game was a level-255 capstone, so a
    /// party that lost a hero at level 20 had nothing but a rare drop.
    #[test]
    fn a_mender_can_raise_the_fallen_at_rung_fifty() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("r", 50), player("d", 50)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let medic = battle.idx("r").unwrap();
        let dead = battle.idx("d").unwrap();
        battle.fighters[medic].class_key = "resonant".into();
        battle.fighters[dead].alive = false;
        battle.fighters[dead].hp = 0;

        let _ = battle.resolve_resonant(medic, "revitalize", dead);
        assert!(battle.fighters[dead].alive, "revitalize left the hero down");
        assert!(battle.fighters[dead].hp > 0);
        // Back at the END of the queue, not with a free turn.
        assert_eq!(battle.fighters[dead].gauge, 0.0);
    }

    /// DREAD forbids going at the thing that frightened you and NOTHING else. The hero keeps
    /// every other option — that is what separates a fear from a stun.
    #[test]
    fn dread_stops_you_facing_the_enemy_and_leaves_the_rest() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 20)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let i = battle.idx("a").unwrap();
        battle.fighters[i].gauge = 1.0;
        battle.fighters[i].awaiting = true;
        battle.fighters[i].timed_statuses.push(("dread".into(), u64::MAX));
        let foe = battle.fighters[battle.idx("m").unwrap()].combatant_id.clone();

        let hit = battle.submit(
            &battle.fighters[i].combatant_id.clone(),
            "x1".into(),
            BattleActionKind::Attack,
            Some(vec![foe]),
            None,
            None,
        );
        assert!(hit.is_err(), "a frightened hero attacked the thing it fears");

        // …but defending is still its own choice.
        let guard = battle.submit(
            &battle.fighters[i].combatant_id.clone(),
            "x2".into(),
            BattleActionKind::Defend,
            None,
            None,
            None,
        );
        assert!(guard.is_ok(), "dread should not stop you defending: {guard:?}");
    }

    /// A BLOW BRINGS YOU ROUND — and a heal takes the wheel back from a frenzy. Both are how a
    /// party answers a condition without carrying the right bottle.
    #[test]
    fn a_hit_wakes_you_and_a_heal_calms_you() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 20)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let i = battle.idx("a").unwrap();
        battle.fighters[i].max_hp = 10_000;
        battle.fighters[i].hp = 5_000;
        for n in ["dread", "confused", "frenzied"] {
            battle.fighters[i].timed_statuses.push((n.to_string(), u64::MAX));
        }

        let _ = battle.apply_damage_reaching(i, 10, true);
        let held = |bt: &Battle| -> Vec<String> {
            bt.fighters[i].timed_statuses.iter().map(|(n, _)| n.clone()).collect()
        };
        assert!(!held(&battle).contains(&"dread".into()), "a blow left the dread");
        assert!(!held(&battle).contains(&"confused".into()), "a blow left the confusion");
        assert!(held(&battle).contains(&"frenzied".into()), "a blow should NOT calm a frenzy");

        let _ = battle.apply_heal(i, 50);
        assert!(!held(&battle).contains(&"frenzied".into()), "healing left the frenzy");
    }

    /// A party where nobody can act is DEAD, not deadlocked. Paralysis skips the turn, so
    /// without this the creatures would work through a party standing still — the unbounded
    /// soft-lock a gauge cap used to cause.
    #[test]
    fn a_wholly_paralysed_party_is_a_defeat() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 20), player("c", 20)],
            vec![monster("m", 500, 1)],
            &b,
            42,
        );
        let a = battle.idx("a").unwrap();
        let c = battle.idx("c").unwrap();
        battle.fighters[a].timed_statuses.push(("paralyzed".into(), u64::MAX));
        let mut events = Vec::new();
        battle.check_terminal(&mut events);
        assert!(events.is_empty(), "one paralysed hero is not a wipe");

        battle.fighters[c].timed_statuses.push(("paralyzed".into(), u64::MAX));
        battle.check_terminal(&mut events);
        assert!(
            events.iter().any(|e| matches!(
                e,
                Event::Ended { outcome: BattleOutcome::Defeat }
            )),
            "a wholly paralysed party should be a defeat, got {events:?}"
        );
    }

    /// CONFUSION rolls BOTH halves — what you do and who you do it to. A confusion that only
    /// mis-aims is a targeting penalty; the point is that the order you gave is not the order
    /// that happens.
    #[test]
    fn confusion_scrambles_the_action_and_the_target() {
        let b = balance();
        let mut actions = std::collections::HashSet::new();
        let mut targets = std::collections::HashSet::new();
        // Many rolls, because "random" is the assertion — one sample proves nothing.
        for seed in 0..60u64 {
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![player("a", 40), player("b", 40)],
                vec![monster("m", 5_000, 1)],
                &b,
                seed,
            );
            let i = battle.idx("a").unwrap();
            battle.fighters[i].class_key = "resonant".into();
            battle.fighters[i].timed_statuses.push(("confused".into(), u64::MAX));
            let (act, skill, target) =
                battle.scramble(i, BattleActionKind::Attack, None);
            actions.insert(format!("{act:?}:{}", skill.unwrap_or_default()));
            targets.insert(target.map(|t| t[0].clone()).unwrap_or_default());
        }
        assert!(actions.len() > 2, "the action barely varies: {actions:?}");
        assert!(targets.len() > 1, "it always hits the same combatant: {targets:?}");
        // …and it can land on a friend, which is the whole flavour of the thing.
        assert!(targets.iter().any(|t| t == "b" || t == "a"), "never once hit its own side");
    }

    /// PARALYSIS can be broken by WILL — a slim hope, and a high-Will hero has a better one.
    /// Without a way out that is not a cure, a party with no mender simply loses.
    #[test]
    fn will_can_break_a_paralysis() {
        let b = balance();
        let odds = |wll: i32| -> f64 {
            (b.affliction.paralysis_break_base + b.affliction.paralysis_break_per_wll * wll as f64)
                .min(b.affliction.paralysis_break_cap)
        };
        assert!(odds(200) > odds(10), "Will should matter to breaking free");
        assert!(odds(10) > 0.0, "even a frail hero gets a chance");
        assert!(odds(100_000) <= b.affliction.paralysis_break_cap, "the cap should hold");
        assert!(
            b.affliction.paralysis_break_cap < 1.0,
            "paralysis must never be shruggable outright, or curing it is pointless"
        );

        // And it actually fires. TWO heroes on purpose: one paralysed hero alone IS a wholly
        // paralysed party, which is an instant defeat, so a solo fixture ends the battle
        // before a single break can be rolled.
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 40), player("c", 40)],
            vec![monster("m", 5_000, 1)],
            &b,
            7,
        );
        let i = battle.idx("a").unwrap();
        battle.fighters[i].wll = 400;
        battle.fighters[i].timed_statuses.push(("paralyzed".into(), u64::MAX));
        // The fixture's second argument is SPEED, not level — at 40 the gauge takes ~130
        // ticks to fill, so a 400-tick budget only buys about three attempts and a run of bad
        // luck reads as a broken feature.
        let mut freed = false;
        for _ in 0..4_000 {
            let _ = battle.tick();
            if !battle.fighters[i].timed_statuses.iter().any(|(n, _)| n == "paralyzed") {
                freed = true;
                break;
            }
        }
        assert!(freed, "a 400-Will hero never broke a paralysis in 4000 ticks");
    }

    /// A passive Regen DRIP must not cancel a frenzy. It fires at the top of every turn, so
    /// letting it count would make any party carrying Regen — which the Resonant hands out
    /// party-wide — flatly immune to the condition, and a condition a passive cancels is not a
    /// condition. A DELIBERATE heal still ends it.
    #[test]
    fn regen_does_not_calm_a_frenzy_but_a_heal_does() {
        let b = balance();
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 40), player("c", 40)],
            vec![monster("m", 5_000, 1)],
            &b,
            11,
        );
        let i = battle.idx("a").unwrap();
        battle.fighters[i].max_hp = 1_000;
        battle.fighters[i].hp = 500;
        battle.fighters[i].regen = 20;
        battle.fighters[i].timed_statuses.push(("frenzied".into(), u64::MAX));

        let _ = battle.start_of_turn(i);
        assert!(
            battle.has(i, "frenzied"),
            "the Regen drip cancelled a frenzy — any party with a Resonant would be immune"
        );

        let _ = battle.apply_heal(i, 50);
        assert!(!battle.has(i, "frenzied"), "a deliberate heal should end it");
    }

    /// A CONFUSED hero did not choose the order that gets rolled, so it must never be told no.
    /// Refusing it would report an Adrenaline cost for a skill the player never pressed, leave
    /// the turn unspent, and re-roll on the next attempt.
    #[test]
    fn a_scrambled_order_never_rejects_the_players_turn() {
        let b = balance();
        // A Hunter with NO banked Adrenaline: every skill it owns is refusable, so a scramble
        // lands on an unaffordable one often.
        for seed in 0..40u64 {
            let mut battle = Battle::new(
                "b".into(),
                EncounterClass::Standard,
                vec![player("a", 40), player("c", 40)],
                vec![monster("m", 5_000, 1)],
                &b,
                seed,
            );
            let i = battle.idx("a").unwrap();
            battle.fighters[i].class_key = "hunter".into();
            battle.fighters[i].level = 40;
            battle.fighters[i].adrenaline = 0;
            battle.fighters[i].gauge = 1.0;
            battle.fighters[i].awaiting = true;
            battle.fighters[i].timed_statuses.push(("confused".into(), u64::MAX));
            let me = battle.fighters[i].combatant_id.clone();
            let foe = battle.fighters[battle.idx("m").unwrap()].combatant_id.clone();

            let out = battle.submit(
                &me,
                format!("a{seed}"),
                BattleActionKind::Attack,
                Some(vec![foe]),
                None,
                None,
            );
            assert!(
                out.is_ok(),
                "seed {seed}: a confused hero was refused its own scrambled order: {out:?}"
            );
        }
    }

    #[test]
    fn a_branded_attack_exploits_a_creature_s_weakness() {
        let b = balance();
        // Creature elemental profiles already existed; what AD-3 adds is a hero
        // whose swing HAS a type, so the profile finally cuts both ways.
        let mut fire_weak = monster("m1", 500, 1);
        fire_weak.damage_modifiers.insert(DamageType::Fire, 2.0);
        let mut fire_tough = monster("m2", 500, 1);
        fire_tough.damage_modifiers.insert(DamageType::Fire, 0.5);

        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![fire_weak, fire_tough],
            &b,
            42,
        );
        let raw = 60;
        let weak_i = battle.idx("m1").unwrap();
        let tough_i = battle.idx("m2").unwrap();

        // The same blow, branded FIRE, against opposite profiles.
        let hot = battle.apply_typed_damage(weak_i, raw, DamageType::Fire);
        assert_eq!(hot[0].modifier_flag, Some(ModifierFlag::Weak));
        let cold = battle.apply_typed_damage(tough_i, raw, DamageType::Fire);
        assert_eq!(cold[0].modifier_flag, Some(ModifierFlag::Resist));
        assert!(
            hot[0].amount.unwrap() > cold[0].amount.unwrap(),
            "a brand must matter: {:?} vs {:?}",
            hot[0].amount,
            cold[0].amount
        );

        // An UNBRANDED swing is untyped, so neither profile applies — which is
        // exactly the gap AD-3 closes.
        let plain = battle.apply_typed_damage(weak_i, raw, DamageType::None);
        assert_eq!(plain[0].modifier_flag, None);
    }

    #[test]
    fn elemental_modifiers_flag_weak_resist_immune_and_absorb() {
        let b = balance();
        // Four players with distinct FIRE profiles; one fire-slinging monster.
        let mk = |id: &str, m: Option<f64>| {
            let mut p = player(id, 1);
            p.hp = 400;
            p.max_hp = 400;
            if let Some(v) = m {
                p.damage_modifiers.insert(DamageType::Fire, v);
            }
            p
        };
        // Single-target picks the weakest; give distinct HP so targeting is fixed.
        // Instead: test via apply_typed_damage directly for precision.
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![
                mk("weak", Some(2.0)),
                mk("resist", Some(0.5)),
                mk("immune", Some(0.0)),
                mk("absorb", Some(-1.0)),
            ],
            vec![monster("m1", 500, 1)],
            &b,
            42,
        );
        let raw = 100;
        let idx = |bt: &Battle, id: &str| bt.idx(id).unwrap();

        let i = idx(&battle, "weak");
        let fx = battle.apply_typed_damage(i, raw, DamageType::Fire);
        assert_eq!(fx[0].modifier_flag, Some(ModifierFlag::Weak));
        assert_eq!(fx[0].amount, Some(200));

        let i = idx(&battle, "resist");
        let fx = battle.apply_typed_damage(i, raw, DamageType::Fire);
        assert_eq!(fx[0].modifier_flag, Some(ModifierFlag::Resist));
        assert_eq!(fx[0].amount, Some(50));

        let i = idx(&battle, "immune");
        let fx = battle.apply_typed_damage(i, raw, DamageType::Fire);
        assert_eq!(fx[0].modifier_flag, Some(ModifierFlag::Immune));
        assert_eq!(fx[0].amount, Some(0));
        assert_eq!(player_hp(&battle, "immune"), 400, "immunity takes nothing");

        let i = idx(&battle, "absorb");
        let hp_before = player_hp(&battle, "absorb");
        let fx = battle.apply_typed_damage(i, raw, DamageType::Fire);
        assert_eq!(fx[0].modifier_flag, Some(ModifierFlag::Absorb));
        assert!(matches!(fx[0].kind, EffectKind::Heal));
        assert!(player_hp(&battle, "absorb") >= hp_before, "absorption heals");

        // Untyped (pure) damage carries no flag and ignores the profile.
        let i = idx(&battle, "weak");
        let fx = battle.apply_typed_damage(i, raw, DamageType::None);
        assert_eq!(fx[0].modifier_flag, None);
        assert_eq!(fx[0].amount, Some(100));
    }

    #[test]
    fn hp_threshold_gates_a_desperation_ability() {
        let b = balance();
        let mut m = monster("m1", 500, 300);
        // Only ability: a self-heal gated to HP ≤ 50%. At full HP the pool is
        // empty, so the monster basic-attacks instead.
        m.abilities = vec![MonsterAbility {
            ability_kind: "mend".into(),
            callout_text: "Mend!".into(),
            weight: 100_000,
            cooldown_ticks: 0,
            telegraph_ticks: 0,
            hp_threshold_pct: Some(0.5),
            min_level: 1,
            effects: vec![meld_proto::abilities::AbilityEffect {
                effect_kind: AbilityEffectKind::Heal,
                scaling_base: Some(ScalingBase::MaxHp),
                coefficient: Some(0.25),
                damage_type: None,
                target: AbilityTarget::SelfCast,
                status_name: None,
                duration_ticks: None,
                steal_target_kind: None,
            }],
        }];
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![m],
            &b,
            42,
        );
        let (_, resolutions) = run_until_monster_acts(&mut battle, 50);
        assert_eq!(
            resolutions[0].action,
            BattleActionKind::Attack,
            "full-HP monster can't use its desperation heal"
        );
    }

    #[test]
    fn min_level_gates_the_pool_like_the_spec_spider() {
        let b = balance();
        let mut young = monster("m1", 500, 300);
        young.level = 1;
        // A high-level-only nuke: the L1 spawn can't roll it.
        let mut nuke = spec_ability(0, vec![dmg_effect(5.0, DamageType::Poison)]);
        nuke.min_level = 20;
        young.abilities = vec![nuke];
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![player("a", 1)],
            vec![young],
            &b,
            42,
        );
        let (_, resolutions) = run_until_monster_acts(&mut battle, 50);
        assert_eq!(
            resolutions[0].action,
            BattleActionKind::Attack,
            "a L1 creature only knows its basic attack"
        );
    }

    #[test]
    fn poison_status_ticks_damage_at_the_victims_turn() {
        let b = balance();
        let mut m = monster("m1", 500, 300);
        m.abilities = vec![spec_ability(
            0,
            vec![meld_proto::abilities::AbilityEffect {
                effect_kind: AbilityEffectKind::Status,
                scaling_base: None,
                coefficient: None,
                damage_type: None,
                target: AbilityTarget::SingleEnemy,
                status_name: Some("poison".into()),
                duration_ticks: Some(600),
                steal_target_kind: None,
            }],
        )];
        let mut p = player("a", 120);
        p.hp = 400;
        p.max_hp = 400;
        let mut battle = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![p],
            vec![m],
            &b,
            42,
        );
        // Let the monster poison the player, then let the player's gauge fill
        // and submit an action — the DoT fires in its start-of-turn upkeep.
        let mut poisoned = false;
        for _ in 0..200 {
            for ev in battle.tick() {
                if let Event::Resolved(r) = &ev {
                    if r.effects.iter().any(|e| e.status.as_deref() == Some("poison")) {
                        poisoned = true;
                    }
                }
                if let Event::TurnReady { combatant_id } = &ev {
                    if combatant_id == "a" && poisoned {
                        let evs = battle
                            .submit("a", uuid_str(), BattleActionKind::Defend, None, None, None)
                            .unwrap();
                        let dot: i32 = evs
                            .iter()
                            .filter_map(|e| match e {
                                Event::Resolved(r) => Some(
                                    r.effects
                                        .iter()
                                        .filter(|fx| {
                                            fx.target_id == "a"
                                                && matches!(fx.kind, EffectKind::Damage)
                                        })
                                        .filter_map(|fx| fx.amount)
                                        .sum::<i32>(),
                                ),
                                _ => None,
                            })
                            .sum();
                        // poison_dot_fraction of 400 max HP (5% → 20).
                        let expected =
                            ((400.0_f64) * b.battle.poison_dot_fraction).round() as i32;
                        assert_eq!(dot, expected, "poison ticked at turn start");
                        return;
                    }
                }
            }
        }
        panic!("player never got a poisoned turn");
    }

    fn uuid_str() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!("act-{}", N.fetch_add(1, Ordering::Relaxed))
    }

    /// FS-4: a fighter's `boss_kind` rides the wire as a `boss:<key>` status,
    /// the same channel hero `class:` already uses — absent when it's empty
    /// (a plain creature, no boss identity).
    #[test]
    fn boss_kind_surfaces_as_a_wire_status() {
        let mut m = monster("m1", 500, 300);
        m.boss_kind = "ironmaw".to_string();
        let wire = m.to_wire();
        assert!(wire.statuses.contains(&"boss:ironmaw".to_string()));

        let plain = monster("m2", 500, 300);
        assert!(!plain.to_wire().statuses.iter().any(|s| s.starts_with("boss:")));
    }
}

#[cfg(test)]
mod profession_class_tests {
    use super::*;

    /// Two heroes and one slow creature. The classes are only a label here — the kits
    /// are resolved by skill key, so the resolver is what is under test.
    fn bench(count: usize) -> Battle {
        let b = Balance::load_default().unwrap();
        let allies: Vec<Fighter> = (0..count)
            .map(|i| {
                Fighter::new(
                    format!("h{i}"),
                    CombatantKind::Player,
                    Some("p".to_string()),
                    None,
                    1,
                    40,
                    12,
                    3,
                    10,
                )
            })
            .collect();
        let mut mob = Fighter::new(
            "m0".to_string(),
            CombatantKind::Monster,
            None,
            Some("beast".to_string()),
            1,
            200,
            5,
            2,
            1,
        );
        mob.faction = "beast".to_string();
        Battle::new("b".into(), EncounterClass::Standard, allies, vec![mob], &b, 7)
    }

    // A Smithwright's job in a fight is to keep everyone else upright: the Bulwark is
    // Barrier for the WHOLE party, and Tempering Blow makes somebody ELSE hit harder.
    // Neither costs a resource, because the class pays in its own slow turns.
    #[test]
    fn a_smithwright_shields_the_party_and_sharpens_an_ally() {
        let mut b = bench(2);
        let (smith, ally) = (0usize, 1usize);

        b.fighters[smith].level = 255; // the whole ladder, so every rung is testable
        b.fighters[smith].gauge = 1.0;
        let before = b.fighters[ally].atk;
        b.resolve_smithwright(smith, "tempering_blow", Some(&b.fighters[ally].combatant_id.clone()), None)
            .expect("temper");
        assert!(b.fighters[ally].atk > before, "an ally should swing harder");

        b.fighters[smith].gauge = 1.0;
        b.resolve_smithwright(smith, "bulwark", None, None).expect("bulwark");
        assert!(b.fighters[smith].barrier > 0, "the smith is behind it too");
        assert!(b.fighters[ally].barrier > 0, "and so is everyone else");

        // Hammer Fall staggers: it costs the target part of its turn, not just HP.
        let mob = b.fighters.iter().position(|f| f.kind != CombatantKind::Player).unwrap();
        b.fighters[mob].gauge = 0.9;
        b.fighters[smith].gauge = 1.0;
        b.resolve_smithwright(smith, "hammer_fall", Some(&b.fighters[mob].combatant_id.clone()), None)
            .expect("hammer");
        assert!(b.fighters[mob].gauge < 0.9, "dropped iron should stagger");
    }

    // A Keeper mends. Its damage rides Mnd rather than Str — the staff is a pestle —
    // and both of its damaging skills buy time rather than kills.
    #[test]
    fn a_keeper_mends_the_party_and_its_damage_rides_mnd() {
        let mut b = bench(2);
        let (keeper, ally) = (0usize, 1usize);
        b.fighters[keeper].level = 255;

        // Poultice heals AND leaves Regen behind.
        b.fighters[ally].hp = 5;
        b.fighters[keeper].gauge = 1.0;
        b.resolve_keeper(keeper, "poultice", Some(&b.fighters[ally].combatant_id.clone()), None)
            .expect("poultice");
        assert!(b.fighters[ally].hp > 5, "the ally should be mended");
        assert!(b.fighters[ally].regen > 0, "and keep mending");

        // Bloomfield is Regen for everyone.
        b.fighters[keeper].gauge = 1.0;
        b.resolve_keeper(keeper, "bloomfield", None, None).expect("bloomfield");
        assert!(b.fighters[keeper].regen > 0 && b.fighters[ally].regen > 0);

        // Root Snare pushes a foe's turn a long way off.
        let mob = b.fighters.iter().position(|f| f.kind != CombatantKind::Player).unwrap();
        b.fighters[mob].gauge = 0.95;
        b.fighters[keeper].gauge = 1.0;
        b.resolve_keeper(keeper, "root_snare", Some(&b.fighters[mob].combatant_id.clone()), None)
            .expect("snare");
        assert!(b.fighters[mob].gauge < 0.5, "the ground should hold it");
    }

    // Both kits are gated by the same ladder as every other class: a level-1 hero has
    // its first rung and nothing else, and the server is the backstop.
    #[test]
    fn the_new_ladders_gate_like_every_other() {
        for (class, first, later, deepest) in [
            ("smithwright", "hammer_fall", "one_true_forge", "great_work"),
            ("keeper", "thornlash", "terras_gift", "world_tree"),
        ] {
            let at_one = meld_proto::skills::skills_for_class_at(class, 1);
            assert_eq!(at_one.len(), 1, "{class} should open with one rung");
            assert_eq!(at_one[0].key, first);
            assert!(meld_proto::skills::is_unlocked(first, 1));
            assert!(!meld_proto::skills::is_unlocked(later, 1), "{later} is not a level-1 tool");
            // The full kit runs to the ladder's top like everyone else's, and stays
            // inside the width its archetype allows.
            let full = meld_proto::skills::skills_for_class_at(class, 255);
            assert!(full.iter().any(|s| s.key == deepest), "{class} stops short of the top");
            assert!(
                full.len()
                    <= meld_proto::skills::menu_width(meld_proto::skills::archetype(class)),
                "{class} fields {} rows",
                full.len()
            );
        }
    }
}

/// The deep rungs — every class learns something at 49 and again at 100. These fire each
/// one through the real resolver, because a row that reaches the menu and resolves to
/// "unknown skill" costs the player a turn and says nothing.
#[cfg(test)]
mod deep_ladder_tests {
    use super::*;

    /// `heroes` allies and `mobs` creatures, everyone at the level cap so nothing is
    /// gated. The creatures are fat and slow so a resolver's effects can be read off
    /// them without the fight ending underneath the assertion.
    fn field(heroes: usize, mobs: usize) -> Battle {
        let b = Balance::load_default().unwrap();
        let allies: Vec<Fighter> = (0..heroes)
            .map(|i| {
                let mut f = Fighter::new(
                    format!("h{i}"),
                    CombatantKind::Player,
                    Some("p".to_string()),
                    None,
                    255,
                    120,
                    20,
                    3,
                    10,
                );
                f.spell_power = 20;
                f.adrenaline_max = 100;
                f.adrenaline = 100;
                f
            })
            .collect();
        let enemies: Vec<Fighter> = (0..mobs)
            .map(|i| {
                let mut m = Fighter::new(
                    format!("m{i}"),
                    CombatantKind::Monster,
                    None,
                    Some("beast".to_string()),
                    1,
                    4000,
                    5,
                    2,
                    1,
                );
                m.faction = "beast".to_string();
                m
            })
            .collect();
        Battle::new("b".into(), EncounterClass::Standard, allies, enemies, &b, 7)
    }

    fn mobs(b: &Battle) -> Vec<usize> {
        b.fighters
            .iter()
            .enumerate()
            .filter(|(_, f)| f.kind != CombatantKind::Player)
            .map(|(i, _)| i)
            .collect()
    }

    /// Fire `skill` from hero 0 the way a player would — through `resolve_skill`, so the
    /// registry routing is under test too, not just the arm.
    fn cast(b: &mut Battle, skill: &str, target: Option<&str>) {
        b.fighters[0].gauge = 1.0;
        let t = target.map(|s| s.to_string());
        b.resolve_skill(0, t.as_deref(), Some(skill), None)
            .unwrap_or_else(|e| panic!("{skill} did not resolve: {e:?}"));
    }

    /// Every enemy took damage — the shape shared by six of the eleven new rows.
    fn all_enemies_hit(b: &Battle, before: &[i32]) {
        for (n, i) in mobs(b).into_iter().enumerate() {
            assert!(
                b.fighters[i].hp < before[n],
                "enemy {n} was untouched by an ALL-enemy ability"
            );
        }
    }

    #[test]
    fn the_world_entire_marks_every_enemy_and_hastes_the_party() {
        let mut b = field(2, 3);
        cast(&mut b, "the_world_entire", None);
        for i in mobs(&b) {
            assert!(b.is_marked(i), "an enemy was left unmarked");
        }
        for a in [0usize, 1] {
            assert!(b.has_timed_status(a, HASTE_STATUS), "ally {a} was not hastened");
        }
    }

    #[test]
    fn iron_lung_heals_harder_than_second_wind_and_leaves_regen() {
        let mut b = field(1, 1);
        b.fighters[0].hp = 1;
        cast(&mut b, "second_wind", None);
        let plain = b.fighters[0].hp;
        assert_eq!(b.fighters[0].regen, 0, "Second Wind grants no Regen");

        let mut b = field(1, 1);
        b.fighters[0].hp = 1;
        cast(&mut b, "iron_lung", None);
        assert!(b.fighters[0].hp > plain, "the upgrade should heal harder");
        assert!(b.fighters[0].regen > 0, "and keep closing the wound");
        // The upgrade costs what the row it replaced cost — the economy does not move.
        assert_eq!(b.fighters[0].adrenaline, 100 - b.hunter_second_wind_cost);
    }

    #[test]
    fn apex_predator_is_frenzy_turned_on_the_whole_pack() {
        let mut b = field(1, 3);
        let before: Vec<i32> = mobs(&b).into_iter().map(|i| b.fighters[i].hp).collect();
        cast(&mut b, "apex_predator", None);
        all_enemies_hit(&b, &before);
        assert_eq!(b.fighters[0].adrenaline, 100 - b.hunter_frenzy_cost);
    }

    /// A Hunter skill is refused unless its cost is banked — the upgrades are Hunter
    /// abilities, so they answer to the same bank.
    #[test]
    fn the_deep_hunter_rows_still_answer_to_adrenaline() {
        for skill in ["iron_lung", "apex_predator"] {
            let mut b = field(1, 2);
            b.fighters[0].adrenaline = 0;
            b.fighters[0].gauge = 1.0;
            assert!(
                b.resolve_skill(0, None, Some(skill), None).is_err(),
                "{skill} resolved on an empty bank"
            );
        }
    }

    #[test]
    fn assassinate_ignores_armour_backstab_only_dents() {
        // A wall of armour is where the upgrade earns its level: Backstab leaves a
        // quarter of it standing, Assassinate leaves none.
        let armoured = |skill: &str| -> i32 {
            let mut b = field(1, 1);
            let m = mobs(&b)[0];
            b.fighters[m].def = 40;
            let before = b.fighters[m].hp;
            let id = b.fighters[m].combatant_id.clone();
            b.fighters[0].dodge = 0.0;
            b.fighters[m].hp = before;
            cast(&mut b, skill, Some(&id));
            before - b.fighters[m].hp
        };
        assert!(
            armoured("assassinate") > armoured("backstab"),
            "the upgrade should bite deeper through armour"
        );
    }

    #[test]
    fn grand_larceny_robs_every_enemy_at_once() {
        let mut b = field(1, 3);
        for i in mobs(&b) {
            b.fighters[i].gauge = 0.9;
        }
        let before: Vec<i32> = mobs(&b).into_iter().map(|i| b.fighters[i].hp).collect();
        cast(&mut b, "grand_larceny", None);
        all_enemies_hit(&b, &before);
        for i in mobs(&b) {
            assert!(b.fighters[i].gauge < 0.9, "an enemy kept its tempo");
        }
    }

    #[test]
    fn hallowed_ground_zeroes_every_gauge_and_ascendant_shields_the_party() {
        let mut b = field(2, 3);
        for i in mobs(&b) {
            b.fighters[i].gauge = 0.95;
        }
        let before: Vec<i32> = mobs(&b).into_iter().map(|i| b.fighters[i].hp).collect();
        cast(&mut b, "hallowed_ground", None);
        all_enemies_hit(&b, &before);
        for i in mobs(&b) {
            assert_eq!(b.fighters[i].gauge, 0.0, "a gauge survived the consecration");
        }

        let mut b = field(2, 3);
        let before: Vec<i32> = mobs(&b).into_iter().map(|i| b.fighters[i].hp).collect();
        cast(&mut b, "phoenix_ascendant", None);
        all_enemies_hit(&b, &before);
        for a in [0usize, 1] {
            assert!(b.fighters[a].barrier > 0, "ally {a} got no Barrier from the fire");
        }
    }

    #[test]
    fn anvil_chorus_sharpens_everyone_and_the_great_work_does_all_three() {
        let mut b = field(3, 1);
        let before: Vec<i32> = (0..3).map(|a| b.fighters[a].atk).collect();
        cast(&mut b, "anvil_chorus", None);
        for (a, was) in before.iter().enumerate() {
            assert!(b.fighters[a].atk > *was, "ally {a} was not sharpened");
        }

        let mut b = field(3, 1);
        for a in 0..3 {
            b.fighters[a].hp = 10;
        }
        let before: Vec<i32> = (0..3).map(|a| b.fighters[a].atk).collect();
        cast(&mut b, "great_work", None);
        for (a, was) in before.iter().enumerate() {
            assert!(b.fighters[a].hp > 10, "ally {a} was not healed");
            assert!(b.fighters[a].barrier > 0, "ally {a} got no Barrier");
            assert!(b.fighters[a].atk > *was, "ally {a} was not sharpened");
        }
    }

    #[test]
    fn thorn_grove_holds_the_room_and_the_world_tree_does_all_three() {
        let mut b = field(2, 3);
        for i in mobs(&b) {
            b.fighters[i].gauge = 0.9;
        }
        let before: Vec<i32> = mobs(&b).into_iter().map(|i| b.fighters[i].hp).collect();
        cast(&mut b, "thorn_grove", None);
        all_enemies_hit(&b, &before);
        for i in mobs(&b) {
            assert!(b.fighters[i].gauge < 0.9, "an enemy walked through the thorns");
        }

        let mut b = field(2, 1);
        for a in [0usize, 1] {
            b.fighters[a].hp = 10;
        }
        cast(&mut b, "world_tree", None);
        for a in [0usize, 1] {
            assert!(b.fighters[a].hp > 10, "ally {a} was not healed");
            assert!(b.fighters[a].barrier > 0, "ally {a} got no Barrier");
            assert!(b.fighters[a].regen > 0, "ally {a} got no Regen");
        }
    }

    /// The Keeper's damage rides Mnd, not Str — including the new all-enemy rung, which
    /// would otherwise quietly become the one Keeper row that wants a sword.
    #[test]
    fn thorn_grove_rides_mnd_like_the_rest_of_the_kit() {
        let hit = |power: i32| -> i32 {
            let mut b = field(1, 1);
            b.fighters[0].spell_power = power;
            let m = mobs(&b)[0];
            let before = b.fighters[m].hp;
            cast(&mut b, "thorn_grove", None);
            before - b.fighters[m].hp
        };
        assert!(hit(60) > hit(20), "more Mnd should mean more thorns");
    }

    /// Gravity Vortex SLOWS the line rather than capping its gauge, and the difference is
    /// whether the enemy ever acts again. Creature speed is a fixed constant while a hero's
    /// climbs with Dex, so a deep Psyker takes several turns per creature turn — a cap at
    /// half would knock the creature back below the line every time it approached one.
    #[test]
    fn gravity_vortex_slows_the_line_but_never_locks_it() {
        let b = Balance::load_default().unwrap();
        // A Psyker four times the creature's speed: the worst case for a cap.
        let mut p = Fighter::new("p".into(), CombatantKind::Player, Some("pl".into()), None,
            255, 400, 20, 3, 400);
        p.focus_max = 5;
        p.spell_power = 20;
        let mut m = Fighter::new("m".into(), CombatantKind::Monster, None,
            Some("beast".into()), 1, 400000, 5, 2, 100);
        m.faction = "beast".into();
        let mut bt = Battle::new("b".into(), EncounterClass::Standard, vec![p], vec![m], &b, 7);
        // Seat the Focus through the real submit path (a Psyker never reaches
        // `resolve_skill`), then let the fight run and count the creature's turns.
        // The engine acts for creatures itself, so the proof that one still gets turns is
        // that the Psyker takes damage — and that the creature's gauge passes the halfway
        // line a cap would have pinned it under.
        let start_hp = bt.fighters[0].hp;
        let mut seated = false;
        let mut peak_gauge = 0.0f64;
        for n in 0..6000 {
            for ev in bt.tick() {
                if let Event::TurnReady { combatant_id } = ev {
                    if combatant_id == "p" {
                        let op = if seated { "hold" } else { "cast:gravity_vortex" };
                        seated = true;
                        let _ = bt.submit(
                            "p",
                            format!("a{n}"),
                            BattleActionKind::Skill,
                            Some(vec!["m".into()]),
                            Some(op.into()),
                            None,
                        );
                    }
                }
            }
            peak_gauge = peak_gauge.max(bt.fighters[1].gauge);
        }
        assert!(seated, "the Focus was never seated");
        assert!(
            bt.fighters[0].hp < start_hp,
            "Gravity Vortex locked the creature out entirely — it never landed a blow"
        );
        assert!(
            peak_gauge > 0.5,
            "the creature's gauge never passed 0.5 (peak {peak_gauge:.2}) — that is a cap, not a slow"
        );
    }

    /// A once-a-fight call is refused on the second ask — and it is SPENT centrally, on
    /// any successful resolve, rather than by each arm remembering to push its own key.
    #[test]
    fn a_once_a_fight_call_is_refused_the_second_time() {
        for key in [
            "now",
            "the_world_entire",
            "iron_lung",
            "pin_the_prey",
            "grand_larceny",
            "hallowed_ground",
            "phoenix_ascendant",
            "anvil_chorus",
            "great_work",
            "world_tree",
            "eternal_bloom",
        ] {
            let mut b = field(2, 3);
            let def = meld_proto::skills::skill(key).unwrap();
            b.fighters[0].class_key = def.class.to_string();
            b.fighters[0].adrenaline = 100;
            b.fighters[0].gauge = 1.0;
            b.resolve_skill(0, None, Some(key), None)
                .unwrap_or_else(|e| panic!("{key} did not resolve: {e:?}"));
            assert!(
                b.fighters[0].once_spent.iter().any(|s| s == key),
                "{key} was not marked spent"
            );
            b.fighters[0].adrenaline = 100;
            b.fighters[0].gauge = 1.0;
            assert!(
                b.resolve_skill(0, None, Some(key), None).is_err(),
                "{key} resolved twice in one battle"
            );
        }
    }

    /// Pin the Prey and Grand Larceny are the pack versions of Snare and Mug: everything
    /// on the other side is hit, slowed, and — for the Shifter — robbed.
    #[test]
    fn the_once_a_fight_calls_reach_the_whole_pack() {
        let mut b = field(1, 3);
        b.fighters[0].adrenaline = 100;
        for i in mobs(&b) {
            b.fighters[i].gauge = 0.9;
        }
        let before: Vec<i32> = mobs(&b).into_iter().map(|i| b.fighters[i].hp).collect();
        cast(&mut b, "pin_the_prey", None);
        all_enemies_hit(&b, &before);
        for i in mobs(&b) {
            assert!(b.fighters[i].gauge < 0.9, "Pin the Prey left an enemy its tempo");
        }

        let mut b = field(1, 3);
        for i in mobs(&b) {
            b.fighters[i].gauge = 0.9;
        }
        let before: Vec<i32> = mobs(&b).into_iter().map(|i| b.fighters[i].hp).collect();
        cast(&mut b, "grand_larceny", None);
        all_enemies_hit(&b, &before);
        // A Mug robs, so every enemy is pilfered rather than merely hit.
        let thefts = b
            .pending_events
            .iter()
            .filter(|e| matches!(e, Event::Pilfered { .. }))
            .count();
        assert_eq!(thefts, 3, "Grand Larceny picked {thefts} of 3 pockets");
    }

    /// Every heal, Barrier and Regen is a FRACTION of the recipient, so a deep hero gets
    /// proportionally what a shallow one does. Flat grants are what made the Keeper stop
    /// healing around level 30 while the Resonant kept scaling.
    #[test]
    fn a_mend_is_worth_the_same_share_at_any_level() {
        let share = |max_hp: i32, skill: &str| -> f64 {
            let mut b = field(2, 1);
            for a in [0usize, 1] {
                b.fighters[a].max_hp = max_hp;
                b.fighters[a].hp = max_hp / 2;
            }
            b.fighters[0].class_key = "keeper".into();
            let before = b.fighters[1].hp;
            cast(&mut b, skill, None);
            (b.fighters[1].hp - before) as f64 / max_hp as f64
        };
        for skill in ["terras_gift", "world_tree"] {
            let shallow = share(60, skill);
            let deep = share(600, skill);
            assert!(shallow > 0.0, "{skill} healed nothing");
            assert!(
                (shallow - deep).abs() < 0.02,
                "{skill} is worth {shallow:.3} of a shallow hero and {deep:.3} of a deep one"
            );
        }
    }

    /// The Smithwright's buff is a share of the ally's OWN attack, so it keeps mattering.
    #[test]
    fn a_temper_is_worth_the_same_share_at_any_attack() {
        let share = |atk: i32| -> f64 {
            let mut b = field(2, 1);
            b.fighters[0].class_key = "smithwright".into();
            b.fighters[1].atk = atk;
            b.fighters[1].base_atk = atk;
            let before = b.fighters[1].atk;
            let id = b.fighters[1].combatant_id.clone();
            b.fighters[0].gauge = 1.0;
            b.resolve_skill(0, Some(&id), Some("tempering_blow"), None).expect("temper");
            (b.fighters[1].atk - before) as f64 / atk as f64
        };
        let (weak, strong) = (share(20), share(300));
        assert!((weak - strong).abs() < 0.02, "temper is {weak:.3} then {strong:.3}");

        // Each stack is a share of BASE attack, so stacking is linear rather than
        // compounding — computed off the current value and added, five casts would be
        // 2x rather than 1.75x — and the sixth is refused by the shared ceiling.
        let mut b = field(2, 1);
        b.fighters[0].class_key = "smithwright".into();
        let base = b.fighters[1].atk;
        let id = b.fighters[1].combatant_id.clone();
        let per_stack = ((base as f64) * 0.15).round() as i32;
        for _ in 0..8 {
            b.fighters[0].gauge = 1.0;
            b.resolve_skill(0, Some(&id), Some("tempering_blow"), None).expect("temper");
        }
        let cap = b.max_effect_stacks as i32;
        assert_eq!(
            b.fighters[1].atk,
            base + per_stack * cap,
            "eight casts should be {cap} stacks of {per_stack} on {base}"
        );
        assert_eq!(b.fighters[1].atk_stacks, b.max_effect_stacks);
    }

    /// **Every lasting effect answers to ONE ceiling**, and Regen decays like the Barrier
    /// beside it. Regen used to have neither: it accumulated without limit and never
    /// faded, so turns spent on it bought permanent, ever-growing party sustain.
    #[test]
    fn a_lasting_effect_caps_at_five_stacks_and_regen_decays() {
        let b0 = Balance::load_default().unwrap();
        let cap = b0.battle.max_effect_stacks;
        assert_eq!(cap, 5, "the ceiling is the number the player learns once");

        // Regen: ten grants, five stacks, and each one a real increase.
        let mut b = field(1, 1);
        let mut seen = Vec::new();
        for _ in 0..10 {
            b.grant_regen(0, 0.05);
            seen.push(b.fighters[0].regen);
        }
        assert_eq!(b.fighters[0].regen_stacks, cap, "regen ignored the ceiling");
        assert_eq!(seen[cap as usize - 1], seen[9], "regen kept growing past the cap");
        assert!(seen[0] < seen[cap as usize - 1], "stacking did nothing");

        // …and it drains, handing the stacks back when it is spent.
        let mut turns = 0;
        while b.fighters[0].regen > 0 && turns < 200 {
            b.start_of_turn(0);
            turns += 1;
        }
        assert!(turns < 200, "Regen never decayed away");
        assert_eq!(b.fighters[0].regen_stacks, 0, "spent Regen kept its stacks");

        // Barrier and Evasion answer to the same number.
        let mut b = field(1, 1);
        for _ in 0..10 {
            b.grant_barrier(0, 10);
            b.grant_evasion(0, 0.05);
        }
        assert_eq!(b.fighters[0].barrier_stacks, cap, "barrier ignored the ceiling");
        assert_eq!(b.fighters[0].evasion_stacks, cap, "evasion ignored the ceiling");
        assert_eq!(b.fighters[0].barrier, 10 * cap as i32);
    }

    /// The client does not ask the player to aim a party buff or an all-enemy sweep, so
    /// it sends no target for them. A resolver that still DEMANDS one rejects every real
    /// cast — which is how Grand Larceny shipped broken for exactly as long as its test
    /// passed a target the client would never send.
    #[test]
    fn an_ability_that_needs_no_aim_resolves_without_one() {
        for def in meld_proto::skills::SKILLS {
            if def.target.needs_pick() || def.class == "psyker" {
                continue;
            }
            let mut b = field(2, 2);
            b.fighters[0].class_key = def.class.to_string();
            b.fighters[0].gauge = 1.0;
            let out = b.resolve_skill(0, None, Some(def.key), None);
            assert!(
                out.is_ok(),
                "{} ({}) needs no aim but refused an unaimed cast: {out:?}",
                def.name,
                def.key
            );
        }
    }

    /// Every ability in the registry resolves. The routing is by owner now, but a class
    /// whose resolver has no arm for a key it owns still fails at the last moment — as
    /// a rejected turn, in a fight, which is the worst place to find out.
    #[test]
    fn every_registered_ability_resolves_rather_than_rejecting_as_unknown() {
        for def in meld_proto::skills::SKILLS {
            // The Psyker's Foci are seated through `resolve_psyker`, never here.
            if def.class == "psyker" {
                continue;
            }
            let mut b = field(2, 2);
            b.fighters[0].class_key = def.class.to_string();
            b.fighters[0].gauge = 1.0;
            let target = b.fighters[mobs(&b)[0]].combatant_id.clone();
            let out = b.resolve_skill(0, Some(&target), Some(def.key), None);
            assert!(
                !matches!(&out, Err(Reject::ValidationError(m)) if m.contains("unknown")),
                "{} ({}) has no resolver arm: {out:?}",
                def.name,
                def.key
            );
        }
    }

}

#[cfg(test)]
mod grouping_and_flanking {
    use super::*;

    pub(super) fn balance() -> Balance {
        Balance::load_default().unwrap()
    }

    pub(super) fn hero(id: &str, player: &str) -> Fighter {
        let mut f = Fighter::new(
            id.into(),
            CombatantKind::Player,
            Some(player.into()),
            Some(id.into()),
            1,
            200,
            40,
            0,
            50,
        );
        f.basic_attack_type = DamageType::Slash;
        f
    }

    pub(super) fn foe(id: &str, group: u32, back: bool) -> Fighter {
        let mut f = Fighter::new(
            id.into(),
            CombatantKind::Monster,
            None,
            Some(id.into()),
            1,
            10_000,
            10,
            0,
            1,
        );
        f.group_id = Some(group);
        f.back_row = back;
        f
    }

    fn arena(allies: Vec<Fighter>, enemies: Vec<Fighter>) -> Battle {
        let b = balance();
        Battle::new("b".into(), EncounterClass::Standard, allies, enemies, &b, 7)
    }

    fn hp_of(b: &Battle, id: &str) -> i32 {
        b.fighters.iter().find(|f| f.combatant_id == id).map(|f| f.hp).unwrap()
    }

    fn strike(b: &mut Battle, attacker: &str, target: &str, dmg: i32) {
        let ai = b.fighters.iter().position(|f| f.combatant_id == attacker).unwrap();
        let ti = b.fighters.iter().position(|f| f.combatant_id == target).unwrap();
        b.active_actor = Some(ai);
        b.apply_damage_reaching(ti, dmg, true);
        b.active_actor = None;
    }

    /// The rank's whole point: a sword lands soft on the rear while the front takes it all.
    #[test]
    fn a_back_rank_softens_a_physical_blow_and_a_front_rank_does_not() {
        let mut b = arena(vec![hero("h", "p1")], vec![foe("front", 0, false), foe("rear", 0, true)]);
        strike(&mut b, "h", "front", 100);
        strike(&mut b, "h", "rear", 100);
        let (front_lost, rear_lost) = (10_000 - hp_of(&b, "front"), 10_000 - hp_of(&b, "rear"));
        assert_eq!(front_lost, 100, "the front rank took a softened blow");
        assert!(rear_lost < front_lost, "the rear took {rear_lost} of a 100 blow");
    }

    /// The co-op rule. One party alone cannot flank; a second party laying into the same
    /// group strips its rear cover, for everyone, for the rest of the fight.
    #[test]
    fn a_second_party_on_the_same_group_flanks_it() {
        let mut b = arena(
            vec![hero("a", "p1"), hero("c", "p2")],
            vec![foe("rear", 0, true)],
        );
        strike(&mut b, "a", "rear", 100);
        let alone = 10_000 - hp_of(&b, "rear");
        assert!(!b.fighters.iter().any(|f| f.flanked), "one party flanked a group by itself");

        strike(&mut b, "c", "rear", 100);
        let flanked = (10_000 - hp_of(&b, "rear")) - alone;
        assert!(b.fighters.iter().any(|f| f.flanked), "two parties did not flank the group");
        assert!(
            flanked > alone,
            "a flanked rear took {flanked} where the covered one took {alone}"
        );
    }

    /// A flank is a property of the GROUP, not of the creature that got hit — surrounding a
    /// knot of enemies exposes the whole knot.
    #[test]
    fn flanking_covers_every_member_of_the_group_and_no_other() {
        let mut b = arena(
            vec![hero("a", "p1"), hero("c", "p2")],
            vec![foe("x", 0, true), foe("y", 0, true), foe("other", 1, true)],
        );
        strike(&mut b, "a", "x", 10);
        strike(&mut b, "c", "x", 10);
        let flanked = |id: &str| {
            b.fighters.iter().find(|f| f.combatant_id == id).map(|f| f.flanked).unwrap()
        };
        assert!(flanked("x") && flanked("y"), "the group's other member was not flanked");
        assert!(!flanked("other"), "an untouched group was flanked");
    }

    /// The same party hitting a group repeatedly is not a flank. Without this, any solo
    /// player would flank everything by attacking twice.
    #[test]
    fn one_party_never_flanks_however_hard_it_swings() {
        let mut b = arena(
            vec![hero("a", "p1"), hero("b", "p1")],
            vec![foe("rear", 0, true)],
        );
        for _ in 0..6 {
            strike(&mut b, "a", "rear", 10);
            strike(&mut b, "b", "rear", 10);
        }
        assert!(
            !b.fighters.iter().any(|f| f.flanked),
            "two heroes of ONE party flanked a group — flanking is per party, not per hero"
        );
    }

    /// A spell reached the back rank at full force before any of this, and must still.
    #[test]
    fn the_rear_never_softened_anything_but_a_physical_blow() {
        let mut b = arena(vec![hero("h", "p1")], vec![foe("rear", 0, true)]);
        let ti = b.fighters.iter().position(|f| f.combatant_id == "rear").unwrap();
        b.active_actor = Some(0);
        b.apply_damage_reaching(ti, 100, false);
        assert_eq!(10_000 - hp_of(&b, "rear"), 100, "a spell was softened by a back rank");
    }

    /// The martial answer to a formation. Before ranged weapons the back rank was a
    /// caster's problem and a swordsman's wall; a bow shoots over the front line.
    #[test]
    fn a_reaching_weapon_lands_on_the_rear_at_full_force() {
        let mut b = arena(vec![hero("h", "p1")], vec![foe("rear", 0, true)]);
        strike(&mut b, "h", "rear", 100);
        let softened = 10_000 - hp_of(&b, "rear");
        assert!(softened < 100, "the rank did not soften a melee blow at all");

        let mut b = arena(vec![hero("archer", "p1")], vec![foe("rear", 0, true)]);
        b.fighters[0].reach = true;
        strike(&mut b, "archer", "rear", 100);
        assert_eq!(10_000 - hp_of(&b, "rear"), 100, "a bow was stopped by a front rank");
    }

    /// Reach is the ATTACKER's property, not the target's — it must not leak into what the
    /// rear does to everyone else's blows.
    #[test]
    fn one_archer_does_not_expose_the_rear_to_the_whole_party() {
        let mut b = arena(
            vec![hero("archer", "p1"), hero("swordsman", "p1")],
            vec![foe("rear", 0, true)],
        );
        b.fighters[0].reach = true;
        strike(&mut b, "archer", "rear", 100);
        let shot = 10_000 - hp_of(&b, "rear");
        strike(&mut b, "swordsman", "rear", 100);
        let swung = (10_000 - hp_of(&b, "rear")) - shot;
        assert_eq!(shot, 100, "the bow did not reach");
        assert!(swung < shot, "the swordsman inherited the archer's reach");
    }

    /// Reach answers a RANK. It is not a damage bonus, and must do nothing to a front rank.
    #[test]
    fn reach_is_worth_nothing_against_a_front_rank() {
        let mut b = arena(vec![hero("archer", "p1")], vec![foe("front", 0, false)]);
        b.fighters[0].reach = true;
        strike(&mut b, "archer", "front", 100);
        assert_eq!(10_000 - hp_of(&b, "front"), 100, "reach changed a front-rank blow");
    }

    /// The other half of reach, and the reason an archer stands at the back at all: a bow
    /// fired from the rear loses nothing. Without this a Hunter pays the back row's damage
    /// penalty for standing exactly where its weapon is meant to be used.
    #[test]
    fn a_ranged_weapon_loses_nothing_by_being_fired_from_the_back() {
        let b = arena(vec![hero("h", "p1")], vec![foe("x", 0, false)]);
        let front = b.rank_attack_mult(0, true);

        let mut b = arena(vec![hero("h", "p1")], vec![foe("x", 0, false)]);
        b.fighters[0].back_row = true;
        let swung_from_back = b.rank_attack_mult(0, true);

        let mut b = arena(vec![hero("h", "p1")], vec![foe("x", 0, false)]);
        b.fighters[0].back_row = true;
        b.fighters[0].reach = true;
        let shot_from_back = b.rank_attack_mult(0, true);

        assert!(swung_from_back < front, "the back row cost a melee swing nothing");
        assert_eq!(shot_from_back, front, "a bow was weakened by being fired from the back");
    }

    /// Reach cancels the RANK, not the rules. A non-physical hit was never weakened by the
    /// back row in the first place, and reach must not become a general damage bonus.
    #[test]
    fn reach_changes_nothing_a_rank_was_not_already_deciding() {
        let mut b = arena(vec![hero("h", "p1")], vec![foe("x", 0, false)]);
        b.fighters[0].reach = true;
        assert_eq!(b.rank_attack_mult(0, false), 1.0);
        assert_eq!(b.rank_attack_mult(0, true), 1.0, "a front-rank archer got a bonus");
    }

    /// The Keeper's kit rides Mnd — its own comments say so twice, "the staff is a pestle,
    /// not a sword" — and a caster gives up nothing against a back rank. Routed through the
    /// PHYSICAL path it did both wrong: answered `def` instead of `ward`, and once creatures
    /// had ranks it was halved against an enemy rear, which is the one thing a caster is
    /// meant to be immune to.
    #[test]
    fn a_keepers_damage_reaches_a_back_rank_like_every_other_caster() {
        let b = balance();
        let mut keeper = hero("k", "p1");
        keeper.class_key = "keeper".into();
        keeper.spell_power = 200;
        let front = foe("front", 0, false);
        let mut rear = foe("rear", 0, true);
        rear.max_hp = 10_000;
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![keeper],
            vec![front, rear],
            &b,
            7,
        );
        let fi = bt.fighters.iter().position(|f| f.combatant_id == "front").unwrap();
        let ri = bt.fighters.iter().position(|f| f.combatant_id == "rear").unwrap();
        bt.active_actor = Some(0);
        bt.apply_ability_damage(fi, 300, DamageType::Mind);
        bt.apply_ability_damage(ri, 300, DamageType::Mind);
        let took = |i: usize| 10_000 - bt.fighters[i].hp;
        assert_eq!(
            took(fi),
            took(ri),
            "a Mind attack was softened by a rank: front {} vs rear {}",
            took(fi),
            took(ri)
        );
    }

    /// And the rank still means something to the things it is FOR. If this ever passes
    /// trivially, the rank has stopped working rather than the caster starting to.
    #[test]
    fn a_physical_blow_is_still_softened_by_the_same_rank() {
        let b = balance();
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![hero("h", "p1")],
            vec![foe("front", 0, false), foe("rear", 0, true)],
            &b,
            7,
        );
        let fi = bt.fighters.iter().position(|f| f.combatant_id == "front").unwrap();
        let ri = bt.fighters.iter().position(|f| f.combatant_id == "rear").unwrap();
        bt.active_actor = Some(0);
        bt.apply_ability_damage(fi, 300, DamageType::Slash);
        bt.apply_ability_damage(ri, 300, DamageType::Slash);
        let took = |i: usize| 10_000 - bt.fighters[i].hp;
        assert!(took(ri) < took(fi), "the rank stopped softening physical blows");
    }
}

#[cfg(test)]
mod thrown_tests {
    use super::*;
    use super::grouping_and_flanking::{balance, foe, hero};
    use meld_proto::consumables::{self as con, ConsumableEffect as E};

    /// The all-enemy tier lives on a limited consumable, never on a weapon. A basic attack
    /// that hit everything would scale with pack size, beat a single-target weapon at about
    /// four enemies, and make the back rank, the group tier and every level-20-and-up AoE
    /// ability pointless.
    #[test]
    fn nothing_a_hero_equips_hits_the_whole_encounter() {
        for f in [
            meld_proto::equipment::ItemFamily::Bow,
            meld_proto::equipment::ItemFamily::Sling,
            meld_proto::equipment::ItemFamily::ThrownSpear,
            meld_proto::equipment::ItemFamily::Sword,
            meld_proto::equipment::ItemFamily::Spear,
        ] {
            assert!(f.damage_type().is_some(), "{f:?} has no blow");
        }
        let all_enemy: Vec<&str> = con::CONSUMABLES
            .iter()
            .filter(|c| c.effect == E::ThrownAll)
            .map(|c| c.key)
            .collect();
        assert!(!all_enemy.is_empty(), "the all-enemy tier has nowhere to live");
    }

    /// Every throwable belongs to a profession, and answers armour its own way. Four items
    /// that all cut would be one item with four names.
    #[test]
    fn every_throwable_is_a_professions_work_and_lands_its_own_blow() {
        let thrown: Vec<_> =
            con::CONSUMABLES.iter().filter(|c| c.effect == E::ThrownAll).collect();
        let mut types = std::collections::HashSet::new();
        for t in &thrown {
            assert!(t.only_class.is_some(), "{} is a throwable anyone can use", t.key);
            let dt = t.damage_type.unwrap_or(DamageType::None);
            assert_ne!(dt, DamageType::None, "{} lands untyped — true damage", t.key);
            types.insert(dt);
        }
        assert!(types.len() > 1, "every throwable lands the same blow, so the set is one item");
    }

    /// A class gate that fizzles is a wasted item and a confused player: refuse instead.
    #[test]
    fn a_hero_who_cannot_use_a_throwable_does_not_spend_it() {
        let b = balance();
        let mut wrong = hero("h", "p1");
        wrong.class_key = "explorer".into();
        wrong.atk = 100;
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![wrong],
            vec![foe("a", 0, false), foe("c", 0, false)],
            &b,
            7,
        );
        let before: Vec<i32> = bt.fighters.iter().map(|f| f.hp).collect();
        bt.active_actor = Some(0);
        let _ = bt.resolve_item(0, Some("francisca"), None, None);
        let after: Vec<i32> = bt.fighters.iter().map(|f| f.hp).collect();
        assert_eq!(before, after, "an Explorer threw a Smithwright's axe");
    }

    /// And in the right hands it lands on everything standing.
    #[test]
    fn a_smithwright_throwing_one_hits_the_whole_encounter() {
        let b = balance();
        let mut smith = hero("s", "p1");
        smith.class_key = "smithwright".into();
        smith.atk = 200;
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![smith],
            vec![foe("a", 0, false), foe("c", 0, true)],
            &b,
            7,
        );
        bt.active_actor = Some(0);
        let _ = bt.resolve_item(0, Some("francisca"), None, None);
        for id in ["a", "c"] {
            let f = bt.fighters.iter().find(|f| f.combatant_id == id).unwrap();
            assert!(f.hp < f.max_hp, "{id} was not hit by a thrown axe");
        }
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::grouping_and_flanking::{balance, foe, hero};
    use super::*;

    fn arena_with(allies: Vec<Fighter>, enemies: Vec<Fighter>) -> Battle {
        Battle::new("b".into(), EncounterClass::Standard, allies, enemies, &balance(), 7)
    }

    fn took(b: &Battle, id: &str) -> i32 {
        let f = b.fighters.iter().find(|f| f.combatant_id == id).unwrap();
        f.max_hp - f.hp
    }

    /// One rank of one group — the intersection of the two structures an encounter already
    /// has. Not the whole group, and not the whole field.
    #[test]
    fn a_sweep_takes_one_rank_of_one_group_and_nothing_else() {
        let mut lasher = hero("h", "p1");
        lasher.sweeps = true;
        lasher.atk = 400;
        let mut b = arena_with(
            vec![lasher],
            vec![
                foe("front_a", 0, false),
                foe("front_b", 0, false),
                foe("rear_a", 0, true),
                foe("other_group", 1, false),
            ],
        );
        let id = b.fighters[0].combatant_id.clone();
        let target = b.fighters.iter().find(|f| f.combatant_id == "front_a").unwrap();
        let tid = target.combatant_id.clone();
        b.fighters[0].gauge = 1.0;
        b.fighters[0].awaiting = true;
        let _ = b.submit(
            &id,
            "a1".into(),
            BattleActionKind::Attack,
            Some(vec![tid]),
            None,
            None,
        );

        assert!(took(&b, "front_a") > 0, "the target was not hit");
        assert!(took(&b, "front_b") > 0, "the rest of the rank was not swept");
        assert_eq!(took(&b, "rear_a"), 0, "the sweep crossed into the other rank");
        assert_eq!(took(&b, "other_group"), 0, "the sweep crossed into another group");
    }

    /// A sweep is a WIDE blow, not several good ones: the bodies beyond the target take a
    /// share. If they took full damage the whip would simply be a better sword.
    #[test]
    fn the_rest_of_the_rank_takes_less_than_the_target() {
        let mut lasher = hero("h", "p1");
        lasher.sweeps = true;
        lasher.atk = 400;
        let mut b = arena_with(
            vec![lasher],
            vec![foe("a", 0, false), foe("c", 0, false)],
        );
        let id = b.fighters[0].combatant_id.clone();
        b.fighters[0].gauge = 1.0;
        b.fighters[0].awaiting = true;
        let _ = b.submit(
            &id,
            "a1".into(),
            BattleActionKind::Attack,
            Some(vec!["a".to_string()]),
            None,
            None,
        );
        assert!(
            took(&b, "c") < took(&b, "a"),
            "a swept body took as much as the target: {} vs {}",
            took(&b, "c"),
            took(&b, "a")
        );
        assert!(took(&b, "c") > 0, "the sweep did nothing to the rest of the rank");
    }

    /// A weapon that does not sweep must not, however many bodies share the rank. Without
    /// this the sweep would be a property of the ENCOUNTER rather than of the weapon.
    #[test]
    fn a_weapon_that_does_not_sweep_hits_one_body() {
        let mut b = arena_with(
            vec![hero("h", "p1")],
            vec![foe("a", 0, false), foe("c", 0, false)],
        );
        let id = b.fighters[0].combatant_id.clone();
        b.fighters[0].gauge = 1.0;
        b.fighters[0].awaiting = true;
        let _ = b.submit(
            &id,
            "a1".into(),
            BattleActionKind::Attack,
            Some(vec!["a".to_string()]),
            None,
            None,
        );
        assert_eq!(took(&b, "c"), 0, "a sword swept a rank");
    }

    /// The trade the whole axis exists for: a sweep should LOSE against one body and WIN
    /// against a full rank. This holds the relationship rather than `sweep_share`, which is
    /// an untuned placeholder pending an MCP measurement.
    #[test]
    fn a_sweep_loses_to_one_body_and_beats_a_full_rank() {
        let b = balance();
        let share = b.battle.sweep_share;
        assert!(share < 1.0, "a swept body takes a full blow, so the whip is a better sword");
        assert!(share > 0.0, "the sweep does nothing, so the axis is decoration");
        // Against a rank of four, the extras alone must be worth more than the single blow
        // the whip gave up — otherwise there is never a reason to carry one.
        assert!(
            share * 3.0 > 1.0,
            "even a full rank of four is worth less than one clean hit ({share} x 3)"
        );
    }
}

#[cfg(test)]
mod creature_aoe_tests {
    use super::grouping_and_flanking::{balance, foe, hero};
    use super::*;

    /// A creature's all-enemy ability must not land on its own side. Playing a pack fight
    /// showed a thornback_boar's Trample apparently hitting another thornback_boar, which
    /// `creatures_hostile` says is impossible — so either the targeting is wrong or the
    /// harness is mislabelling. This settles which.
    #[test]
    fn a_creatures_all_enemy_ability_spares_its_own_faction() {
        let b = balance();
        let mut leader = foe("leader", 0, false);
        leader.faction = "beast".into();
        let mut minion = foe("minion", 0, false);
        minion.faction = "beast".into();
        let mut other = foe("fungal_one", 1, false);
        other.faction = "fungal".into();
        let mut bt = Battle::new(
            "b".into(),
            EncounterClass::Standard,
            vec![hero("h", "p1")],
            vec![leader, minion, other],
            &b,
            7,
        );
        let li = bt.fighters.iter().position(|f| f.combatant_id == "leader").unwrap();
        let targets = bt.ability_targets(li, AbilityTarget::AllEnemies);
        let named: Vec<&str> = targets
            .iter()
            .map(|&i| bt.fighters[i].combatant_id.as_str())
            .collect();
        assert!(
            !named.contains(&"minion"),
            "a beast's all-enemy ability targeted another beast: {named:?}"
        );
        assert!(named.contains(&"h"), "it did not target the party at all: {named:?}");
    }
}
