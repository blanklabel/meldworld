//! Typed loader for `balance/balance.toml` — every `[TUNABLE]` constant
//! (CANON.md §B; working agreement #2: no gameplay literal lives in code).
//!
//! The server loads this once at boot and threads `&Balance` into the world,
//! run, and battle systems. Changing a tunable is a one-line config edit + a
//! reboot, never a code change.

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum BalanceError {
    #[error("reading balance file {0}: {1}")]
    Io(String, std::io::Error),
    #[error("parsing balance toml: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Balance {
    pub session: Session,
    pub auth: Auth,
    pub world: World,
    pub runs: Runs,
    pub battle: Battle,
    pub loot: Loot,
    pub encounters: Encounters,
    pub gear_rarity: GearRarity,
    pub requisition: Requisition,
    pub equip_best: EquipBest,
    pub consumable: Consumable,
    pub smithwright: Smithwright,
    pub keeper: Keeper,
    pub forge: Forge,
    pub tempo: Tempo,
    pub adventure: Adventure,
    pub affix: Affix,
    pub meld: Meld,
    pub material: Material,
    pub hunt: Hunt,
    pub bounty: Bounty,
    pub harvest: Harvest,
    pub combat_math: CombatMath,
    pub world_scaling: WorldScaling,
    pub worldgen: WorldGen,
    pub ai: Ai,
    pub attributes: Attributes,
    pub creature: Creatures,
    pub player: Players,
    pub resource: Resources,
    pub perks: Perks,
    pub biome_gate: BiomeGate,
    pub region: Region,
    pub region_barrier: RegionBarrier,
    pub armor_resist: ArmorResist,
    pub affliction: Affliction,
    pub shift: Shift,
    pub world_persist: WorldPersist,
    pub building: Building,
}

/// The distance at which each biome starts appearing in a randomized run, keyed by
/// biome id. A biome is a difficulty-neutral *skin* only in the sense that distance
/// scales its creatures; the creatures themselves are not interchangeable — a tundra
/// `glacier_maw` and a forest `sporeling` are a level-1 party's death and its lunch.
/// Gating the harsher themes outward is what makes the shallow ring an on-ramp.
pub type BiomeGate = std::collections::HashMap<String, i64>;

