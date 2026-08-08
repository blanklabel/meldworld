//! Realtime C2S/S2C message payloads, grouped by domain
//! (docs/interfaces/realtime-protocol.md and its detail files).
//!
//! Each payload struct binds to its wire `type` string via [`Message::TYPE`],
//! so the gateway can peek a [`crate::RawEnvelope`], match the string, and
//! decode into the right struct. Only the subset the today-slice uses is
//! modelled; the rest land as their systems do.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::common::{Combatant, ItemStack, LootGear, Position};
use crate::enums::*;
use crate::Id;

/// Binds a payload struct to its canonical `<domain>.<verb>` wire type.
pub trait Message: Serialize + DeserializeOwned {
    const TYPE: &'static str;
}

// ---------------------------------------------------------------- session ---

pub mod session {
    use super::*;

    /// C2S — first frame on a socket; presents a realtime ticket (session.md).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Authenticate {
        pub ticket: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub resume: Option<Resume>,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Resume {
        pub session_id: Id,
        pub last_server_seq: u32,
    }
    impl Message for Authenticate {
        const TYPE: &'static str = "session.authenticate";
    }

    /// S2C — handshake success + session parameters.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Authenticated {
        pub client_seq: u32,
        pub session_id: Id,
        pub player_id: Id,
        pub resumed: bool,
        pub heartbeat_interval_ms: i32,
        pub grace_window_ms: i32,
        pub server_ts: u64,
        pub last_client_seq: u32,
    }
    impl Message for Authenticated {
        const TYPE: &'static str = "session.authenticated";
    }

    /// C2S — keepalive ping (empty payload).
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Heartbeat {}
    impl Message for Heartbeat {
        const TYPE: &'static str = "session.heartbeat";
    }

    /// S2C — keepalive pong.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct HeartbeatAck {
        pub client_seq: u32,
        pub server_ts: u64,
    }
    impl Message for HeartbeatAck {
        const TYPE: &'static str = "session.heartbeat_ack";
    }

    /// S2C — the single rejection message for any failed C2S intent.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Error {
        pub code: ErrorCode,
        pub message: String,
        pub client_seq: Option<u32>,
    }
    impl Message for Error {
        const TYPE: &'static str = "session.error";
    }

    /// S2C — server-initiated close notice.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Terminated {
        pub reason: TerminateReason,
        pub resumable: bool,
    }
    impl Message for Terminated {
        const TYPE: &'static str = "session.terminated";
    }
}

// --------------------------------------------------------------- movement ---

pub mod movement {
    use super::*;

    /// C2S — a movement input sample (movement-world.md).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MoveIntent {
        pub input_seq: u32,
        pub move_dir: MoveDir,
        pub client_pos: Position,
    }
    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct MoveDir {
        pub x: f64,
        pub y: f64,
    }
    impl Message for MoveIntent {
        const TYPE: &'static str = "movement.move_intent";
    }

    /// S2C — authoritative position override.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PositionCorrection {
        pub position: Position,
        pub last_input_seq: u32,
    }
    impl Message for PositionCorrection {
        const TYPE: &'static str = "movement.position_correction";
    }

    /// S2C — periodic dynamic-entity snapshot in interest radius.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Snapshot {
        pub server_tick: i64,
        pub entities: Vec<SnapshotEntity>,
    }
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct SnapshotEntity {
        pub entity_id: Id,
        pub position: Position,
        pub velocity: Velocity,
        pub avatar_state: Option<String>,
        /// Elevation level this entity stands on (terraced verticality). Absent →
        /// ground level 0; old clients ignore it. The client raises the entity's
        /// render height by `level × step_height`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub level: Option<u8>,
        /// Overworld mob intel (Explorer/Psyker perks). All absent for non-mobs and
        /// old wire; the client renders each only when its own party perk unlocks
        /// it (nameplates gated by `run.perks`). `mob_level` is the creature's
        /// combat level; `hp`/`max_hp` drive the pre-fight HP bar (mobs already
        /// lose HP to hostile-faction skirmishes out of battle); `encounter_class`
        /// (standard|elite|gatekeeper) and `aggression` (passive|territorial|
        /// aggressive) drive the Psyker threat marker.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub mob_level: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub hp: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max_hp: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub encounter_class: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub aggression: Option<String>,
    }
    #[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
    pub struct Velocity {
        pub x: f64,
        pub y: f64,
    }
    impl Message for Snapshot {
        const TYPE: &'static str = "world.snapshot";
    }
}

// ------------------------------------------------------------------ world ---

/// Static section geometry for terraced verticality (docs/proposals/verticality.md).
/// The overworld streams in as a sequence of **sections**; each carries a coarse
/// elevation grid + the connectors (ladders/ropes/slopes) that join levels. The
/// client builds one stepped ground+cliff mesh per section and spawns the
/// connector props. Sent per initial section at run start, and again for each new
/// section the server streams in as the player advances (endless world).
pub mod world {
    use super::*;