#[derive(Debug, Clone, Deserialize)]
pub struct Session {
    pub heartbeat_interval_ms: i32,
    pub grace_window_ms: i32,
    pub auth_timeout_ms: i32,
    pub realtime_ticket_ttl_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Auth {
    pub bcrypt_cost: u32,
    pub session_token_ttl_secs: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct World {
    pub chunk_size: i32,
    pub interest_radius_chunks: i32,
    pub overworld_sim_hz: u64,
    pub snapshot_hz: u64,
    pub touch_radius_tiles: f64,
    pub interaction_radius_tiles: f64,
    pub avatar_speed_tiles_per_sec: f64,
    pub battle_reentry_grace_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Runs {
    pub base_run_level_per_distance: f64,
    pub encounter_party_scale: Vec<f64>,
    pub xp_split_across_party: bool,
    /// Slots in ONE hero's pouch. There is deliberately no Party-Inventory equivalent:
    /// the shared inventory is unbounded, so a tunable for it would be a cap nobody
    /// wants and a number the code would have to keep pretending to honour.
    pub hero_pouch_slots: i32,
    pub extraction_channel_ms: u64,

    pub fights_per_level_base: i32,
    pub fights_per_level_ramp: i32,
    pub fights_per_level_knee: i32,
    pub fights_per_level_ramp_late: i32,
    pub xp_reference_creature: f64,
    pub max_hero_level: i32,
    /// How XP falls off once a hero has out-levelled the ground it is standing on:
    /// full pay inside `xp_gap_grace` levels of the encounter, then linear down to
    /// `xp_gap_floor_mult` at `xp_gap_zero`. See `meld_run::xp_after_level_gap`.
    pub xp_gap_grace: i32,
    pub xp_gap_zero: i32,
    pub xp_gap_floor_mult: f64,
    /// The other end of the same axis: what an encounter ABOVE a hero's level pays
    /// extra. `xp_up_per_level` per level up to `xp_up_knee`, the steeper
    /// `xp_up_per_level_steep` past it, capped at `xp_up_max`. See
    /// `meld_run::xp_after_level_gap`.
    pub xp_up_per_level: f64,
    pub xp_up_knee: i32,
    pub xp_up_per_level_steep: f64,
    pub xp_up_max: f64,
    /// Town Portal item economy (extraction is mostly this item now).
    pub starting_town_portals: i32,
    pub town_portal_drop_chance: f64,
    /// Finite battle heal items each dive starts with (consumed on use in battle).
    pub starting_salves: i32,
    pub starting_elixirs: i32,
}

/// How the four attributes (Str/Mnd/Dex/Wll) map to combat stats. See the
/// `[attributes]` block in balance.toml for the meaning of each coefficient.
#[derive(Debug, Clone, Deserialize)]
pub struct Attributes {
    pub str_to_atk: f64,
    pub mnd_to_power: f64,
    pub dex_to_speed: f64,
    pub wll_to_hp: f64,
    pub wll_to_def: f64,
    pub dodge_dex_floor: i32,
    pub dodge_per_dex: f64,
    pub dodge_cap: f64,
    /// Mnd -> `ward`, the elemental/psychic counterpart of `wll_to_def`.
    pub mnd_to_ward: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Battle {
    pub tick_ms: u64,
    pub gauge_fill_divisor: f64,
    pub turn_timeout_ms: u64,
    pub flee_base: f64,
    pub flee_penalty_per_tier: f64,
    pub flee_floor: f64,
    /// Fraction of un-banked chits forfeited on a successful flee (the run
    /// continues, unlike death — see `handle_battle_end`'s `Fled` arm).
    pub flee_chit_loss_fraction: f64,
    /// Per-item probability that a non-permanent item (backpack material or
    /// red-chest looted gear) is dropped when you flee.
    pub flee_item_drop_chance: f64,
    pub merge_cap_normal_instances: i32,
    pub merge_cap_gatekeeper_instances: i32,
    pub defend_damage_reduction: f64,
    pub back_row_damage_mult: f64,
    /// Share of its own PHYSICAL damage a back-row hero deals — the other half of the trade.
    pub back_row_attack_mult: f64,
    pub sweep_share: f64,
    pub back_row_target_weight: f64,
    pub party_size_per_player: usize,
    pub skill_power_mult: f64,
    pub skill_heal_fraction: f64,
    pub item_heal_fraction: f64,
    pub crit_chance_base: f64,
    pub crit_chance_per_dex: f64,
    pub crit_chance_cap: f64,
    pub crit_mult: f64,
    pub psyker_focus_base: usize,
    pub psyker_focus_per_level: i32,
    pub psyker_focus_cap: usize,
    pub psyker_gravity_tick_mult: f64,
    pub psyker_spike_tick_mult: f64,
    pub psyker_aegis_tick_fraction: f64,
    pub psyker_anchor_gauge_drain: f64,
    pub psyker_wave_tick_mult: f64,
    pub psyker_thermal_tick_mult: f64,
    pub psyker_dissolution_tick_mult: f64,
    pub psyker_dissolution_armour_shred: i32,
    pub psyker_phase_evasion: f64,
    pub psyker_collapse_tick_mult: f64,
    pub psyker_vortex_tick_mult: f64,
    /// ATB fill multiplier while ANCHORED — the deepest slow, never a cap.
    pub psyker_anchor_slow_mult: f64,
    /// Hero level at which Mind's Eye grants its first free (turn-less) cast.
    pub psyker_minds_eye_at: i32,
    /// Levels between each additional Mind's Eye cast.
    pub psyker_minds_eye_per_level: i32,
    /// Ceiling on Mind's Eye casts at the top of a fight.
    pub psyker_minds_eye_cap: u32,
    /// Hero level from which every Psyker turn refunds one cast (Dual Manifestation).
    pub psyker_dual_manifest_at: i32,
    /// Hero level at which an offensive Focus starts reaching a second enemy.
    pub psyker_expansion_at: i32,
    /// Levels between each additional enemy Expansion reaches.
    pub psyker_expansion_per_level: i32,
    /// Ceiling on the extra enemies Expansion reaches.
    pub psyker_expansion_cap: i32,
    /// Share of the primary tick each Expansion target takes.
    pub psyker_expansion_mult: f64,
    /// Ticks an aspect's mark outlives the Psyker turn that applied it.
    pub psyker_aspect_ticks: u64,
    /// Shield: Barrier granted per ally per turn, as a share of that ally's max HP.
    pub psyker_shield_party_fraction: f64,
    /// Acceleration: gauge filled on the chosen ally each Psyker turn.
    pub psyker_accel_gauge: f64,
    /// Blackout: ticks the blinding lasts.
    pub psyker_blackout_ticks: u64,
    /// Added to the defender's dodge while a BLINDED creature is the attacker.
    pub psyker_blackout_miss: f64,
    pub psyker_vortex_ticks: u64,
    pub resonant_second_life_revive_fraction: f64,
    pub resonant_second_life_heal_fraction: f64,
    pub resonant_second_life_self_cost: f64,
    pub barrier_decay_fraction: f64,
    pub regen_decay_fraction: f64,
    pub max_effect_stacks: u8,
    pub resonant_regen_fraction: f64,
    pub resonant_transfuse_heal_fraction: f64,
    pub resonant_transfuse_cost_fraction: f64,
    pub resonant_boon_regen_fraction: f64,
    pub resonant_ward_barrier_fraction: f64,
    pub shifter_backstab_mult: f64,
    pub shifter_backstab_pierce: f64,
    pub shifter_flicker_evasion: f64,
    pub shifter_flicker_decay: f64,
    pub shifter_ransack_mult: f64,
    pub shifter_ransack_drain: f64,
    pub shifter_assassinate_mult: f64,
    pub shifter_assassinate_pierce: f64,
    pub shifter_larceny_mult: f64,
    pub shifter_larceny_drain: f64,
    pub hunter_adrenaline_max: i32,
    pub hunter_adrenaline_per_attack: i32,
    pub hunter_power_strike_cost: i32,
    pub hunter_second_wind_cost: i32,
    pub hunter_snare_cost: i32,
    pub explorer_snare_mult: f64,
    pub explorer_snare_drain: f64,
    pub hunter_frenzy_cost: i32,
    pub explorer_frenzy_mult: f64,
    pub hunter_iron_lung_heal_fraction: f64,
    pub hunter_iron_lung_regen_fraction: f64,
    pub hunter_apex_mult: f64,
    pub phoenix_guard_swell_mult: f64,
    pub phoenix_guard_swell_drain: f64,
    pub phoenix_guard_root_barrier_fraction: f64,
    pub phoenix_guard_shock_mult: f64,
    pub phoenix_guard_toll_mult: f64,
    /// The Order of the Iron Hull (`resolve_iron_hull`). Priced below the Phoenix Guard on
    /// raw damage and paid back in tempo — a stagger, a drain, a Barrier — because an
    /// order whose art is returning momentum must not also win the damage comparison.
    /// The D&D monk's Unarmored Defence: `def` per level, because the order wears no
    /// armour and its discipline is what hardens.
    pub iron_hull_unarmored_def_per_level: f64,
    pub iron_hull_oar_mult: f64,
    pub iron_hull_sea_legs_evasion: f64,
    pub iron_hull_swell_mult: f64,
    pub iron_hull_swell_drain: f64,
    pub iron_hull_rooting_barrier_fraction: f64,
    pub iron_hull_shock_mult: f64,
    pub iron_hull_resonance_barrier_fraction: f64,
    /// The acoustic half. Both land as `Ethereal`, so a back rank is no protection — a
    /// melee order with an answer to the rear is the trade for having no armour.
    pub iron_hull_wake_mult: f64,
    pub iron_hull_wake_drain: f64,
    pub iron_hull_toll_mult: f64,
    pub iron_hull_toll_barrier_fraction: f64,
    /// The Wall Defense Force's Rift Drop-Trooper (`resolve_rift_knight`). The highest
    /// single-target numbers in the game, each paid for by a turn the party fights one
    /// hero short — tune `dive_untargetable_ticks` before touching a multiplier.
    pub rift_knight_blink_mult: f64,
    pub rift_knight_recall_atk_fraction: f64,
    pub rift_knight_dive_mult: f64,
    pub rift_knight_dive_untargetable_ticks: u64,
    pub rift_knight_payload_mult: f64,
    pub rift_knight_breach_mult: f64,
    pub rift_knight_breach_splash_mult: f64,
    pub rift_knight_gate_barrier_fraction: f64,
    pub rift_knight_cataclysm_mult: f64,
    pub rift_knight_cataclysm_drain: f64,
    pub phoenix_guard_undead_mult: f64,
    /// How many of its OWN turns a fighter must take before its gauge can be knocked again.
    pub gauge_guard_turns: u8,
    /// What a knocked-down fighter takes from everything until it is back up.
    pub staggered_damage_mult: f64,
    pub phoenix_guard_vigil_barrier_fraction: f64,
    pub phoenix_guard_eradication_mult: f64,
    pub phoenix_guard_eradication_missing_bonus: f64,
    pub phoenix_guard_hallowed_mult: f64,
    pub phoenix_guard_ascendant_mult: f64,
    pub phoenix_guard_ascendant_barrier_fraction: f64,
    pub explorer_trailblaze_mult: f64,
    pub explorer_mark_damage_mult: f64,
    pub explorer_mark_ticks: u64,
    pub explorer_field_dressing_fraction: f64,
    pub explorer_read_ground_mult: f64,
    pub explorer_read_ground_drain: f64,
    pub explorer_misdirection_miss: f64,
    pub explorer_misdirection_flee_bonus: f64,
    pub explorer_misdirection_ticks: u64,
    pub explorer_stable_ground_fraction: f64,
    pub explorer_safe_passage_evasion: f64,
    pub explorer_haste_mult: f64,
    pub explorer_haste_ticks: u64,
    pub explorer_now_ticks: u64,
    pub explorer_world_entire_mark_ticks: u64,
    pub explorer_world_entire_haste_ticks: u64,
    pub resonant_mend_all_fraction: f64,
    pub resonant_mend_all_self_cost: f64,
    pub resonant_sanctuary_regen_fraction: f64,
    pub resonant_revitalize_fraction: f64,
    pub resonant_revitalize_self_cost: f64,
    pub resonant_lifewell_fraction: f64,
    pub resonant_lifewell_regen_fraction: f64,
    pub resonant_lifewell_self_cost: f64,
    pub resonant_bloodbond_fraction: f64,
    pub resonant_bloodbond_regen_fraction: f64,
    pub resonant_bloodbond_barrier_fraction: f64,
    pub resonant_bloodbond_self_cost: f64,
    pub resonant_martyr_fraction: f64,
    pub resonant_martyr_self_cost: f64,
    pub resonant_bloom_fraction: f64,
    pub resonant_bloom_barrier_fraction: f64,
    pub resonant_bloom_self_cost: f64,
    pub shifter_steal_drain: f64,
    pub shifter_steal_chits_per_tier: i64,
    pub shifter_steal_material_chance: f64,
    pub shifter_mug_mult: f64,
    pub shifter_mug_drain: f64,
    pub hunter_crushing_blow_mult: f64,
    pub pin_the_prey_mult: f64,
    pub pin_the_prey_drain: f64,
    // Monster-ability system (Creature AI spec §2).
    /// ATB fill multiplier while a slowing status (web/chill/bind/…) is active.
    pub status_slow_mult: f64,
    /// Poison DoT per victim turn, as a fraction of max HP.
    pub poison_dot_fraction: f64,
    /// Burn DoT per victim turn, as a fraction of max HP.
    pub burn_dot_fraction: f64,
    /// Weight of the implicit basic attack mixed into every ability roll.
    pub basic_attack_weight: i32,
    /// Fraction of a victim's carried chits a `steal chits` effect takes.
    pub steal_chits_fraction: f64,
    pub resonant_revitalize_revive_fraction: f64,
    pub keeper_terras_gift_revive_fraction: f64,
}

/// Creature loot tunables (economy.md sources S1). See the `[loot]` block in
/// balance.toml. Chits + biome material + red-chest gear on a felled encounter.
#[derive(Debug, Clone, Deserialize)]
pub struct Loot {
    pub chits_per_mlevel: f64,
    pub chits_jitter: f64,
    pub material_per_creature: f64,
    pub material_qty_per_tier: f64,
    pub gear_drop_chance: f64,
    /// Shallowest distance a gear drop is possible at all (GR-8); below it, none.
    pub gear_ramp_start_distance: i64,
    /// Fraction of `gear_drop_chance` in force at `gear_ramp_start_distance`, ramping
    /// to all of it at `WorldScaling::red_chest_floor_distance`.
    pub gear_ramp_start_mult: f64,
    /// P(a felled encounter also drops a band-appropriate potion) (GR-8).
    pub potion_drop_chance: f64,
    /// Fraction of a reward-spike encounter's gear drops that are PERMANENT (blue).
    pub ephemeral_gear_chance: f64,
    pub ephemeral_power_mult: f64,
    pub permanent_gear_chance: f64,
    pub insured_power_mult: f64,
    /// POINTS of max durability an INSURED piece loses per HERO DEATH (GR-2, CANON
    /// D6), charged on the fallen hero's own kit. A wipe is not a separate case; it
    /// is four heroes falling. Flat points, so a piece's own durability decides how
    /// many deaths it survives — which is what makes one drop better MADE than
    /// another rather than merely better rolled.
    pub durability_loss_per_fall: i32,
    pub gear_atk_per_tier: f64,
    pub gear_atk_jitter: f64,
    /// Mean max durability of a rolled piece; every drop jitters around it.
    pub gear_base_durability: i32,
    /// ± fraction on each piece's durability roll, so no two drops wear out together.
    pub gear_durability_jitter: f64,
}

/// Encounter-variety tunables (FS-4): Elite champions + Gatekeeper bosses. See the
/// `[encounters]` block in balance.toml.
#[derive(Debug, Clone, Deserialize)]
pub struct Encounters {
    pub elite_chance: f64,
    pub elite_hp_mult: f64,
    pub elite_atk_mult: f64,
    pub elite_xp_mult: f64,
    pub elite_loot_mult: f64,
    /// A wall of HP **per party** — multiplied by the encounter's declared party count
    /// (`meld_proto::warbands`), so the number means the same thing whoever shows up.
    pub gatekeeper_hp_mult: f64,
    pub gatekeeper_raid_chance: f64,
    pub gatekeeper_raid_max_parties: u8,
    /// How much oftener a raid boss's PARTY-WIDE abilities are rolled, per party past the
    /// first. Cadence rather than magnitude: a wide blow is the only part of a boss's output
    /// that does not dilute as the crowd grows, so it is the only part a raid tier may raise
    /// without one-shotting whoever arrives before the merge fills. Measured: the wide share
    /// climbs 12.5% -> 41.8% across the four rungs, taking a full raid from 52% of the
    /// one-party per-hero baseline to ~132% of it.
    pub raid_wide_weight_per_party: f64,
    /// How much sooner those same abilities come back, per party past the first — the
    /// cooldown is divided by `1 + this x (parties - 1)`, floored at the telegraph. It
    /// saturates early in practice: past the point where a wide row is ready whenever the boss
    /// acts, only the weight above moves the share.
    pub raid_wide_cooldown_per_party: f64,
    pub gatekeeper_atk_mult: f64,
    pub gatekeeper_xp_mult: f64,
    pub gatekeeper_loot_mult: f64,
    pub undead_rite_loot_mult: f64,
    pub pack_spread: f64,
    /// How far a pack member roams from its pack's anchor (`CR-11`). Must stay under half
    /// of `[ai] group_radius`, or a pack wanders out of being one encounter.
    pub pack_leash: f64,
    /// `CR-11` THE CALL: a leader under this share of max HP (or with no minions left)
    /// spends its turn calling, and the pack answers from `pack_call_radius` away, up to
    /// `pack_call_max` bodies.
    pub pack_call_hp_fraction: f64,
    pub pack_call_radius: f64,
    pub pack_call_max: usize,
    pub undead_rite_chance: f64,
    /// THE END FIGHT (EW): distance past which one encounter becomes three named bosses.
    pub end_fight_min_distance: f64,
    pub end_fight_bosses: usize,
    /// AUTHORED per-boss stats — see the balance comment for why this is not a multiplier.
    pub end_fight_boss_hp: i32,
    pub end_fight_boss_atk: i32,
    pub end_fight_boss_xp: i64,
    /// The reward spike the end fight rolls its drops at — above a Gatekeeper's, so it is
    /// the best unique/set source in the game rather than only a guaranteed floor.
    pub end_fight_loot_mult: f64,
    /// Damage of a warded family that actually lands on an end-fight boss.
    pub end_fight_ward_mult: f64,
    /// Floor on how far a gauge slow may drag an end-fight boss's fill rate.
    pub end_fight_slow_floor: f64,
    pub end_fight_reward_pieces: i32,
    pub end_fight_reward_tier: i32,
    pub undead_rite_min_tier: i32,
    /// Distance below which a standard creature is never promoted to Elite. Without
    /// it an Elite — a named boss with `elite_hp_mult` behind it — can be the second
    /// creature a level-1 party meets, which is a wipe rather than an encounter.
    pub elite_min_distance: i64,
    /// Distance below which a peak never mounts a Gatekeeper. Deeper than the Elite
    /// gate because a Gatekeeper carries `gatekeeper_hp_mult` (10x) — it was gated
    /// only on `hub_safe_radius`, so one could stand 14 units from the hub.
    pub gatekeeper_min_distance: i64,
    pub undead_rite_minions: usize,
    pub undead_rite_boss_hp_mult: f64,
    pub undead_rite_boss_atk_mult: f64,
    pub undead_rite_boss_xp_mult: f64,
    pub undead_rite_minion_hp_mult: f64,
    pub undead_rite_minion_atk_mult: f64,
    pub pack_aura_atk_mult: f64,
    pub pack_guard_per_minion: f64,
    pub pack_guard_cap: f64,
    pub pack_rout_atk_mult: f64,
    pub pack_rout_flees: bool,
    pub leader_hp_mult: f64,
    pub leader_atk_mult: f64,
    pub leader_xp_mult: f64,
    pub minion_hp_mult: f64,
    pub minion_atk_mult: f64,
    pub minion_xp_mult: f64,
    /// The encounter-size ramp, in distance order (see `group_band_at`).
    #[serde(default)]
    pub group_ramp: Vec<GroupBand>,
}

/// One rung of the encounter-size ramp: from `from_distance` outward, a spawn has
/// `chance` of forming a group of `size` creatures (leader + `size - 1` minions).
#[derive(Debug, Clone, Deserialize)]
pub struct GroupBand {
    pub from_distance: f64,
    pub size: usize,
    pub chance: f64,
    pub mixed_chance: f64,
}

impl Encounters {
    /// The ramp rung in force at `distance` — the LAST band whose `from_distance`
    /// the spawn has passed. `None` inside the first band, where fights are duels
    /// while a player is still learning the ATB.
    pub fn group_band_at(&self, distance: f64) -> Option<&GroupBand> {
        self.group_ramp
            .iter()
            .filter(|b| distance >= b.from_distance)
            .max_by(|a, b| a.from_distance.total_cmp(&b.from_distance))
    }

    /// How many creatures one encounter at `distance` is worth ON AVERAGE — a
    /// leader plus however many minions the band's `chance` actually produces.
    ///
    /// This is what the XP ladder has to be priced against. The first ~150 tiles are
    /// duels, so pricing a level there as a two-creature pack makes the opening of the
    /// game take twice the fights the design asks for.
    pub fn expected_group_size(&self, distance: f64) -> f64 {
        match self.group_band_at(distance) {
            Some(b) => 1.0 + (b.size.max(1) - 1) as f64 * b.chance.clamp(0.0, 1.0),
            None => 1.0,
        }
    }
}

/// Forge knobs (MS-1): crafting gear, rerolling affixes, repairing durability.
#[derive(Debug, Clone, Deserialize)]
pub struct Forge {
    pub gear_material_cost: i32,
    pub gear_chit_cost: i64,
    pub gear_tier_per_forging_level: f64,
    pub gear_variance: f64,
    pub gear_variance_floor: f64,
    pub gear_variance_per_level: f64,
    pub reroll_material_cost: i32,
    pub reroll_material_per_tier: i32,
    pub reroll_chit_cost: i64,
    pub reroll_min_forging_level: i32,
    pub repair_chit_cost_per_point: i64,
    pub repair_points_per_forging_level: i32,
    pub forge_xp_per_craft: i64,
    pub catalyst_material_cost: i32,
    pub catalyst_tier_bonus: i32,
    pub station_min_forging_level: i32,
    pub station_min_alchemy_level: i32,
    pub station_ore_cost: i32,
    pub station_uses: i32,
    pub station_radius: f64,
    pub station_setup_ms: u64,
    pub station_teardown_ms: u64,
    pub station_teardown_refund: i32,
    pub enhance_material_cost: i32,
    pub enhance_chit_cost: i64,
    pub enhance_bonus_base: i32,
    pub enhance_bonus_per_quality: i32,
    pub enhance_min_forging_level: i32,
    pub alembic_field_radius: f64,
    pub alembic_regen_per_sec: f32,
    pub tonic_material_cost: i32,
    pub tonic_chit_cost: i64,
    pub tonic_atk: i32,
    pub tonic_def: i32,
    pub tonic_regen: i32,
    pub tonic_per_quality: i32,
}

impl Forge {
    /// One line of a tonic, at this cook's quality: the base plus a share of
    /// `tonic_per_quality`, so a Keeper who can time a pot hands out a better draught.
    pub fn tonic_amount(&self, base: i32, quality: f64) -> i32 {
        base + (self.tonic_per_quality.max(0) as f64 * quality.clamp(0.0, 1.0)).floor() as i32
    }
}

impl Forge {
    /// The highest tier a smith of this Forging level can forge at.
    pub fn forgeable_tier(&self, forging_level: i32) -> i32 {
        ((forging_level.max(1) as f64) * self.gear_tier_per_forging_level).floor() as i32
    }

    /// The tier a smith reaches when they quench the piece in a **trophy** — a
    /// monster part is how a forge reaches past the smith's own level, which is
    /// what makes a combat drop worth carrying home to the Forge.
    pub fn catalyzed_tier(&self, forging_level: i32) -> i32 {
        self.forgeable_tier(forging_level) + self.catalyst_tier_bonus
    }

    /// How wide a forged stat's roll is at this Forging level — a master smith is
    /// consistent, an apprentice is not.
    pub fn variance_at(&self, forging_level: i32) -> f64 {
        (self.gear_variance - self.gear_variance_per_level * (forging_level.max(1) - 1) as f64)
            .max(self.gear_variance_floor)
    }

    /// The materials one reroll eats on a piece of this tier. Re-drawing a deep
    /// item's affixes is a bigger job than a starter blade's, so the cost climbs
    /// with the piece rather than sitting flat at every depth.
    pub fn reroll_materials(&self, tier: i32) -> i32 {
        (self.reroll_material_cost + self.reroll_material_per_tier * tier.max(0)).max(1)
    }

    /// How many points of max durability one repair restores.
    pub fn repair_points(&self, forging_level: i32) -> i32 {
        self.repair_points_per_forging_level * forging_level.max(1)
    }
}

/// The smithing tempo game (MS-1): how hard the bar is to hit, and what hitting it
/// buys. Difficulty rides the piece being worked, so depth is the difficulty axis here
/// exactly as it is everywhere else.
#[derive(Debug, Clone, Deserialize)]
pub struct Tempo {
    pub strikes_base: i32,
    pub strikes_per_tier: f64,
    pub strikes_max: i32,
    pub band_width_base: f64,
    pub band_width_per_tier: f64,
    pub band_width_min: f64,
    pub sweep_ms_base: i64,
    pub sweep_ms_per_tier: i64,
    pub sweep_ms_min: i64,
    pub band_width_per_skill_level: f64,
    pub band_width_per_extra_hand: f64,
    pub sweep_ms_per_skill_level: i64,
    pub sweep_ms_per_extra_hand: i64,
    pub extra_hands_max: i32,
    pub grace_ms: i64,
    pub cook_bonus_doses: i32,
    pub quality_epic: f64,
    pub quality_rare: f64,
    pub repair_quality_floor: f64,
}

impl Tempo {
    /// Blows this piece takes.
    pub fn strikes(&self, tier: i32) -> i32 {
        let t = tier.max(0) as f64;
        ((self.strikes_base as f64 + self.strikes_per_tier * t).floor() as i32)
            .clamp(1, self.strikes_max.max(1))
    }

    /// How wide the hot band is, as a fraction of the bar: the PIECE narrows it, the
    /// smith and their crew widen it back. That subtraction is the whole difficulty
    /// curve — a deep piece is only hard for a smith who cannot yet work it.
    pub fn band_width(&self, tier: i32, skill_level: i32, extra_hands: i32) -> f64 {
        let eased = self.band_width_per_skill_level * (skill_level.max(1) - 1) as f64
            + self.band_width_per_extra_hand * self.crew(extra_hands) as f64;
        (self.band_width_base - self.band_width_per_tier * tier.max(0) as f64 + eased)
            .max(self.band_width_min)
            .clamp(0.01, 1.0)
    }

    /// One full pass of the marker, in milliseconds. Same shape: deeper is faster,
    /// a better smith with more help gets time back.
    pub fn sweep_ms(&self, tier: i32, skill_level: i32, extra_hands: i32) -> i64 {
        let eased = self.sweep_ms_per_skill_level * (skill_level.max(1) - 1) as i64
            + self.sweep_ms_per_extra_hand * self.crew(extra_hands) as i64;
        (self.sweep_ms_base - self.sweep_ms_per_tier * tier.max(0) as i64 + eased)
            .max(self.sweep_ms_min)
    }

    /// Extra smiths that actually help. Past a full party of them there is nothing left
    /// to hold, so the help caps.
    fn crew(&self, extra_hands: i32) -> i32 {
        extra_hands.clamp(0, self.extra_hands_max.max(0))
    }

    /// The affix pool a heat of this quality earns. A flawless heat reaches the epic
    /// pool — the same reach a trophy catalyst buys, but paid for in skill.
    pub fn rarity_for(&self, quality: f64) -> &'static str {
        if quality >= self.quality_epic {
            "epic"
        } else if quality >= self.quality_rare {
            "rare"
        } else {
            "common"
        }
    }

    /// Extra doses a cook of this quality yields on a brew — the Keeper's side of the
    /// same idea: a good cook feeds more people from the same reagents.
    pub fn bonus_doses(&self, quality: f64) -> i32 {
        (self.cook_bonus_doses.max(0) as f64 * quality.clamp(0.0, 1.0)).floor() as i32
    }

    /// The fraction of a repair a heat of this quality gives back.
    pub fn repair_fraction(&self, quality: f64) -> f64 {
        let floor = self.repair_quality_floor.clamp(0.0, 1.0);
        floor + (1.0 - floor) * quality.clamp(0.0, 1.0)
    }
}

/// The Foundry Smithwright's kit (MS-1): tempo and shielding rather than damage.
#[derive(Debug, Clone, Deserialize)]
pub struct Smithwright {
    pub hammer_mult: f64,
    pub hammer_gauge_drain: f64,
    pub quench_barrier_fraction: f64,
    pub bulwark_barrier_fraction: f64,
    pub temper_atk_fraction: f64,
    pub slag_mult: f64,
    pub forge_heal_fraction: f64,
    pub forge_barrier_fraction: f64,
    pub chorus_atk_fraction: f64,
    pub great_work_heal_fraction: f64,
    pub great_work_barrier_fraction: f64,
    pub great_work_atk_fraction: f64,
}

/// The Open Flower Keeper's kit (MS-1): everything here keeps someone standing.
#[derive(Debug, Clone, Deserialize)]
pub struct Keeper {
    pub thornlash_mult: f64,
    pub thornlash_gauge_drain: f64,
    pub poultice_heal_fraction: f64,
    pub poultice_regen_fraction: f64,
    pub bloomfield_regen_fraction: f64,
    pub root_snare_mult: f64,
    pub root_snare_gauge_drain: f64,
    pub draught_barrier_fraction: f64,
    pub draught_regen_fraction: f64,
    pub gift_heal_fraction: f64,
    pub gift_barrier_fraction: f64,
    pub gift_gauge: f64,
    pub thorn_grove_mult: f64,
    pub thorn_grove_gauge_drain: f64,
    pub world_tree_heal_fraction: f64,
    pub world_tree_barrier_fraction: f64,
    pub world_tree_regen_fraction: f64,
}

/// Per-class `[atk, def, spd]` weights for "equip best" (GR-5). Keyed by class key so a new
/// class is a row here rather than a code change.
pub type EquipBest = std::collections::HashMap<String, [f64; 3]>;

/// Potion magnitudes + Apothecary prices (GR-4 / EC-2).
#[derive(Debug, Clone, Deserialize)]
pub struct Affliction {
    pub venom_hp_per_step: i32,
    pub venom_steps_per_tick: i32,
    pub bindings_move_mult: f64,
    pub paralysis_break_base: f64,
    pub paralysis_break_per_wll: f64,
    pub paralysis_break_cap: f64,
}

/// The Shifting Lands (CANON D20 / §W2). Cadence, region size and Force damage are
/// the game's translation of the tabletop tables; the *structure* — that the schedule
/// **THE REGION DECOMPOSITION** — the size and shape of one cell of the world
/// ([`meld_proto::regions`]). The decomposition's STRUCTURE is code; these are the
/// coefficients, so a world can be made of provinces or of parishes without touching it.
#[derive(Debug, Clone, Deserialize)]
pub struct Region {
    pub ring_step: f64,
    pub cell_width: f64,
    pub boundary_warp: f64,
    pub blend_width: f64,
}

/// **WG-11: the cell graph is the maze.** How porous each biome's region boundaries are —
/// the share that stay walkable — plus how a closed one is drawn. Porosity is what makes a
/// biome maze in its OWN way rather than merely differ by density: field and desert are the
/// open crossings between mazes, ashfall is where you hunt for a pass.
#[derive(Debug, Clone, Deserialize)]
pub struct RegionBarrier {
    pub erase_field: f64,
    pub erase_desert: f64,
    pub erase_forest: f64,
    pub erase_amber_wood: f64,
    pub erase_tundra: f64,
    pub erase_mire: f64,
    pub erase_ashfall: f64,
    pub erase_default: f64,
    pub dead_end_chest_tier_bonus: i32,
    pub band_half_width: f64,
    pub prop_spacing: f64,
}

/// is a pure function of `(world_seed, shift_generation)` driven by the tick counter —
/// is code, and lives in [`meld_world::shift`].
#[derive(Debug, Clone, Deserialize)]
pub struct Shift {
    pub cadence_ticks: u64,
    pub cadence_jitter: f64,
    pub warning_ticks: u64,
    pub min_sections: usize,
    pub max_sections: usize,
    pub damage_fraction_min: f64,
    pub damage_fraction_max: f64,
    pub safe_radius: f64,
    pub random_pick_share: f64,
}

/// Magnitudes for the one `Structure` primitive (CANON D21/§W3). The *functions* live in
/// [`meld_proto::structures`]; only the numbers are here.
#[derive(Debug, Clone, Deserialize)]
pub struct Building {
    pub anchor_stone_cost: i32,
    pub wall_wood_cost: i32,
    pub anchor_max_hp: i32,
    pub wall_max_hp: i32,
    pub anchor_build_ms: u64,
    pub wall_build_ms: u64,
    pub build_start_fraction: f64,
    pub road_speed_mult: f64,
    pub anchor_pin_radius: f64,
    pub repair_hp_per_material: i32,
    pub demolish_refund_fraction: f64,
    pub min_spacing: f64,
    pub abut_spacing: f64,
    pub build_reach: f64,
    pub enclosure_escape_radius: f64,
    pub enclosure_cell: f64,
    pub enclosure_cell_budget: usize,
    pub no_build_near_player: f64,
    pub max_per_player: usize,
    pub stuck_check_ticks: u64,
    pub shift_hold_damage_fraction: f64,
}

impl Building {
    /// Ore cost, max HP and build time for a function key — one place, so a new function
    /// cannot be half-priced by being added to only some of the three.
    pub fn spec(&self, key: &str) -> Option<(i32, i32, u64)> {
        match key {
            "anchor" => Some((self.anchor_stone_cost, self.anchor_max_hp, self.anchor_build_ms)),
            "wall" => Some((self.wall_wood_cost, self.wall_max_hp, self.wall_build_ms)),
            _ => None,
        }
    }
}

/// A world is a place, not a lobby (CANON §W1/§W5): it outlives its divers, and its
/// delta from the seed baseline is written to Postgres so it outlives the process.
#[derive(Debug, Clone, Deserialize)]
pub struct WorldPersist {
    pub enabled: bool,
    pub save_every_ticks: u64,
    pub creature_regrow_ticks: u64,
    pub node_regrow_ticks: u64,
    pub chest_regrow_ticks: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArmorResist {
    pub step: f64,
    pub creature_ward_fraction: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Consumable {
    pub thrown_atk_mult: f64,
    pub barrier_amount: i32,
    pub regen_amount: i32,
    pub evasion_pct: i32,
    pub adrenaline_amount: i32,
    pub revive_hp_fraction: f64,
    pub insight_mote_xp: i64,
    pub world_xp_item_chance: f64,
    pub world_revive_item_chance: f64,
    pub price_bloom_salve: i64,
    pub price_bulwark_tonic: i64,
    pub price_mending_draught: i64,
    pub price_town_portal: i64,
    pub price_markup_per_tier: f64,
    pub potency_per_step: f64,
}

impl Consumable {
    /// The multiplier on a potion's magnitude at `potency` steps up its own
    /// effect's ladder (`ConsumableDef::potency`). Step 0 is exactly the standard
    /// dose, so adding the trophy line above the reagent line cannot move a
    /// number a player already knows.
    pub fn potency_mult(&self, potency: i32) -> f64 {
        self.potency_per_step.powi(potency.max(0))
    }

    /// Shelf price of one unit, in chits. `None` for anything the Apothecary does
    /// not stock — a shop that sells everything is not a shop.
    pub fn price(&self, item_kind: &str) -> Option<i64> {
        Some(match item_kind {
            "bloom_salve" => self.price_bloom_salve,
            "bulwark_tonic" => self.price_bulwark_tonic,
            "mending_draught" => self.price_mending_draught,
            "town_portal" => self.price_town_portal,
            _ => return None,
        })
    }
}

/// Adventure-depth knobs (AD-2): the combo window and class-pair synergy
/// magnitudes. Which combos/synergies exist is content in `meld_proto::synergies`.
#[derive(Debug, Clone, Deserialize)]
pub struct Adventure {
    pub combo_window_ticks: u64,
    pub synergy_party_barrier_fraction: f64,
    pub synergy_party_regen_fraction: f64,
    pub synergy_back_row_evasion: i32,
}

/// Affix knobs (AD-1). Which affixes exist is content
/// (`meld_proto::affixes::AFFIXES`); these are the numbers here.
#[derive(Debug, Clone, Deserialize)]
pub struct Affix {
    pub count_common: usize,
    pub count_rare: usize,
    pub count_epic: usize,
    pub count_legendary: usize,
    pub count_signature_bonus: usize,
    pub count_ephemeral_bonus: usize,
    pub magnitude_per_tier: f64,
    pub magnitude_jitter: f64,
    /// Relative DRAW weight per affix class — how often a line of that kind is the one
    /// you get, not how big it is (`AffixDef::scale` is the size). A flat pool made
    /// `brand` — which decides what damage type your attacks ARE — exactly as common as
    /// `masterwork`, extra durability: measured at 32-34% each on a deep legendary, every
    /// key identical. With nothing rarer than anything else there is nothing to chase, so a
    /// wide roll was just more random lines. Filler sits at 1.0 and the build-defining
    /// classes below it. [TUNABLE]
    pub weight_stat: f64,
    pub weight_quality: f64,
    pub weight_element: f64,
    pub weight_ward: f64,
    pub weight_keyword: f64,
    pub weight_synergy: f64,
    pub tier_floor_stat: i32,
    pub tier_floor_element: i32,
    pub tier_floor_ward: i32,
    pub tier_floor_keyword: i32,
    pub tier_floor_synergy: i32,
    pub tier_floor_quality: i32,
    /// Percent of extra max durability an "of Masterwork" piece carries.
    pub masterwork_durability_pct: i32,
    pub resist_pct_per_tier: i32,
    pub resist_pct_cap: i32,
    pub unique_chance: f64,
    pub unique_min_tier: i32,
    pub unique_requires_spike: bool,
    pub set_chance: f64,
    pub set_min_tier: i32,
}

impl Affix {
    /// How many affixes a drop of this rarity rolls.
    pub fn count_for(&self, rarity: &str, signature: bool, ephemeral: bool) -> usize {
        let base = match rarity {
            "legendary" => self.count_legendary,
            "epic" => self.count_epic,
            "rare" => self.count_rare,
            _ => self.count_common,
        };
        base
            + if signature { self.count_signature_bonus } else { 0 }
            + if ephemeral { self.count_ephemeral_bonus } else { 0 }
    }

    /// The relative draw weight for an affix class, keyed by its wire word (same reason
    /// `tier_floor` is: this crate stays a pure config loader with no proto dependency).
    /// An unknown word reads as filler weight rather than 0, so a new affix class is
    /// merely un-tuned instead of unreachable.
    pub fn weight(&self, class: &str) -> f64 {
        match class {
            "element" => self.weight_element,
            "ward" => self.weight_ward,
            "keyword" => self.weight_keyword,
            "synergy" => self.weight_synergy,
            "quality" => self.weight_quality,
            _ => self.weight_stat,
        }
    }

    /// The tier a given affix class unlocks at, keyed by its wire word — so this
    /// crate stays a pure config loader with no proto dependency.
    pub fn tier_floor(&self, class: &str) -> i32 {
        match class {
            "element" => self.tier_floor_element,
            "ward" => self.tier_floor_ward,
            "keyword" => self.tier_floor_keyword,
            "synergy" => self.tier_floor_synergy,
            "quality" => self.tier_floor_quality,
            _ => self.tier_floor_stat,
        }
    }
}

/// Gear-rarity tunables (loot excitement). See the `[gear_rarity]` block.
/// The Requisition counter's price list (EC-2). Flat per slot category: a shop piece
/// has no roll, so it can have a number rather than an estimate.
#[derive(Debug, Clone, Deserialize)]
pub struct Requisition {
    pub weapon_price: i64,
    pub armor_price: i64,
    pub accessory_price: i64,
}

impl Requisition {
    /// What the counter charges for `slot`, or `None` if it does not stock that slot.
    pub fn price(&self, slot: &str) -> Option<i64> {
        Some(match slot {
            "main_hand" | "off_hand" => self.weapon_price,
            "head" | "chest" | "legs" => self.armor_price,
            "accessory" => self.accessory_price,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GearRarity {
    pub rare_weight: f64,
    pub epic_weight: f64,
    pub legendary_weight: f64,
    /// Distance shift (spec §4): every 2 tiers, the non-common weights grow by
    /// this fraction (and Common, the remainder, shrinks to match).
    pub rarity_shift_per_2_tiers: f64,
    pub rare_mult: f64,
    pub epic_mult: f64,
    pub legendary_mult: f64,
    /// Chance a gear drop is its class's one signature item for that slot
    /// instead of the normal tiered catalog name (independent of rarity —
    /// a signature item can still separately roll Legendary).
    pub class_signature_chance: f64,
    /// Signature items can't appear before this tier (keeps them from
    /// showing up on a shallow kill).
    pub class_signature_min_tier: i32,
    /// Stat-bonus multiplier for a signature item, stacking with rarity.
    pub class_signature_mult: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meld {
    pub xp_per_level: i64,
    pub alchemy_xp_per_extracted_stack: i64,
    pub forging_xp_per_craft: i64,
    pub mercantile_xp_per_sale: i64,
}

impl Meld {
    /// A Meld skill's level from its banked XP.
    pub fn skill_level(&self, xp: i64) -> i32 {
        meld_skill_level(xp, self.xp_per_level)
    }
}

/// A Meld skill's level from its banked XP. One definition, because both the HTTP
/// crafting gates and the world's field-station gates read it — two copies of this
/// would be two different games.
pub fn meld_skill_level(xp: i64, xp_per_level: i64) -> i32 {
    (1 + xp / xp_per_level.max(1)).clamp(1, 99) as i32
}

/// What the Broker pays for a material (MS-1 / EC-2). Deliberately a **floor
/// price**, not an income: selling a material must always be worth less than
/// crafting with it, so the Broker is the answer to "I will never use this" rather
/// than the optimal play. See docs/behaviors/economy.md (source S3).
#[derive(Debug, Clone, Deserialize)]
pub struct Material {
    pub sale_base_chits: i64,
    pub sale_growth_per_tier: f64,
    pub sale_trophy_mult: f64,
    pub sale_refined_mult: f64,
    pub sale_haggle_pct_per_level: f64,
    pub sale_haggle_max_pct: f64,
}

impl Material {
    /// Unit price in chits for a material of `tier` and `class` (a
    /// `meld_proto::materials::MaterialClass` wire word), at this Mercantile level.
    /// What it cost to get is what it fetches: a trophy costs a fight rather than a
    /// walk, and refined stock has a Smelter's labour in it.
    pub fn sale_price(&self, tier: i32, class: &str, mercantile_level: i32) -> i64 {
        let band = self.sale_growth_per_tier.powi(tier.max(0));
        let class = match class {
            "trophy" => self.sale_trophy_mult,
            "refined" => self.sale_refined_mult,
            _ => 1.0,
        };
        let haggle = (self.sale_haggle_pct_per_level * (mercantile_level.max(1) - 1) as f64)
            .min(self.sale_haggle_max_pct)
            / 100.0;
        (((self.sale_base_chits as f64) * band * class * (1.0 + haggle)).round() as i64).max(1)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hunt {
    pub reward_chits_base: i64,
    pub reward_chits_growth_per_tier: f64,
    pub reward_material_qty: i32,
    pub reward_material_qty_per_tier: i32,
    pub quarry_sense_radius: f64,
    pub quarry_sense_hunter_radius: f64,
}

impl Hunt {
    /// Chits the board pays for completing a hunt of `tier`.
    pub fn reward_chits(&self, tier: i32) -> i64 {
        let band = self.reward_chits_growth_per_tier.powi(tier.max(0));
        (((self.reward_chits_base as f64) * band).round() as i64).max(1)
    }

    /// Size of the material stack handed over with the chits.
    pub fn reward_qty(&self, tier: i32) -> i32 {
        (self.reward_material_qty + self.reward_material_qty_per_tier * tier.max(0)).max(1)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Bounty {
    pub active_slots: usize,
    pub window_hours: i64,
    pub rank_xp_base: i64,
    pub rank_xp_per_rank: i64,
    pub sighting_base_distance: i32,
    pub sighting_per_rank: i32,
    pub sighting_jitter: f64,
    pub power_base: f64,
    pub power_per_rank: f64,
    pub dungeon_chance: f64,
    pub reward_chits_base: i64,
    pub reward_chits_per_rank: i64,
    pub reward_material_qty: i32,
    pub reward_material_qty_per_rank: i32,
    pub reward_gear_from_rank: i32,
}

impl Bounty {
    /// Where a mark is sighted for this rank, before jitter.
    pub fn sighting(&self, rank: i32) -> i32 {
        self.sighting_base_distance + self.sighting_per_rank * rank.max(0)
    }

    /// How much harder than a standard creature at that depth the mark is.
    pub fn power(&self, rank: i32) -> f64 {
        self.power_base + self.power_per_rank * rank.max(0) as f64
    }

    pub fn reward_chits(&self, rank: i32) -> i64 {
        self.reward_chits_base + self.reward_chits_per_rank * rank.max(0) as i64
    }

    pub fn reward_qty(&self, rank: i32) -> i32 {
        (self.reward_material_qty + self.reward_material_qty_per_rank * rank.max(0)).max(1)
    }

    pub fn rank_xp(&self, rank: i32) -> i64 {
        self.rank_xp_base + self.rank_xp_per_rank * rank.max(0) as i64
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CombatMath {
    pub min_damage: i32,
    pub damage_floor_fraction: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorldScaling {
    pub tier_divisor: f64,
    pub mlevel_divisor: f64,
    pub stat_mult_base_divisor: f64,
    pub stat_mult_exp: f64,
    pub def_mult_exp: f64,
    pub hp_per_tier: f64,
    /// The shallow-ring on-ramp: creature power at the hub, ramping to 1.0 by
    /// `onboarding_distance`.
    pub onboarding_floor: f64,
    pub onboarding_distance: i64,
    /// XP curve exponent (spec §4): `xp = floor(base_xp × (1 + d/divisor)^exp)`
    /// — steeper than the stat curve so deep kills out-reward the grind.
    pub xp_distance_exp: f64,
    pub red_chest_floor_distance: i64,
}

/// Procedural area-generation tunables (world-generation.md subset).
#[derive(Debug, Clone, Deserialize)]
pub struct WorldGen {
    pub area_count: usize,
    /// Dungeons (WG-1): every Nth procedural section is a dungeon (0 = disabled).
    pub dungeon_every: usize,
    pub dungeon_rooms: usize,
    pub dungeon_creature_mult: f64,
    pub dungeon_wall_radius: f64,
    pub dungeon_door_half: f64,
    /// DG-3: per-streamed-section chance of a hand-designed dungeon entrance.
    pub dungeon_spawn_chance: f64,
    /// No dungeon entrance closer to the hub than this.
    pub dungeon_min_distance: f64,
    /// DG-3/DG-5: effective-distance (difficulty + loot) added per dungeon floor.
    pub dungeon_depth_level_step: i64,
    /// DG-3b/DG-4: base HP a sprung dungeon trap deals (scaled up by dungeon depth).
    pub dungeon_trap_damage: i32,
    /// WG-4: crossing this far west of the hub returns the player to Last City.
    pub west_return_border: f64,
    /// WG-4: bend the generated corridor into a radial arc of this many degrees.
    pub radial_arc_degrees: f64,
    pub base_area_length: f64,
    pub area_length_growth: f64,
    pub area_length_jitter: f64,
    pub monster_spacing: f64,
    pub monster_spacing_jitter: f64,
    pub lateral_jitter: f64,
    pub first_monster_x: f64,
    pub first_area_portal_gap: f64,
    pub portal_setback: f64,
    pub world_margin: f64,
    pub lateral_half_extent: f64,
    pub creature_lateral_spread: f64,
    /// How many times a section's corridor is walked laying creatures — one walk per
    /// corridor-width of arc, so the radial fan doesn't thin creature density with depth.
    /// Capped here because the true stretch passes 30× and the world streams forever.
    pub creature_radial_lane_cap: f64,
    /// Creature-free safe ring around the Center Hub in the spawn section (must stay
    /// above `[ai] aggro_radius` so a just-spawned player isn't instantly aggro'd).
    pub hub_safe_radius: f64,
    pub resources_per_area: f64,
    pub resource_lateral_spread: f64,
    pub obstacles_per_area: f64,
    /// Forest sections pack this multiple of `obstacles_per_area` extra trees into
    /// the play area (dense maze); other biomes keep the base density.
    pub field_obstacle_mult: f64,
    pub forest_obstacle_mult: f64,
    /// Per-biome fill density so each biome FEELS distinct (open desert, dense
    /// Ashfall mountain-pass, choked mire, …). `maze_obstacle_mult` is the fallback.
    pub desert_obstacle_mult: f64,
    pub ashfall_obstacle_mult: f64,
    pub tundra_obstacle_mult: f64,
    pub mire_obstacle_mult: f64,
    /// Share of the maze fill that is the biome's SIGNATURE kind; the rest is drawn from
    /// its authored `obstacles_for_biome` list.
    pub fill_signature_share: f64,
    /// Fallback fill density for any biome without its own multiplier.
    pub maze_obstacle_mult: f64,
    /// Cap on the radial density compensation: a deep area's arc is much wider than the
    /// corridor, so its obstacle count is scaled up by the arc-stretch (min 1.0) to keep
    /// maze density roughly constant with depth — but never beyond this multiple, so the
    /// deepest areas don't blow up the obstacle count. 1.0 disables the compensation.
    pub maze_radial_scale_cap: f64,
    /// World units over which a section's fill density blends toward the neighbouring
    /// section's near its edges (forest thins into desert; rock thickens into Ashfall).
    pub biome_transition_width: f64,
    pub obstacle_min_radius: f64,
    pub obstacle_max_radius: f64,
    /// Water's own radius range. A body of water has to be a BODY — sharing the prop range
    /// made every pond a 2-5 unit puddle. Bounded, because `BlockField`'s spatial-hash cell
    /// is sized from the largest radius in the world.
    pub water_min_radius: f64,
    pub water_max_radius: f64,
    pub path_clear_radius: f64,
    pub path_meander: f64,
    /// Extra WEB trails woven through each section (branches, loops, dead-end spurs)
    /// so the overworld reads as an interconnected maze of routes with real junctions,
    /// not a single lane. The clear tube is carved around these too. 0 ⇒ just the backbone.
    pub web_trails_per_area: f64,
    /// How far a web fork leaves the backbone, in WORLD units (see balance.toml).
    pub web_offset_min: f64,
    pub web_offset_max: f64,
    pub web_spur_offset: f64,
    pub player_radius: f64,
    /// How far ahead of the frontier player the world streams new sections in.
    pub stream_lookahead: f64,
    /// Probability a procedural section's CLEAR PATH climbs onto a mid-segment
    /// plateau (up a ramp, across, back down) — the "path itself is a maze" knob.
    /// Endpoints stay on level 0, so feasibility is preserved.
    pub path_climb_chance: f64,
    /// When a section's clear path climbs a summit, the probability a gate-boss guards
    /// the top (otherwise a guaranteed treasure chest crowns it). Boss is held off the
    /// tutorial regardless. The "there's always a payoff for the climb" knob (#3).
    pub peak_boss_chance: f64,
    /// Minimum WORLD terrain height a clear-path crest must reach to earn a summit
    /// reward (#3, heightmap era). Filters flat/gentle sections so a boss/chest only
    /// crowns a genuine hill the route actually climbs — not every meander bump.
    pub summit_min_height: f64,
    /// World-radius of an authored CLIMBABLE landmark mountain (#3). Its height is
    /// `radius * terrain::PEAK_MAX_ASPECT * ~0.9`, keeping the dome walkable (you climb
    /// it) while reading as a real peak; a boss/treasure sits on the summit.
    pub peak_radius: f64,
    /// Nearest hub distance an authored peak may spawn — keeps the big domes out of the
    /// tight near-hub rings where they'd swamp the width.
    pub peak_min_distance: f64,

    /// **CONTINENTS (WG-7).** Chance a deep-enough section holds a STRAIT — an inland sea
    /// filling an annular sector, pierced by isthmuses. The CONTINENT is the land between
    /// two of them. Deliberately under 1.0: a continent should be several sections across,
    /// and a world where every ring is a coast is an archipelago.
    pub ridge_chance: f64,
    pub ridge_min_section: usize,
    pub ridge_arc_share: f64,
    pub ridge_arc_share_min: f64,
    pub ridge_arc_share_max: f64,
    pub ridge_spoke_rings_min: u32,
    pub ridge_spoke_rings_max: u32,
    pub ridge_max_per_section: usize,
    /// Boundaries per section that may be walled with PROPS (its own budget — see the
    /// note in `balance.toml`; a prop wall costs O(length), a range costs per-sample geometry).
    pub prop_wall_max_per_section: usize,
    pub ridge_passes_min: usize,
    pub ridge_passes_max: usize,
    pub ridge_pass_share: f64,
    pub ridge_half_width_min: f64,
    pub ridge_half_width_max: f64,
    pub ridge_aspect: f64,
    pub ridge_pass_width: f64,
    /// How far out the decided maze is built. Past the taper the world is a corridor.
    pub maze_horizon: f64,
    /// Share of the maze's DEAD ENDS given a second way out, so it is not a bare tree.
    pub maze_braid: f64,
    /// Half-width of a mire boundary channel, in world units.
    pub water_wall_half_width: f64,
    /// Spacing of a water wall's chain nodes along the boundary.
    pub water_wall_node_step: f64,
    /// How tightly a PROP wall is packed, as a share of the widest impassable gap.
    pub prop_wall_tightness: f64,
    /// `WG-11` stage 6: share of passes that get a part at all (0..1).
    pub pass_part_chance: f64,
    /// How far a part's piece sits off the crossing's centre line, in world units.
    pub pass_part_stagger: f64,
    /// A piece is refused unless this much of the pass mouth stays open beside it.
    pub pass_part_min_gap: f64,
    pub ridge_segments_min: usize,
    pub ridge_segments_max: usize,
    pub bridge_half_width: f64,
    pub bridge_join_gap: f64,
    pub bridge_max_span: f64,
    pub strait_chance: f64,
    /// Earliest section index that may hold a strait. Sections grow with depth
    /// (`base_area_length + area_length_growth*i`), so this is simultaneously what makes a
    /// section thick enough to hold a sea with dry land on both shores, and what keeps the
    /// on-ramp coastline-free.
    pub strait_min_section: usize,
    /// A strait's radial thickness as a share of its own section's length. Bounded well
    /// under 1 so land always remains on BOTH shores inside the same section — the clear
    /// path enters that section on one side and leaves on the other, so A* always has dry
    /// ground at each end to route an isthmus crossing between.
    pub strait_thickness_share: f64,
    /// Narrowest angular span of a strait, in degrees. Below this it reads as a river
    /// rather than a sea, and you would cross it without noticing there was a choice.
    pub strait_span_min_degrees: f64,
    /// Widest angular span, in degrees. Capped so "walk around its end" stays a real
    /// alternative to an isthmus instead of a joke.
    pub strait_span_max_degrees: f64,
    /// Isthmuses per strait. **Two**, not one — one door is the retired `Seam`, which
    /// funnelled the world into a corridor. With the span's two ends that is four ways past.
    pub strait_bridges: usize,
    /// Half the arc width of an isthmus, in WORLD units rather than radians (an angular
    /// bridge is a few units wide near the hub and hundreds at the frontier). Must stay
    /// above [`meld_proto::coast::MIN_BRIDGE_HALF_WIDTH`], the walkability floor
    /// `coast::strait_is_crossable` enforces.
    pub strait_bridge_half_width: f64,

    /// **BAYS (WG-7).** Chance a deep-enough section's rim holds a bay — a disc of water
    /// biting inward. Lower than `strait_chance`, because the fan has TWO rims so this
    /// fires twice per band on average, and a coast bitten at every ring is a fjord system.
    pub bay_chance: f64,
    /// Earliest section index that may hold a bay or an isle. Shallower than a strait's
    /// gate: a bay does not block the way out, it only bends it.
    pub bay_min_section: usize,
    /// Nearest hub distance a bay may be cut. The western gap near the hub is a narrow
    /// wedge closed by the neck, so a bay there would eat the walk home to Last City.
    pub bay_min_reach: f64,
    /// A bay's radius as a share of the local half-arc — smallest and largest drawn. Both
    /// must stay under [`meld_proto::coast::BAY_LAND_SHARE`], which is the hard guarantee
    /// `coast::bay_leaves_a_shore` enforces; these are the range drawn inside it.
    pub bay_radius_share_min: f64,
    pub bay_radius_share_max: f64,
    /// ⚠️ **Absolute** cap on a bay's radius, world units. The share above is a share of the
    /// LOCAL HALF-ARC, which grows linearly with depth — so at r=2000 a 0.30 share is a
    /// 1,500-unit "bay", which is a sea, and no `nudge_ashore` can walk a creature out of
    /// one. The share keeps a bay from severing the fan; this keeps it a coastal *feature*.
    /// Both apply, whichever is smaller.
    pub bay_radius_max: f64,
    /// **ISLES (WG-7).** Chance a section stands an isle offshore of its rim. Freer than a
    /// bay because an isle is outside the fan and cannot block anything.
    pub isle_chance: f64,
    /// Isle radius range, world units.
    pub isle_radius_min: f64,
    pub isle_radius_max: f64,
    /// How far past the fan's rim an isle's shore sits, as an ARC length rather than an
    /// angle — an angular offset would beach it on the rim near the hub and strand it over
    /// the horizon at the frontier.
    pub isle_offshore_min: f64,
    pub isle_offshore_max: f64,
    /// **INLAND WATER (WG-7).** Earliest section index, and nearest hub distance, that may
    /// hold a basin or a river. The on-ramp stays dry: a river across a player's first
    /// minutes is a barrier before they know what a ford is.
    pub water_min_section: usize,
    pub water_min_reach: f64,
    /// Chance a deep-enough section springs a river.
    pub river_chance: f64,
    /// World units between river nodes, and the node budget before a river gives up and
    /// pools into a lake instead. The budget caps the wire payload and the descent cost.
    pub river_step: f64,
    pub river_max_nodes: usize,
    /// A channel's half-width: the low end reads as a creek, the high end as a river.
    pub river_half_width_min: f64,
    pub river_half_width_max: f64,
    /// A FORD every this many nodes. ⚠️ A **guarantee**, not a decoration — connectedness is
    /// what a river is, and a connected impassable line is exactly what disconnects a
    /// world. Same contract as a strait's isthmus.
    pub river_ford_every: usize,
    /// Chance a section also holds standing water with no river feeding it.
    pub basin_chance: f64,
    /// Bounds on how far a basin may spread. The SHAPE is the terrain's own contour; these
    /// only stop a flat hollow flooding the whole ring.
    pub basin_radius_min: f64,
    pub basin_radius_max: f64,
    /// How far above a hollow's floor the water surface sits, in HEIGHT units (the field
    /// runs about ±16). A deeper fill floods wider, because a contour is wider higher up.
    pub basin_fill: f64,
}

/// Creature AI tunables (overworld movement + encounter grouping).
#[derive(Debug, Clone, Deserialize)]
pub struct Ai {
    /// CR-9: creature level below which no ordinary spawn is upgraded to a tactical
    /// targeting profile.
    pub smart_level_floor: i32,
    /// Share of ordinary spawns upgraded at the floor, its growth per level, and its cap.
    pub smart_chance_base: f64,
    pub smart_chance_per_level: f64,
    pub smart_chance_cap: f64,
    /// Chance per attacking turn that a ganging pack re-picks its mark.
    pub gang_switch_chance: f64,
    pub wander_speed: f64,
    /// How long a creature commits to one wander destination before picking another.
    /// Load-bearing: the destination used to be re-rolled every 100 ms tick, which
    /// made every creature vibrate in place instead of going anywhere (see
    /// `MonsterSpawn::wander_to`).
    pub wander_leg_seconds: f64,
    /// How close counts as arrived, so the next leg is picked instead of jittering
    /// through the destination point.
    pub wander_arrive_radius: f64,
    /// Chance that a finished leg is followed by standing still, and how long for
    /// (jittered ±50%). A creature that never stops reads as machinery.
    pub wander_pause_chance: f64,
    pub wander_pause_seconds: f64,
    pub chase_speed: f64,
    pub aggro_radius: f64,
    pub territorial_aggro_radius: f64,
    pub leash_radius: f64,
    pub group_radius: f64,
    pub flee_hp_fraction: f64,
    pub join_radius: f64,
    /// `SOC-3`: how close a fight has to be to WATCH it. Deliberately wider than
    /// `join_radius` — you can see further than you can reach, and a watcher who is
    /// shoved out of the feed the instant they stop pressing forward would never get
    /// to read the fight they walked over for.
    pub watch_radius: f64,
    /// `CR-2`: how much of its own max HP a roaming creature mends per second. A wound
    /// has to CLOSE, or the world becomes strip-minable by attrition — walk a ring,
    /// chip everything, come back. But it has to close SLOWLY, or a creature you found
    /// halfway dead is worth nothing by the time you reach it. This is the width of that
    /// window, and it is a fraction because a creature's max HP spans two orders of
    /// magnitude between the on-ramp and d3200.
    ///
    /// It does NOT run while the creature is in a clash or a battle: nothing mends while
    /// it is still being hit, and the clash's own linger is what covers the gap between
    /// one blow and the next.
    pub creature_regen_fraction_per_sec: f64,
    /// `CR-2`: seconds a clash stays "live" after the last blow lands in it. Creatures
    /// trade blows on a cadence, so a clash with no grace period would blink out
    /// between swings and take the ⚔ marker — and any watcher — with it.
    pub clash_linger_seconds: f64,
    /// Overworld creature-vs-creature skirmish: hostile-faction creatures hunt
    /// each other within this range, trade blows once `skirmish_attack_range`
    /// close, on a `skirmish_attack_interval`-second cadence.
    pub skirmish_aggro_radius: f64,
    pub skirmish_attack_range: f64,
    pub skirmish_attack_interval: f64,
    /// A player auto-collects a ground-loot drop within this range.
    pub loot_pickup_radius: f64,
}

/// Overworld class-perk tunables ("party sense"): each hero class, when present
/// in the party, grants a distinct overworld utility whose tier scales with the
/// shared `run_level`. See the `[perks]` block in balance.toml and the CANON
/// class taxonomy. All thresholds are run-level values (per-hero level is uniform).
#[derive(Debug, Clone, Deserialize)]
pub struct Perks {
    // --- Explorer: night glow + "predator's eye" monster intel. ---
    /// Avatar-light intensity at run level 1 (client scales it by night darkness).
    pub explorer_glow_base: f32,
    /// Added avatar-light intensity per run level above 1.
    pub explorer_glow_per_level: f32,
    /// Run level that reveals a mob's LEVEL over its head.
    pub hunter_intel_level_at: i32,
    /// Run level that additionally reveals a mob's HP bar.
    pub hunter_intel_hp_at: i32,
    /// Run level that additionally reveals enemy ATB gauges in battle.
    pub hunter_intel_atb_at: i32,
    // --- Shifter: corner minimap. ---
    /// Run level that unlocks the minimap (+ mob/portal dots).
    pub explorer_map_at: i32,
    /// Run level that adds treasure-chest dots.
    pub explorer_map_chests_at: i32,
    /// Run level that adds harvestable (resource-node) dots.
    pub explorer_map_harvest_at: i32,
    /// World-units the minimap covers at unlock.
    pub explorer_map_radius_base: f32,
    pub shifter_dungeon_at: i32,
    pub shifter_dungeon_radius_base: f32,
    pub shifter_dungeon_radius_per_level: f32,
    pub shifter_item_sense_at: i32,
    pub shifter_trap_sense_at: i32,
    pub shifter_trap_radius_base: f32,
    pub shifter_trap_radius_per_level: f32,
    /// Extra minimap coverage per run level above the unlock.
    pub explorer_map_radius_per_level: f32,
    // --- Hunter: threat sense (the long-range half of the predator's eye). ---
    /// Run level that marks elite/gatekeeper mobs.
    pub hunter_threat_elites_at: i32,
    /// Run level that additionally marks aggressive mobs.
    pub hunter_threat_aggro_at: i32,
    /// Extended mob interest radius (tiles) at unlock — dangerous mobs revealed
    /// beyond the normal snapshot radius.
    pub hunter_reveal_base: f64,
    /// Extra reveal radius per run level.
    pub hunter_reveal_per_level: f64,
    // --- Psyker: telekinesis (the pin) + Mind Link. ---
    /// Run level at which the pin is earned.
    pub psyker_hold_at: i32,
    /// Seconds one pin holds at unlock, and the growth/ceiling on it.
    pub psyker_hold_seconds_base: f32,
    pub psyker_hold_seconds_per_level: f32,
    pub psyker_hold_seconds_cap: f32,
    /// Seconds between pins at unlock, shortening with level but never to nothing.
    pub psyker_hold_cooldown_base: f32,
    pub psyker_hold_cooldown_per_level: f32,
    pub psyker_hold_cooldown_floor: f32,
    /// Run level for a second simultaneous pin, its growth, and its ceiling.
    pub psyker_hold_targets_at: i32,
    pub psyker_hold_targets_per_level: i32,
    pub psyker_hold_targets_cap: i32,
    /// World-units a Psyker can reach to pin a creature.
    pub psyker_hold_radius: f32,
    /// Run level at which co-op teammates ride the snapshot at any distance.
    pub psyker_mind_link_at: i32,
    // --- Resonant: overworld regen. ---
    /// HP/sec restored to each carried hero while walking the overworld, per run
    /// level (0 at level 0). Applied server-side; feeds next fight's start HP.
    pub resonant_regen_per_level: f32,
    // --- Phoenix Guard: bulwark (reduced skirmish/aggro pull). ---
    /// Fraction the creature aggro/skirmish radius shrinks per run level.
    /// The Order of the Iron Hull on the overworld: the Resonant Wake as a standing
    /// deterrent, and Hull-Listening — an ear to the ground.
    pub iron_hull_wake_at: i32,
    pub iron_hull_wake_reduction_per_level: f64,
    pub iron_hull_wake_mult_floor: f64,
    pub iron_hull_listen_at: i32,
    pub iron_hull_listen_radius_base: f32,
    pub iron_hull_listen_radius_per_level: f32,
    /// The Wall Defense Force on the overworld: Recall Blade reaching for loot, and
    /// Inertial Nullification making every cliff a route down.
    pub rift_knight_recall_at: i32,
    pub rift_knight_recall_radius_base: f32,
    pub rift_knight_recall_radius_per_level: f32,
    pub rift_knight_drop_at: i32,
    pub phoenix_guard_aggro_reduction_per_level: f64,
    /// Lowest the aggro multiplier can fall to (a floor so mobs never fully ignore).
    pub phoenix_guard_aggro_mult_floor: f64,
    /// Obstacle radius an Phoenix Guard party can trample through (0 = disabled; stretch).
    pub phoenix_guard_trample_radius: f64,
    // --- Smithwright: benches, and the rock they are built from. ---
    pub smithwright_ore_sense_at: i32,
    pub smithwright_ore_radius_base: f32,
    pub smithwright_ore_radius_per_level: f32,
    pub smithwright_setup_at: i32,
    pub smithwright_setup_mult: f64,
    pub smithwright_stock_discount: i32,
    pub smithwright_pack_full_at: i32,
    pub smithwright_bench_uses_at: i32,
    pub smithwright_bench_uses_bonus: i32,
    // --- Keeper: the ground, and what it will give up. ---
    pub keeper_reagent_sense_at: i32,
    pub keeper_reagent_radius_base: f32,
    pub keeper_reagent_radius_per_level: f32,
    pub keeper_green_thumb_at: i32,
    pub keeper_green_thumb_chance: f64,
    pub keeper_rooted_at: i32,
    pub keeper_rooted_radius_mult: f32,
    pub keeper_rooted_regen_mult: f32,
    pub keeper_whole_vein_at: i32,
    pub keeper_whole_vein_chance: f64,
}

/// Content-ish stat blocks. Keyed by content id (e.g. `forest_bloom_stalker`).
pub type Creatures = std::collections::HashMap<String, CreatureStats>;
pub type Players = std::collections::HashMap<String, PlayerStats>;
pub type Resources = std::collections::HashMap<String, ResourceStats>;

/// Harvest-channel knobs (MS-2). Keyed by **material class** rather than by node id,
/// because the rhythm — a patch of quick gathers vs a long dangerous dig — is what
/// separates the two gathering professions.
#[derive(Debug, Clone, Deserialize)]
pub struct Harvest {
    pub reagent_stock: i32,
    pub reagent_tick_ms: u64,
    pub ore_stock: i32,
    pub ore_tick_ms: u64,
    pub wood_stock: i32,
    pub wood_tick_ms: u64,
    pub stone_stock: i32,
    pub stone_tick_ms: u64,
    pub default_stock: i32,
    pub default_tick_ms: u64,
}

impl Harvest {
    /// How many units a node of this material class holds, and how long each unit
    /// takes to cut loose. `class` is a `meld_proto::materials::MaterialClass` wire
    /// word; anything unrecognised falls back to the defaults rather than failing —
    /// a new material class should change the pace of the game, not break spawning.
    pub fn node_yield(&self, class: &str) -> (i32, u64) {
        match class {
            "reagent" => (self.reagent_stock, self.reagent_tick_ms),
            "ore" => (self.ore_stock, self.ore_tick_ms),
            "wood" => (self.wood_stock, self.wood_tick_ms),
            "stone" => (self.stone_stock, self.stone_tick_ms),
            _ => (self.default_stock, self.default_tick_ms),
        }
    }
}

/// A harvestable resource node's content, keyed by node id (e.g. `bloom_herb`).
#[derive(Debug, Clone, Deserialize)]
pub struct ResourceStats {
    /// Item kind banked into the backpack when harvested (feeds crafting).
    pub material: String,
    /// Meld skill credited on harvest (`forging` | `alchemy`).
    pub skill: String,
    /// Skill XP granted per harvest.
    pub xp: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreatureStats {
    pub base_hp: i32,
    pub base_atk: i32,
    pub base_def: i32,
    pub speed_stat: i32,
    pub xp_reward: i64,
    pub encounter_class: String,
    /// Faction (for grouping + hostility). See meld-proto::factions.
    pub faction: String,
    /// `passive` | `territorial` | `aggressive` — overworld movement style.
    pub aggression: String,
    /// Whether this creature flees a losing battle.
    #[serde(default)]
    pub flees: bool,
    /// Item kind dropped as ground loot when this creature is felled by an
    /// overworld skirmish (players walk over it to collect).
    #[serde(default = "default_loot_kind")]
    pub loot_kind: String,
}

fn default_loot_kind() -> String {
    "monster_trophy".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayerStats {
    pub base_hp: i32,
    pub base_atk: i32,
    pub base_def: i32,
    pub speed_stat: i32,
    /// Level-1 attribute baseline.
    pub str: i32,
    pub mnd: i32,
    pub dex: i32,
    pub wll: i32,
    /// Attribute points auto-gained per level (the class's growth focus).
    pub str_per_level: i32,
    pub mnd_per_level: i32,
    pub dex_per_level: i32,
    pub wll_per_level: i32,
    /// Elemental/psychic resistance at level 1 — `base_def`'s counterpart, grown by Mnd.
    #[serde(default)]
    pub base_ward: i32,
}

impl PlayerStats {
    /// The four attributes at `level` = baseline + per-level gain × (level-1).
    /// Returns `(str, mnd, dex, wll)`.
    pub fn attributes_at(&self, level: i32) -> (i32, i32, i32, i32) {
        let steps = (level - 1).max(0);
        (
            self.str + self.str_per_level * steps,
            self.mnd + self.mnd_per_level * steps,
            self.dex + self.dex_per_level * steps,
            self.wll + self.wll_per_level * steps,
        )
    }
}

impl Balance {
    /// Parse a balance TOML string.
    pub fn from_toml_str(s: &str) -> Result<Self, BalanceError> {
        Ok(toml::from_str(s)?)
    }

    /// Load from a path on disk.
    pub fn load(path: &str) -> Result<Self, BalanceError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| BalanceError::Io(path.to_string(), e))?;
        Self::from_toml_str(&text)
    }

    /// The default `balance.toml`, embedded at compile time. This is what makes a
    /// shipped binary (e.g. the self-contained QA build) self-sufficient: it needs
    /// no `balance/balance.toml` on disk beside it. `load_default` still prefers a
    /// live file when one is present, so in-repo runs pick up local tweaks.
    pub const EMBEDDED_DEFAULT: &'static str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../balance/balance.toml"));

    /// Load the balance table, in priority order:
    /// 1. `MELD_BALANCE` env → that file (explicit override).
    /// 2. The checked-in `balance/balance.toml`, if it exists on disk (in-repo runs).
    /// 3. Otherwise the [`Self::EMBEDDED_DEFAULT`] baked into the binary, so a
    ///    standalone binary works with no config file present.
    pub fn load_default() -> Result<Self, BalanceError> {
        if let Ok(p) = std::env::var("MELD_BALANCE") {
            return Self::load(&p);
        }
        // CARGO_MANIFEST_DIR of this crate is server/crates/meld-balance.
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../balance/balance.toml");
        if std::path::Path::new(root).exists() {
            return Self::load(root);
        }
        Self::from_toml_str(Self::EMBEDDED_DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_balance_parses_and_has_creature() {
        let b = Balance::load_default().expect("balance.toml parses");
        assert_eq!(b.battle.tick_ms, 100);
        assert_eq!(b.auth.bcrypt_cost, 12);
        assert!(b.creature.contains_key("forest_bloom_stalker"));
        assert!(b.player.contains_key("explorer"));
        // Overworld class perks load.
        // The overworld perks moved to the classes whose fantasy they are: the map to
        // the Explorers, the predator's eye to the Hunters, Shift-sense to the Shifter.
        assert_eq!(b.perks.hunter_intel_hp_at, 3);
        assert_eq!(b.perks.explorer_map_at, 1);
        assert!(b.perks.shifter_dungeon_radius_base > 0.0);
        assert!(b.perks.shifter_trap_radius_base > 0.0);
        assert!(b.perks.shifter_item_sense_at >= 1);
        assert!(b.perks.phoenix_guard_aggro_mult_floor > 0.0 && b.perks.phoenix_guard_aggro_mult_floor <= 1.0);
    }
}