    /// One connector joining two elevation levels.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ConnectorDto {
        pub kind: String, // "slope" | "ladder" | "rope"
        pub position: Position,
        pub lo: u8,
        pub hi: u8,
        pub radius: f64,
    }

    /// S2C — one section's elevation field + connectors (+ its trail contribution
    /// for streamed sections). `levels` is row-major `levels[gx*rows + gy]`.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TerrainSection {
        pub index: u32,
        pub start_x: f64,
        pub end_x: f64,
        pub y_min: f64,
        pub cell: f64,
        pub cols: u32,
        pub rows: u32,
        pub levels: Vec<u8>,
        pub connectors: Vec<ConnectorDto>,
        /// This section's clear-path waypoints, so a streamed section extends the
        /// trail. Empty for initial-chain sections (already in `run.started.path`).
        #[serde(default)]
        pub path: Vec<Position>,
        /// The section's biome theme (`forest`/`desert`/`ashfall`/`tundra`/`mire`).
        /// Since a section occupies a radius ring (radius = corridor x in the radial
        /// world), the client keys the ground texture + biome HUD off the ACTUAL
        /// per-section biome from these, not the fixed distance bands — so ground,
        /// label, and the section's creatures/obstacles finally agree.
        #[serde(default)]
        pub biome: String,
        /// WG-4 radial world: half the fan arc in **radians** (0 ⇒ flat corridor, no
        /// bend). The terrain grid above is in un-bent corridor coords; the client
        /// bends each terrace/cliff/connector vertex by the same arc the server used
        /// to fan entity positions, so the raised ground lines up with where you walk.
        #[serde(default)]
        pub radial_half: f64,
        /// The corridor half-extent the arc maps against (corridor y ∈ [−lat, lat] ↦
        /// bearing ∈ [−radial_half, radial_half]). Pairs with `radial_half` for the bend.
        #[serde(default)]
        pub corridor_lateral: f64,
        /// Authored CLIMBABLE landmark peaks this section adds (world-space
        /// `[cx, cz, radius, height]`; see `terrain::peak_height` / `run.started.peaks`).
        #[serde(default)]
        pub peaks: Vec<[f32; 4]>,
    }
    impl Message for TerrainSection {
        const TYPE: &'static str = "world.terrain_section";
    }

    /// WG-1/DG-6b: the client's cue to re-skin the whole environment as a
    /// **secluded dungeon** rather than the open overworld. The playable floor is
    /// still just the server's `Snapshot` walls (a thin blocking perimeter); this
    /// message tells the client the *theme* + *bounds* so it can, client-side only,
    /// swap the ground, dim the sky, and ring the play area with a dense,
    /// collision-free biome enclosure (a forest wall for a `forest` dungeon) so no
    /// overworld shows through. Sent on descent and on every floor change with
    /// `active = true`; sent once with `active = false` on exit/death to restore the
    /// overworld look. Purely presentational — no gameplay rides on it.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct DungeonScene {
        /// `true` while inside a dungeon floor; `false` once returned to the overworld.
        pub active: bool,
        /// The dungeon's biome theme (`forest`/`desert`/`ashfall`/`tundra`/`mire`),
        /// keying the floor material + enclosure props. Empty when `active = false`.
        #[serde(default)]
        pub theme: String,
        /// Current floor index (0-based), for a subtle depth cue.
        #[serde(default)]
        pub floor: u32,
        /// Floor grid bounds in tiles — the client rings the enclosure just outside
        /// `[0,width] × [0,height]` (tile = 1 world unit, matching `dungeon_snapshot`).
        #[serde(default)]
        pub width: u32,
        #[serde(default)]
        pub height: u32,
    }
    impl Message for DungeonScene {
        const TYPE: &'static str = "world.dungeon_scene";
    }
}

// ----------------------------------------------------------------- battle ---

pub mod battle {
    use super::*;

    /// S2C — a battle subscreen opened (battle.md).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Started {
        pub battle_id: Id,
        pub encounter_class: EncounterClass,
        pub allies: Vec<Combatant>,
        pub enemies: Vec<Combatant>,
        /// The first combatant this player controls (back-compat single-hero id).
        pub your_combatant_id: Id,
        /// Every combatant this player controls (a solo player fields a party of
        /// four; in co-op each player controls their one hero).
        #[serde(default)]
        pub your_combatant_ids: Vec<Id>,
        pub triggered_by: Option<Id>,
    }
    impl Message for Started {
        const TYPE: &'static str = "battle.started";
    }

    /// S2C — a second party merged into an active battle (raid merge).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PartyJoined {
        pub battle_id: Id,
        pub joining_instance_id: Id,
        pub joining_allies: Vec<Combatant>,
    }
    impl Message for PartyJoined {
        const TYPE: &'static str = "battle.party_joined";
    }

    /// S2C — a combatant's gauge filled; a player's 15s window opens.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TurnReady {
        pub battle_id: Id,
        pub combatant_id: Id,
        pub timeout_at: Option<u64>,
    }
    impl Message for TurnReady {
        const TYPE: &'static str = "battle.turn_ready";
    }

    /// S2C — authoritative gauge/HP sync (event-driven + 1 Hz keepalive).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct GaugeUpdate {
        pub battle_id: Id,
        pub server_tick: i64,
        pub combatants: Vec<GaugeEntry>,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct GaugeEntry {
        pub combatant_id: Id,
        pub gauge: f64,
        pub hp: i32,
        pub statuses: Vec<String>,
    }
    impl Message for GaugeUpdate {
        const TYPE: &'static str = "battle.gauge_update";
    }

    /// C2S — submit the acting player's chosen action.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SubmitAction {
        pub battle_id: Id,
        pub action_id: Id,
        pub action: BattleActionKind,
        /// Which of the sender's combatants is acting. Optional for back-compat
        /// (absent → the player's first/only hero).
        #[serde(default)]
        pub actor_combatant_id: Option<Id>,
        #[serde(default)]
        pub skill_kind: Option<String>,
        #[serde(default)]
        pub item_id: Option<Id>,
        #[serde(default)]
        pub target_ids: Option<Vec<Id>>,
    }
    impl Message for SubmitAction {
        const TYPE: &'static str = "battle.submit_action";
    }

    /// S2C — a monster shouted a telegraphed ability and entered channeling;
    /// the client shows a flashing shout bubble and a charging sprite until
    /// `executes_at_tick` (Creature AI spec §3).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TelegraphStarted {
        pub battle_id: Id,
        pub combatant_id: Id,
        pub callout_text: String,
        pub executes_at_tick: i64,
    }
    impl Message for TelegraphStarted {
        const TYPE: &'static str = "battle.telegraph_started";
    }

    /// S2C — authoritative outcome of one resolved action.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ActionResolved {
        pub battle_id: Id,
        pub action_id: Option<Id>,
        pub actor_id: Id,
        pub action: BattleActionKind,
        pub auto: bool,
        pub flee_success: Option<bool>,
        /// Shout text for *instant* monster abilities (telegraphed ones already
        /// shouted via `battle.telegraph_started`). `None` for plain actions.
        #[serde(default)]
        pub callout_text: Option<String>,
        pub effects: Vec<Effect>,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Effect {
        pub target_id: Id,
        pub kind: EffectKind,
        pub amount: Option<i32>,
        pub status: Option<String>,
        pub hp_after: i32,
        /// How the target's damage_modifiers bent this effect
        /// (weak/resist/immune/absorb/normal). `None` when untyped.
        #[serde(default)]
        pub modifier_flag: Option<ModifierFlag>,
    }
    impl Message for ActionResolved {
        const TYPE: &'static str = "battle.action_resolved";
    }

    /// S2C — one party's combatants left an ongoing battle.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ParticipantLeft {
        pub battle_id: Id,
        pub combatant_ids: Vec<Id>,
        pub reason: String, // "fled" | "forced_flee"
    }
    impl Message for ParticipantLeft {
        const TYPE: &'static str = "battle.participant_left";
    }

    /// S2C — terminal battle resolution for the recipient's party.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Ended {
        pub battle_id: Id,
        pub outcome: BattleOutcome,
        pub xp_awards: Vec<XpAward>,
        pub loot: Vec<ItemStack>,
        /// Chits found by the recipient this encounter (economy.md S1). Banked on
        /// extraction, lost on death (it never entered circulation).
        #[serde(default)]
        pub chits_found: i64,
        /// Red-chest gear dropped to the recipient this encounter (deep fights only).
        #[serde(default)]
        pub gear_drops: Vec<LootGear>,
        pub class_emblem_drops: Vec<EmblemDrop>,
        pub gatekeeper_cleared: bool,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct XpAward {
        pub player_id: Id,
        pub xp: i64,
        pub run_level_after: i32,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct EmblemDrop {
        pub player_id: Id,
        pub emblem_kind: String,
    }
    impl Message for Ended {
        const TYPE: &'static str = "battle.ended";
    }
}

// -------------------------------------------------------------------- run ---

pub mod run {
    use super::*;

    /// C2S — start the party's run. Class selection is optional and back-compatible:
    /// `party` is the explicit per-hero composition from the party builder; if it is
    /// absent the server falls back to `character_class` as the party lead (building
    /// a default mixed party around it), and to Explorer if both are absent.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct EnterMaze {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub character_class: Option<crate::enums::CharacterClass>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub party: Option<Vec<crate::enums::CharacterClass>>,
        /// Per-slot hero names (persistent, per-account). Mirrors the player's saved
        /// roster; the server also reads/writes them via the `/v1/heroes` HTTP API.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub names: Option<Vec<String>>,
        /// Solo dive: a private instance for just the caller (no other humans).
        /// When absent/false, legacy behavior groups all waiting players (used by
        /// the headless bot tests); the co-op path is the `lobby.*` flow.
        #[serde(default)]
        pub solo: bool,
        /// Request the guided TUTORIAL world (Forest-first ordered biomes + a centred,
        /// obstacle-free area 0) for this dive. Opt-in: absent/false gives a normal
        /// randomized run, so a returning player isn't dropped into the same onboarding
        /// corridor every time. The hub offers it but never forces it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tutorial: Option<bool>,
    }
    impl Message for EnterMaze {
        const TYPE: &'static str = "run.enter_maze";
    }

    /// S2C — authoritative run/instance state at entry.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Started {
        pub client_seq: Option<u32>,
        pub run_id: Id,
        pub instance_id: Id,
        pub departure_hub_distance: i32,
        pub base_run_level: i32,
        pub members: Vec<Member>,
        pub backpack: Vec<ItemStack>,
        /// Chits carried in the run backpack at entry (always 0 — chits is found
        /// in the maze and banked on extraction, economy.md S1).
        #[serde(default)]
        pub chits: i64,
        /// Red-chest gear carried in the run backpack at entry (always empty at
        /// entry; grows as deep creatures drop loot).
        #[serde(default)]
        pub backpack_gear: Vec<LootGear>,
        /// Waypoints of the guaranteed clear path from the hub to the deep portal.
        /// The client draws this as a faint trail so the feasible route is legible.
        #[serde(default)]
        pub path: Vec<Position>,
        /// The WEB of extra trails (edges `(a, b)`) woven through the field — branches,
        /// loops and spurs off the backbone. The client draws these as trail dots too,
        /// so the overworld reads as an interconnected maze of routes, not one lane.
        #[serde(default)]
        pub web: Vec<(Position, Position)>,
        /// Walkable bounds — the client frames the map (edge cliffs/water + end
        /// walls) from these so it reads as a contained map, not an endless plain.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub bounds: Option<WorldBounds>,
        /// Biome-boundary chokepoints (a walled seam with one gap you pass through).
        #[serde(default)]
        pub seams: Vec<SeamView>,
        /// Per-run world-space offset into the shared terrain height field
        /// (`terrain::seed_offset`, hub-validated). The client feeds it to the ground
        /// shader + entity/camera Y so it renders the SAME hills/mesas the server placed
        /// content on — and so the world looks DIFFERENT every run (fixed function of
        /// position otherwise). `[0, 0]` = the un-shifted hand-tuned field.
        #[serde(default)]
        pub terrain_offset: [f32; 2],
        /// Authored CLIMBABLE landmark peaks (mountains), each `[cx, cz, radius, height]`
        /// in world space (see `terrain::peak_height`). The client sums them onto the
        /// ground so each mountain renders + you climb it; a boss or treasure sits on the
        /// summit. Streamed sections append more via `world.terrain_section`.
        #[serde(default)]
        pub peaks: Vec<[f32; 4]>,
    }
    /// Walkable extent of the instance (world-generation.md corridor bounds).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WorldBounds {
        pub x_min: f64,
        pub x_max: f64,
        /// Half-height of the corridor: walkable `y ∈ [-lateral, lateral]`.
        pub lateral: f64,
        /// Crossing west of this world-x returns you to Last City. The client draws
        /// the city's **wall + gate** here so you can see the boundary coming.
        #[serde(default)]
        pub west_return_border: f64,
        /// WG-4 radial fan arc (degrees); 0 = flat corridor. The content fans across
        /// this arc, leaving the western `360 - arc` sliver for Last City. The client
        /// uses it to place the city wall/gate as an ARC clipped to that western
        /// wedge (radius = `|west_return_border|`), instead of a straight wall that
        /// would spill across the fan's western content.
        #[serde(default)]
        pub radial_arc_degrees: f64,
    }
    /// One biome seam for the client to wall + gate.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SeamView {
        pub x: f64,
        pub gap_y: f64,
        pub gap_half_width: f64,
        pub biome_from: String,
        pub biome_to: String,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Member {
        pub player_id: Id,
        pub username: String,
        pub character_class: CharacterClass,
        pub spawn_position: Position,
    }
    impl Message for Started {
        const TYPE: &'static str = "run.started";
    }

    /// One of the caller's heroes, for the party/roster panel: persistent name,
    /// class, level, and the four attributes at that level. Stats live here (the
    /// inventory party screen) rather than cluttering the battle HUD.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct HeroView {
        pub slot: i32,
        pub name: String,
        pub class_key: String,
        pub level: i32,
        pub str_: i32,
        pub mnd: i32,
        pub dex: i32,
        pub wll: i32,
        pub max_hp: i32,
        /// Current HP this run (wounds persist across battles until healed).
        #[serde(default)]
        pub hp: i32,
        /// This run's total XP and the level curve's threshold to advance —
        /// level (like XP) is tracked per player, not per individual hero, so
        /// every hero on the same player's roster carries the same values.
        #[serde(default)]
        pub xp: i64,
        #[serde(default)]
        pub xp_to_next: i64,
        /// Formation rank: `true` = back row (halved damage, targeted less). The
        /// player sets this per hero on the party screen; defaults to the class
        /// default (casters back) until overridden. See [`SetFormation`].
        #[serde(default)]
        pub back_row: bool,
    }

    /// S2C — the caller's current party roster (sent at run start and refreshed on
    /// level-up), for the inventory party panel.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Party {
        pub heroes: Vec<HeroView>,
        /// AD-2: the class-pair synergies this composition has ACTIVE, and the
        /// sequenced combos it can perform. The build feedback loop — a player has
        /// to see what their comp enables to chase a better one. Additive; older
        /// clients simply don't render them.
        #[serde(default)]
        pub synergies: Vec<SynergyView>,
        #[serde(default)]
        pub combos: Vec<ComboView>,
    }

    /// One active class-pair synergy, described for the party screen.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SynergyView {
        pub key: String,
        pub name: String,
        pub description: String,
        /// The one-line mechanical effect ("every hero opens with 10 Barrier").
        pub effect: String,
    }

    /// One combo this comp can run, as setup -> payoff.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ComboView {
        pub key: String,
        pub name: String,
        /// e.g. "Snare (Explorer) then Backstab (Shifter)".
        pub sequence: String,
        pub description: String,
        /// Damage multiplier on the payoff, as a percentage bonus (60 = +60%).
        pub bonus_pct: i32,
    }
    impl Message for Party {
        const TYPE: &'static str = "run.party";
    }

    /// S2C — the caller's earned **overworld class perks** ("party sense"). The
    /// server computes these from the party's class composition × shared
    /// `run_level` against the `[perks]` balance thresholds, and re-sends on run
    /// start and every level-up (alongside `run.party`). Each field is 0/absent
    /// when the gating class isn't in the party. The client gates all client-side
    /// perk rendering (avatar glow, mob nameplates, minimap, battle ATB reveal) by
    /// these values; `resonant_regen`/`phoenix_guard_aggro_mult` are enforced
    /// server-side and mirrored here only for a HUD hint. See CANON class taxonomy.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Perks {
        /// Explorer avatar-light intensity factor (0 = no Explorer in party).
        #[serde(default)]
        pub explorer_glow: f32,
        /// Explorer minimap tier: 0 none · 1 map+mob/portal · 2 +chests · 3 +harvestables.
        /// The Explorers are the order that maps and reclaims the world
        /// (docs/lore/factions.md), so the map is theirs.
        #[serde(default)]
        pub explorer_map: u8,
        /// World-units the Explorer minimap covers (0 when no map).
        #[serde(default)]
        pub explorer_map_radius: f32,
        /// Hunter prey-sense tier: 0 none · 1 mob level · 2 +HP bar · 3 +battle ATB
        /// reveal. Knowing what you are hunting is the Hunters' guild's whole job.
        #[serde(default)]
        pub hunter_intel: u8,
        /// World-units within which a Shifter reveals DUNGEON entrances (0 = none).
        /// Shift-sense reads the instability that a door leaks.
        #[serde(default)]
        pub shifter_dungeon_radius: f32,
        /// Whether a Shifter can tell insured loot from ephemeral before picking it
        /// up — "check the weight", in the crew's own cant.
        #[serde(default)]
        pub shifter_item_sense: bool,
        /// Dungeon CELLS within which a Shifter reveals armed traps (0 = none). The
        /// class was already the best at disarming; this is what lets it find one
        /// before somebody stands on it.
        #[serde(default)]
        pub shifter_trap_radius: f32,
        /// Psyker threat tier: 0 none · 1 elites/gatekeepers · 2 +aggressive mobs.
        #[serde(default)]
        pub psyker_threat: u8,
        /// Extended mob interest radius the Psyker reveals (0 when no Psyker).
        #[serde(default)]
        pub psyker_reveal_radius: f32,
        /// Resonant overworld regen applied server-side, in HP/sec (display hint).
        #[serde(default)]
        pub resonant_regen: f32,
        /// Phoenix Guard skirmish/aggro radius multiplier (≤1; 1 = no Phoenix Guard).
        #[serde(default = "one_f32")]
        pub phoenix_guard_aggro_mult: f32,
    }
    fn one_f32() -> f32 {
        1.0
    }
    /// Neutral perks (no gating class in the party). Note `phoenix_guard_aggro_mult`
    /// defaults to 1.0 (no aggro reduction), NOT 0.0 — so a derived `Default`
    /// would be wrong; this is hand-written to match the serde `default`.
    impl Default for Perks {
        fn default() -> Self {
            Self {
                explorer_glow: 0.0,
                explorer_map: 0,
                explorer_map_radius: 0.0,
                hunter_intel: 0,
                shifter_dungeon_radius: 0.0,
                shifter_item_sense: false,
                shifter_trap_radius: 0.0,
                psyker_threat: 0,
                psyker_reveal_radius: 0.0,
                resonant_regen: 0.0,
                phoenix_guard_aggro_mult: 1.0,
            }
        }
    }
    impl Message for Perks {
        const TYPE: &'static str = "run.perks";
    }

    /// S2C — the party gained one or more levels this victory. Carries the
    /// before/after stats per hero so the client can play the classic JRPG
    /// "LEVEL UP!" stat-gain sequence. Sent alongside the refreshed `run.party`.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct LevelUp {
        pub new_run_level: i32,
        pub levels_gained: i32,
        pub heroes: Vec<HeroLevelUp>,
    }
    /// One hero's stat gains across a level-up (before → after).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct HeroLevelUp {
        pub slot: i32,
        pub name: String,
        pub class_key: String,
        pub level: i32,
        pub max_hp_before: i32,
        pub max_hp_after: i32,
        pub str_before: i32,
        pub str_after: i32,
        pub mnd_before: i32,
        pub mnd_after: i32,
        pub dex_before: i32,
        pub dex_after: i32,
        pub wll_before: i32,
        pub wll_after: i32,
    }
    impl Message for LevelUp {
        const TYPE: &'static str = "run.level_up";
    }

    /// S2C — one or more account-permanent unlocks just landed (roadmap `CL-1`).
    /// Sent the moment the milestone is met, not at extraction, so the reward
    /// arrives while the player still remembers what earned it. Also sent on
    /// connect with `banner: false` so the client knows what the account owns.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Unlocked {
        pub unlocks: Vec<UnlockView>,
        /// Everything the account owns now, so the party builder never has to
        /// accumulate deltas to know what it can field.
        pub owned: Vec<String>,
        pub party_slots: i32,
        /// Whether to announce these with the banner. False on the connect-time
        /// inventory: nobody wants four banners at login.
        pub banner: bool,
    }
    /// One unlock, described well enough for the banner and the locked row.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct UnlockView {
        pub key: String,
        pub name: String,
        /// `party_slot` or `class`.
        pub kind: String,
        /// The class key, when `kind == "class"`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub class_key: Option<String>,
        /// The slot number, when `kind == "party_slot"`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub slot: Option<i32>,
        pub trigger_text: String,
        pub banner: String,
    }
    impl Message for Unlocked {
        const TYPE: &'static str = "run.unlocked";
    }

    /// C2S — begin working a resource node the avatar is standing next to (MS-2).
    /// Opens a **channel** that yields one unit per tick until the node is empty, the
    /// player moves, a fight starts, or [`CancelHarvest`] arrives — so this starts a
    /// gather rather than completing one. The node's
    /// `material` banks into the backpack and its `skill` gains XP (world-gen.md).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Harvest {
        pub entity_id: Id,
    }
    impl Message for Harvest {
        const TYPE: &'static str = "run.harvest";
    }

    /// C2S — open a treasure chest the avatar is standing next to. Rolls loot
    /// (chits + materials + deep-enough red gear) into the backpack (economy.md S2).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct OpenChest {
        pub entity_id: Id,
    }
    impl Message for OpenChest {
        const TYPE: &'static str = "run.open_chest";
    }

    /// C2S — raise a field workstation where the avatar stands (MS-1). Costs ore from
    /// the run backpack and a Meld skill level in the trade, both checked server-side.
    /// Deliberate and explicit (a menu choice, not a hotkey) because it spends what you
    /// gathered — the same reasoning that put the Town Portal on the menu.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BuildStation {
        /// Which bench to raise (`smith`).
        pub kind: String,
    }
    impl Message for BuildStation {
        const TYPE: &'static str = "run.build_station";
    }

    /// C2S — ask the smith whose station this is to work a piece of YOUR gear. Anyone
    /// standing at a station may ask; the station owner's Forging level is the skill
    /// the job is done at, and they take the XP. **Ownership never moves**: the server
    /// only ever touches gear the requester already owns.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SmithRequest {
        /// The station being worked at (a `station:<kind>:<uses>` snapshot entity).
        pub entity_id: Id,
        /// The requester's own Vault gear.
        pub gear_id: Id,
        /// `reroll`, `repair` or `enhance` (a temporary edge that dies with the run).
        pub service: String,
        /// Material to spend on a reroll (ignored by a repair).
        #[serde(default)]
        pub material: String,
        /// The recipe a **brew** cooks, at a Keeper's alembic. Ignored by the smith's
        /// services, and `gear_id` is ignored by a brew — a pot has no piece in it.
        #[serde(default)]
        pub recipe: String,
    }
    impl Message for SmithRequest {
        const TYPE: &'static str = "run.smith_request";
    }

    /// S2C — the heat is open: strike on the yellow. The bar is **red** and each blow
    /// has one **yellow** band on it; the marker sweeps the bar in `sweep_ms`. The server
    /// laid this out (from a seed it picked) and it is the only thing that grades a blow —
    /// a client renders the bar, it does not decide what happened on it.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TempoStarted {
        pub job_id: Id,
        /// What is being worked, for the panel's own words.
        pub service: String,
        /// How many blows the piece takes.
        pub strikes: i32,
        /// One full pass of the marker, in milliseconds.
        pub sweep_ms: i64,
        /// The yellow, one band per blow, as fractions of the bar (`0.0`–`1.0`).
        pub bands: Vec<TempoBand>,
    }
    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct TempoBand {
        pub lo: f64,
        pub hi: f64,
    }
    impl Message for TempoStarted {
        const TYPE: &'static str = "run.tempo_started";
    }

    /// C2S — a blow, at the marker's position on the bar when the player struck.
    /// Out-of-range values are clamped, and blows past the last one are ignored: spam
    /// can neither raise nor lower a heat's quality.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Strike {
        pub job_id: Id,
        /// Where the marker was, as a fraction of the bar.
        pub at: f64,
    }
    impl Message for Strike {
        const TYPE: &'static str = "run.strike";
    }

    /// S2C — what the smith did, or why they would not: one line, already written for
    /// the player, plus the station's remaining jobs so the prompt can count down.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SmithResult {
        pub player_id: Id,
        pub entity_id: Id,
        pub gear_id: Id,
        pub service: String,
        pub ok: bool,
        pub message: String,
        pub uses_left: i32,
        /// The heat's quality, `0.0`–`1.0` — the blows that landed on yellow. What it
        /// bought depends on the service (the affix pool, the points restored, the size
        /// of a temporary edge).
        #[serde(default)]
        pub quality: f64,
    }
    impl Message for SmithResult {
        const TYPE: &'static str = "run.smith_result";
    }

    /// C2S — descend into a hand-designed dungeon whose entrance (`entity_id`, an
    /// `entrance:<dungeon>` snapshot entity) the avatar is standing next to
    /// (WG-1/DG-3b). A committed space: you leave by the exit or by dying, never a
    /// Town Portal. Deliberate (a keypress), never automatic on walking past.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct EnterDungeon {
        pub entity_id: Id,
    }
    impl Message for EnterDungeon {
        const TYPE: &'static str = "run.enter_dungeon";
    }

    /// C2S — opt into the fight already in progress nearby (the avatar must be
    /// within join range of the battle). The whole of the caller's party joins the
    /// existing side; teammates are never auto-pulled in.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct JoinBattle {}
    impl Message for JoinBattle {
        const TYPE: &'static str = "run.join_battle";
    }

    /// C2S — rename one of the caller's heroes (persistent, per-account). Takes
    /// effect immediately (the roster is re-sent) and is saved to the account.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct RenameHero {
        pub slot: i32,
        pub name: String,
    }
    impl Message for RenameHero {
        const TYPE: &'static str = "run.rename_hero";
    }

    /// C2S — set one of the caller's heroes to the front (`back_row=false`) or back
    /// (`back_row=true`) row. Persistent per-account, like [`RenameHero`]: takes
    /// effect immediately (the roster is re-sent) and applies to the next/active
    /// battle's Fighter, overriding the class default.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SetFormation {
        pub slot: i32,
        pub back_row: bool,
    }
    impl Message for SetFormation {
        const TYPE: &'static str = "run.set_formation";
    }

    /// C2S — start an extraction channel. `method` is `"portal"` (stand at the
    /// single deep portal) or `"town_portal"` (consume a Town Portal item, works
    /// anywhere — the primary way out).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BeginExtraction {
        pub method: String, // "portal" | "town_portal"
        #[serde(default)]
        pub portal_entity_id: Option<Id>,
        #[serde(default)]
        pub item_id: Option<Id>,
    }
    impl Message for BeginExtraction {
        const TYPE: &'static str = "run.begin_extraction";
    }

    /// C2S — drink a potion on the overworld, out of combat. Same item menu, same
    /// backpack, same registry as the battle Item command — you just don't have to
    /// be bleeding in front of a monster to use one.
    ///
    /// Only the potions whose effect OUTLIVES a fight work here: heals, a full heal,
    /// a revive, an Insight Mote. Barrier/Regen/Evasion/Adrenaline are battle state
    /// that would evaporate before the next encounter, so the server refuses them
    /// rather than letting a player waste the bottle.
    ///
    /// The client sends only *what* and *on whom*; the server owns the magnitude,
    /// the stock check, and whether the effect applies at all.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct UseItem {
        pub item_kind: String,
        /// Which of the caller's heroes drinks it (0-based party slot).
        pub hero_slot: i32,
    }
    impl Message for UseItem {
        const TYPE: &'static str = "run.use_item";
    }

    /// C2S — stop an in-progress harvest channel on purpose (the "click away"
    /// gesture). Movement stops one too; this is for putting the tool down while
    /// standing still.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CancelHarvest {}
    impl Message for CancelHarvest {
        const TYPE: &'static str = "run.cancel_harvest";
    }

    /// S2C — a channel began (interruptible; visible to the whole instance). Covers
    /// extraction *and* harvesting; `method` distinguishes them.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ChannelStarted {
        pub client_seq: Option<u32>,
        pub player_id: Id,
        pub method: String,
        pub completes_at: u64,
        /// Milliseconds per **fill** — how long the client's progress bar takes to go
        /// from empty to full once. Extraction fills once and completes; a harvest
        /// repeats it, paying a unit each time, until `completes_at` (or an interrupt).
        /// `0` = unknown, draw no bar.
        #[serde(default)]
        pub fill_ms: u64,
    }
    impl Message for ChannelStarted {
        const TYPE: &'static str = "run.channel_started";
    }

    /// S2C — an extraction channel broke before completing.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ChannelInterrupted {
        pub player_id: Id,
        pub reason: String, // damage_taken | battle_started | moved | cancelled | disconnected
    }
    impl Message for ChannelInterrupted {
        const TYPE: &'static str = "run.channel_interrupted";
    }

    /// S2C — a member's run reached a terminal state.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MemberResult {
        pub run_id: Id,
        pub player_id: Id,
        pub result: RunResult,
        pub max_distance_reached: i32,
        pub banked: Option<Vec<ItemStack>>,
        pub lost: Option<Vec<ItemStack>>,
        /// Chits banked (on `extracted`) or forfeited (on `died`/`abandoned`) with
        /// this run. Minted into the persistent economy only on extraction.
        #[serde(default)]
        pub chits: i64,
        /// Red-chest gear banked into the Vault on extraction (empty on death).
        #[serde(default)]
        pub gear_banked: Vec<LootGear>,
        pub durability_loss_applied: bool,
    }
    impl Message for MemberResult {
        const TYPE: &'static str = "run.member_result";
    }

    /// S2C — authoritative delta to the recipient's own backpack.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BackpackUpdate {
        pub changes: Vec<BackpackChange>,
        /// Signed change to the run's chits total (economy.md S1). Positive on a
        /// loot drop, negative when chits leaves the backpack (banked/dropped).
        #[serde(default)]
        pub chits_delta: i64,
        /// Red-chest gear added to the backpack by this update (loot drops).
        #[serde(default)]
        pub gear_added: Vec<LootGear>,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BackpackChange {
        pub item: ItemStack,
        pub delta: String, // "added" | "removed"
        pub cause: String,
    }
    impl Message for BackpackUpdate {
        const TYPE: &'static str = "run.backpack_update";
    }

    /// C2S — equip (or unequip) a piece of this run's not-yet-banked loot gear
    /// onto one of the caller's hero slots. Unlike Vault equip (HTTP,
    /// persistent, effective from the next dive), this is run-scoped: it
    /// applies immediately to the caller's remaining battles this run, and —
    /// like the rest of the backpack — is lost on death; only a successful
    /// extraction banks it (already equipped, if worn) into the Vault.
    /// `hero_slot: None` unequips.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct EquipLoot {
        pub gear_id: Id,
        #[serde(default)]
        pub hero_slot: Option<i32>,
    }
    impl Message for EquipLoot {
        const TYPE: &'static str = "run.equip_loot";
    }

    /// S2C — authoritative snapshot of the recipient's current run-loot gear
    /// (found this run, not yet banked): sent whenever it changes (new loot,
    /// an equip/unequip) so the Equip tab always reflects the truth.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct RunGear {
        pub gear: Vec<LootGear>,
    }
    impl Message for RunGear {
        const TYPE: &'static str = "run.gear";
    }
}

// -------------------------------------------------------------------- lobby ---

/// Pre-maze co-op lobby: create/join a party by code, ready up, and the host
/// starts a shared dive (everyone lands in one instance). Solo play skips this
/// entirely via `run.enter_maze { solo: true }`.
pub mod lobby {
    use super::*;

    /// C2S — create a new lobby (caller becomes host + first member).
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Create {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub party: Option<Vec<CharacterClass>>,
    }
    impl Message for Create {
        const TYPE: &'static str = "lobby.create";
    }

    /// C2S — join an existing lobby by its code.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Join {
        pub code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub party: Option<Vec<CharacterClass>>,
    }
    impl Message for Join {
        const TYPE: &'static str = "lobby.join";
    }

    /// C2S — toggle the caller's ready flag.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Ready {
        pub ready: bool,
    }
    impl Message for Ready {
        const TYPE: &'static str = "lobby.ready";
    }

    /// C2S — leave the current lobby.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Leave {}
    impl Message for Leave {
        const TYPE: &'static str = "lobby.leave";
    }

    /// C2S — host only: launch the dive with all (ready) members.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Start {}
    impl Message for Start {
        const TYPE: &'static str = "lobby.start";
    }

    /// One member in a lobby (S2C view).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MemberView {
        pub player_id: Id,
        pub username: String,
        pub party: Vec<CharacterClass>,
        pub ready: bool,
    }

    /// S2C — authoritative lobby state, broadcast to all members on any change.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct State {
        pub code: String,
        pub host_player_id: Id,
        pub members: Vec<MemberView>,
    }
    impl Message for State {
        const TYPE: &'static str = "lobby.state";
    }

    /// S2C — the lobby was disbanded (host left / everyone gone).
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Closed {}
    impl Message for Closed {
        const TYPE: &'static str = "lobby.closed";
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Envelope;

    #[test]
    fn authenticate_type_string_matches_canon() {
        assert_eq!(session::Authenticate::TYPE, "session.authenticate");
        assert_eq!(battle::SubmitAction::TYPE, "battle.submit_action");
        assert_eq!(run::EnterMaze::TYPE, "run.enter_maze");
    }

    // The field-station pair, on the wire: a build is a kind, a request names the
    // station AND the requester's own gear (never anyone else's), and the reply is a
    // sentence plus the jobs the station has left.
    #[test]
    fn the_field_station_messages_round_trip() {
        assert_eq!(run::BuildStation::TYPE, "run.build_station");
        assert_eq!(run::SmithRequest::TYPE, "run.smith_request");
        assert_eq!(run::SmithResult::TYPE, "run.smith_result");

        let json = r#"{"type":"run.smith_request","seq":7,"ts":1,"payload":{"entity_id":"station-smith-0","gear_id":"0195d001-aaaa-7abc-8f01-23456789abcd","service":"reroll","material":"dune_ingot"}}"#;
        let env: Envelope<run::SmithRequest> = serde_json::from_str(json).unwrap();
        assert_eq!(env.payload.entity_id, "station-smith-0");
        assert_eq!(env.payload.service, "reroll");
        let back: run::SmithRequest =
            serde_json::from_str(&serde_json::to_string(&env.payload).unwrap()).unwrap();
        assert_eq!(back.material, "dune_ingot");

        // A repair carries no material, so the field is optional on the wire.
        let repair: run::SmithRequest = serde_json::from_str(
            r#"{"entity_id":"station-smith-0","gear_id":"g","service":"repair"}"#,
        )
        .unwrap();
        assert!(repair.material.is_empty());

        let reply: run::SmithResult = serde_json::from_str(
            r#"{"player_id":"p","entity_id":"station-smith-0","gear_id":"g","service":"repair","ok":true,"message":"mended +6 for 24c","uses_left":3}"#,
        )
        .unwrap();
        assert!(reply.ok && reply.uses_left == 3);
        assert_eq!(reply.quality, 0.0, "an old reply without a quality still parses");

        // The heat: the server hands over the bar, the client hands back blows.
        assert_eq!(run::TempoStarted::TYPE, "run.tempo_started");
        assert_eq!(run::Strike::TYPE, "run.strike");
        let heat: run::TempoStarted = serde_json::from_str(
            r#"{"job_id":"j1","service":"reroll","strikes":2,"sweep_ms":1400,"bands":[{"lo":0.1,"hi":0.4},{"lo":0.5,"hi":0.8}]}"#,
        )
        .unwrap();
        assert_eq!(heat.bands.len(), 2);
        assert!((heat.bands[1].hi - 0.8).abs() < 1e-9);
        let blow: run::Strike =
            serde_json::from_str(r#"{"job_id":"j1","at":0.25}"#).unwrap();
        assert!((blow.at - 0.25).abs() < 1e-9);
    }

    #[test]
    fn terrain_section_round_trips() {
        let json = r#"{"type":"world.terrain_section","seq":5,"ts":1,"payload":{"index":2,"start_x":40.0,"end_x":72.0,"y_min":-28.0,"cell":2.0,"cols":16,"rows":28,"levels":[0,1,1],"connectors":[{"kind":"ladder","position":{"x":50.0,"y":-6.0},"lo":0,"hi":1,"radius":2.2}],"path":[{"x":40.0,"y":0.0},{"x":72.0,"y":3.0}]}}"#;
        let env: Envelope<world::TerrainSection> = serde_json::from_str(json).unwrap();
        assert_eq!(env.payload.index, 2);
        assert_eq!(env.payload.connectors[0].kind, "ladder");
        assert_eq!(env.payload.levels, vec![0, 1, 1]);
        // Round-trips back out.
        let s = serde_json::to_string(&env.payload).unwrap();
        let back: world::TerrainSection = serde_json::from_str(&s).unwrap();
        assert_eq!(back.cols, 16);
        assert_eq!(back.connectors.len(), 1);
    }

    #[test]
    fn dungeon_scene_round_trips() {
        assert_eq!(world::DungeonScene::TYPE, "world.dungeon_scene");
        let scene = world::DungeonScene {
            active: true,
            theme: "forest".to_string(),
            floor: 1,
            width: 24,
            height: 18,
        };
        let s = serde_json::to_string(&scene).unwrap();
        let back: world::DungeonScene = serde_json::from_str(&s).unwrap();
        assert_eq!(back, scene);
        // Exit form: minimal wire (only `active`) still decodes, theme defaults empty.
        let exit: world::DungeonScene =
            serde_json::from_str(r#"{"active":false}"#).unwrap();
        assert!(!exit.active);
        assert_eq!(exit.theme, "");
    }

    #[test]
    fn snapshot_entity_level_is_optional_and_defaults() {
        // Old wire (no `level`) still decodes; absent → None.
        let json = r#"{"entity_id":"m","position":{"x":1.0,"y":2.0},"velocity":{"x":0.0,"y":0.0},"avatar_state":"active"}"#;
        let e: movement::SnapshotEntity = serde_json::from_str(json).unwrap();
        assert_eq!(e.level, None);
    }

    #[test]
    fn snapshot_entity_mob_intel_is_optional_and_round_trips() {
        // Old wire (no intel fields) still decodes; all absent → None.
        let json = r#"{"entity_id":"m","position":{"x":1.0,"y":2.0},"velocity":{"x":0.0,"y":0.0},"avatar_state":"mob:dune_wyrm:beasts"}"#;
        let e: movement::SnapshotEntity = serde_json::from_str(json).unwrap();
        assert_eq!(e.mob_level, None);
        assert_eq!(e.hp, None);
        assert_eq!(e.encounter_class, None);
        // A fully-populated mob round-trips.
        let full = movement::SnapshotEntity {
            entity_id: "m".into(),
            position: Position { x: 1.0, y: 2.0 },
            velocity: movement::Velocity { x: 0.0, y: 0.0 },
            avatar_state: Some("mob:dune_wyrm:beasts".into()),
            level: Some(1),
            mob_level: Some(7),
            hp: Some(30),
            max_hp: Some(52),
            encounter_class: Some("elite".into()),
            aggression: Some("aggressive".into()),
        };
        let s = serde_json::to_string(&full).unwrap();
        let back: movement::SnapshotEntity = serde_json::from_str(&s).unwrap();
        assert_eq!(back.mob_level, Some(7));
        assert_eq!(back.hp, Some(30));
        assert_eq!(back.max_hp, Some(52));
        assert_eq!(back.encounter_class.as_deref(), Some("elite"));
        assert_eq!(back.aggression.as_deref(), Some("aggressive"));
    }

    #[test]
    fn perks_round_trips_and_defaults_sanely() {
        assert_eq!(run::Perks::TYPE, "run.perks");
        // Old/empty wire: aggro mult defaults to 1.0 (no Phoenix Guard), rest to 0.
        let empty: run::Perks = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.phoenix_guard_aggro_mult, 1.0);
        assert_eq!(empty.hunter_intel, 0);
        assert_eq!(empty.explorer_map, 0);
        assert!(!empty.shifter_item_sense);
        let env_json = r#"{"type":"run.perks","seq":9,"ts":1,"payload":{"explorer_glow":2.5,"hunter_intel":3,"explorer_map":2,"explorer_map_radius":40.0,"shifter_dungeon_radius":55.0,"shifter_item_sense":true,"shifter_trap_radius":4.5,"psyker_threat":1,"psyker_reveal_radius":30.0,"resonant_regen":1.5,"phoenix_guard_aggro_mult":0.6}}"#;
        let env: Envelope<run::Perks> = serde_json::from_str(env_json).unwrap();
        assert_eq!(env.payload.hunter_intel, 3);
        assert_eq!(env.payload.shifter_dungeon_radius, 55.0);
        assert!(env.payload.shifter_item_sense);
        assert_eq!(env.payload.shifter_trap_radius, 4.5);
        assert_eq!(env.payload.phoenix_guard_aggro_mult, 0.6);
        let s = serde_json::to_string(&env.payload).unwrap();
        let back: run::Perks = serde_json::from_str(&s).unwrap();
        assert_eq!(back.explorer_map, 2);
        assert_eq!(back.psyker_reveal_radius, 30.0);
    }

    #[test]
    fn submit_action_round_trips_against_spec_example() {
        // battle.md example, wrapped in the envelope.
        let json = r#"{"type":"battle.submit_action","seq":310,"ts":1783728115000,"payload":{"battle_id":"b","action_id":"a","action":"attack","skill_kind":null,"item_id":null,"target_ids":["t"]}}"#;
        let env: Envelope<battle::SubmitAction> = serde_json::from_str(json).unwrap();
        assert_eq!(env.payload.action, BattleActionKind::Attack);
        assert_eq!(env.payload.target_ids.as_ref().unwrap()[0], "t");
    }
}
