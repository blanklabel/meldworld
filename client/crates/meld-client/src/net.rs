//! The client's network layer: HTTP for the API, a WebSocket for realtime.
//!
//! Poll-based, single-threaded: [`Net`] holds an internal state machine advanced
//! by [`Net::poll`] once per frame. Auth HTTP goes through `ehttp`, the realtime
//! socket through `ewebsock` — neither needs tokio or OS threads, so the exact
//! one code path, native only (the browser client is gone).
//!
//! Bevy holds `Net` as a NonSend resource; commands go in via [`Net::send`],
//! server events come out via [`Net::poll`] + [`Net::try_recv`]. Message
//! sequence mirrors the proven bot harness.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc;

use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use meld_proto::common::Combatant;
use meld_proto::realtime::{
    battle as wb, lobby as wl, movement as wm, onboarding as wo, run as wr, session as ws,
    world as ww, Message as _,
};
use meld_proto::RawEnvelope;
use serde_json::{json, Value};

pub const GUEST_PASSWORD: &str = "meld-guest-password";

/// Commands sent from Bevy into the network layer.
pub enum ClientCmd {
    Connect { username: String, password: String },
    /// Enter the maze with the built party (one class key per hero slot).
    EnterMaze { party: Vec<String>, tutorial: bool, hub: Option<String> },
    Move { dx: f64, dy: f64 },
    /// Battle commands. `actor` is which of the player's heroes acts; `target` is the
    /// chosen combatant (an enemy for Attack/offensive Skill, an ally for a
    /// heal/support Skill or Item). Defend is self-cast (no target).
    Attack { battle_id: String, actor: String, target: String },
    Defend { battle_id: String, actor: String },
    Skill { battle_id: String, actor: String, target: String, skill_kind: String },
    Item { battle_id: String, actor: String, item_id: String, target: String },
    /// Flee the battle (self-cast on the acting hero). A successful flee ends the
    /// whole encounter and returns the party to the overworld — but the server
    /// charges a toll (dropped chits + a chance to lose non-permanent items), so
    /// it's a real escape decision, not a free reset (combat-atb.md).
    Flee { battle_id: String, actor: String },
    /// Drink a potion out of combat, from the overworld menu's Items column. Only
    /// the effects that outlive a fight work; the server refuses the rest.
    UseItem { item_kind: String, hero_slot: i32 },
    /// Move an item between the Party Inventory and one hero's pouch. Overworld only —
    /// the server refuses it mid-battle, so a fight is fought with what was packed.
    MoveItem { item_kind: String, hero_slot: i32, to_pouch: bool },
    /// Begin an extraction channel at the single deep fixed portal.
    Extract,
    /// Consume a Town Portal item to extract from anywhere (the primary way out).
    TownPortal,
    /// Begin working a resource node the avatar is standing next to — a channel that
    /// drips one unit per tick until stopped (MS-2).
    Harvest { entity_id: String },
    PsykerHold { entity_id: String },
    /// Put the tool down on purpose, keeping every unit already banked.
    CancelHarvest,
    /// Open a treasure chest the avatar is standing next to.
    OpenChest { entity_id: String },
    /// Descend into a hand-designed dungeon whose entrance the avatar is next to.
    EnterDungeon { entity_id: String },
    /// Raise a field workstation where the avatar stands (spends ore you carry).
    BuildStation { kind: String },
    /// Raise a `Structure` (CANON D21/§W3). One command for every function, because there
    /// is one primitive — the key comes from `meld_proto::structures`.
    BuildStructure { function: String },
    /// BD-9: build at a chosen spot with a facing — what one piece of a dragged run is.
    BuildStructureAt { function: String, at: (f64, f64), yaw: f64 },
    RepairStructure { entity_id: String },
    DemolishStructure { entity_id: String },
    /// Ask whoever raised this station to do a piece of work for you: the smith's
    /// services on YOUR OWN gear, or a brew at a Keeper's alembic.
    SmithRequest {
        entity_id: String,
        gear_id: String,
        service: String,
        material: String,
        recipe: String,
    },
    /// A blow on the smithing bar, at the marker's position (0.0-1.0) when struck.
    Strike { job_id: String, at: f64 },
    /// Pack up a bench you raised (its own channel; hands back part of the stock).
    TeardownStation { entity_id: String },
    /// Opt into the ongoing fight nearby (the server checks proximity).
    JoinBattle,
    /// WATCH the nearest fight in reach without entering it (`SOC-3`) — another player's
    /// battle, or two mobs tearing at each other (`CR-2`).
    WatchBattle,
    /// Stop watching. Idempotent server-side, so the same key can toggle.
    StopWatching,
    /// Rename one of the caller's heroes (persistent, per-account).
    RenameHero { slot: i32, name: String },
    /// Set a hero to the front (`false`) or back (`true`) row (persistent).
    SetFormation { slot: i32, back_row: bool },
    /// Equip (or unequip with `hero_slot: None`) a piece of this run's
    /// not-yet-banked loot gear onto a hero slot — run-scoped, takes effect
    /// immediately (unlike the Vault's HTTP equip, which is next-dive-only).
    EquipLoot { gear_id: String, hero_slot: Option<i32> },
    /// Co-op lobby.
    LobbyCreate { party: Vec<String> },
    LobbyJoin { code: String, party: Vec<String> },
    LobbyReady { ready: bool },
    LobbyStart,
    LobbyLeave,
    /// Dismiss the town welcome tour (finished OR skipped) — persisted per-account.
    OnboardingTownSeen,
    /// Dismiss the first-dive briefing — persisted per-account.
    OnboardingRunSeen,
}

/// A render-ready combatant view for the battle screen.
#[derive(Clone)]
pub struct CombatantView {
    pub id: String,
    pub name: String,
    pub hp: i32,
    pub max_hp: i32,
    pub gauge: f64,
    pub is_player: bool,
    /// Owning player for a hero combatant (`None` for monsters). Allied heroes are
    /// grouped by this into per-party strips on the battle screen edges.
    pub player_id: Option<String>,
    pub level: i32,
    /// Wire statuses — for a Psyker these carry Focus state (`focus_slots:N`,
    /// `focus:<kind>:<stacks>`) that drives the focus UI.
    pub statuses: Vec<String>,
}

impl CombatantView {
    fn from_wire(c: &Combatant) -> Self {
        let name = match (&c.player_id, &c.monster_kind) {
            // Heroes carry their (persistent, per-account) name on `name:<name>`.
            (Some(_), _) => c
                .statuses
                .iter()
                .find_map(|s| s.strip_prefix("name:"))
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Hero".to_string()),
            (_, Some(k)) => k.replace('_', " "),
            _ => "?".to_string(),
        };
        CombatantView {
            id: c.combatant_id.clone(),
            name,
            hp: c.hp,
            max_hp: c.max_hp,
            gauge: c.gauge,
            is_player: c.player_id.is_some(),
            player_id: c.player_id.clone(),
            level: c.level,
            statuses: c.statuses.clone(),
        }
    }
}

/// What an overworld entity is (decides how the client draws it).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Player,
    Monster,
    Portal,
    /// A harvestable resource node (`monster_kind` carries its content id/label).
    Resource,
    /// Ground loot from a creature skirmish (`monster_kind` carries the item kind).
    /// Walk over it to auto-collect.
    Loot,
    /// An impassable terrain feature (`monster_kind` carries its kind, `radius` its size).
    Obstacle,
    /// A treasure chest (`opened` tells the client to draw it opened vs closed).
    Chest,
    /// A player-built `Structure` (CANON D21/§W3). `monster_kind` carries its `function`
    /// key, `bodies_required` its whole-percent HP — ONE kind for every function, so a new
    /// one needs no new render path and cannot be forgotten by one.
    Structure,
    /// A hand-designed dungeon entrance (`monster_kind` carries the dungeon name).
    /// Walk up and press F to descend (`run.enter_dungeon`).
    Entrance,
    /// A dungeon staircase — the way to the next floor.
    Stair,
    /// A player-raised field workstation (`monster_kind` carries its kind, `level`
    /// its elevation, `bodies_required` the jobs it has left). Press [E] to work at it.
    Station,
    /// An ARMED dungeon trap a Shifter has read (`monster_kind` carries its kind).
    /// Only ever sent when the party's Shift-sense reaches it — the server decides
    /// what is visible, so an unaccompanied party genuinely cannot see these.
    Trap,
}

/// A dynamic overworld entity.
#[derive(Clone)]
pub struct EntityView {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub kind: EntityKind,
    /// Creature content id for monsters, or the terrain kind for obstacles.
    pub monster_kind: Option<String>,
    /// Creature faction for monsters (drives colour); `None` otherwise.
    pub faction: Option<String>,
    /// World-unit radius for obstacles; `0.0` otherwise.
    pub radius: f64,
    /// True if this is a player currently in a fight (`avatar_state == in_battle`).
    pub battling: bool,
    /// CR-2: this creature is trading blows with another right now. An EVENT, not
    /// intel — so it is marked for everyone, with no perk gating.
    pub clashing: bool,
    /// Elevation level (terraced verticality) — the render height is raised by
    /// `level × step_height`. Absent on the wire → 0 (ground).
    pub level: u8,
    /// For chests: whether it's already been opened.
    pub opened: bool,
    /// For chests: the treasure tier off `chest:<tier>:<open>`, which is what decides
    /// how good the loot inside is — and so what the chest should LOOK like. It rode the
    /// wire from the day chests existed and the client threw it away, drawing every
    /// chest as the common brown one.
    pub chest_tier: i32,
    /// Overworld mob intel (Explorer/Psyker perks). `None` for non-mobs. The client
    /// shows each field only when the viewer's perk unlocks it (see `Perks`).
    pub mob_level: Option<i32>,
    pub hp: Option<i32>,
    pub max_hp: Option<i32>,
    /// `standard` | `elite` | `gatekeeper` — drives the Psyker threat marker.
    pub encounter_class: Option<String>,
    /// `passive` | `territorial` | `aggressive`.
    pub aggression: Option<String>,
    /// This creature is the quarry of a hunt the viewer is working (AD-4). Server-decided
    /// and per-viewer: the same creature is not a quarry to the teammate beside them.
    pub quarry: bool,
    /// This creature is PINNED by a Psyker right now (CL-2): it cannot move, and a fight
    /// begun against it opens with the whole party's gauges full.
    pub held: bool,
    /// FS-4: the named boss this creature IS, if it is one (`gloamhound`, `ironmaw`, …).
    /// A boss overlays a host creature, so `monster_kind` is the wildlife it rode in on
    /// and this is what it actually fights and renders as. `None` for ordinary fauna.
    pub boss: Option<String>,
    /// For dungeon entrances: how many heroes the doors inside want standing on
    /// plates at once. 1 for anything a lone player can finish.
    pub bodies_required: u8,
    /// How many PARTIES this fight is sized for (`FS-4`), when that is more than one.
    /// 0 on everything ordinary — a label on a normal creature is a label players learn
    /// to ignore, and then the raid ones stop working.
    pub expects_parties: u8,
}

/// One saved party composition (PT-2).
#[derive(Clone, Debug)]
pub struct LoadoutLine {
    pub name: String,
    pub classes: Vec<String>,
}

/// A connector (ladder/rope/slope) joining two elevation levels — client view.
#[derive(Clone)]
pub struct ConnectorView {
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub lo: u8,
    pub hi: u8,
    pub radius: f64,
}

/// One streamed overworld **section**'s static geometry: its elevation grid +
/// connectors (+ trail contribution). The client builds one stepped ground+cliff
/// mesh per section and spawns the connector props.
#[derive(Clone)]
pub struct TerrainSectionView {
    pub index: u32,
    pub start_x: f64,
    pub end_x: f64,
    pub y_min: f64,
    pub cell: f64,
    pub cols: u32,
    pub rows: u32,
    pub levels: Vec<u8>,
    pub connectors: Vec<ConnectorView>,
    pub path: Vec<(f64, f64)>,
    /// The section's biome theme, so the client keys ground + HUD off the actual
    /// per-section biome (radius ring) rather than fixed distance bands.
    pub biome: String,
    /// WG-4 radial fan: half the arc in radians (0 ⇒ flat). The elevation grid is in
    /// un-bent corridor coords; the client bends terrace/cliff/connector geometry by
    /// this arc so raised ground lines up with the (server-bent) positions it walks on.
    pub radial_half: f64,
    /// Corridor half-extent the arc maps against (pairs with `radial_half`).
    pub corridor_lateral: f64,
    /// Authored CLIMBABLE peaks this streamed section adds (`[cx, cz, radius, height]`).
    pub peaks: Vec<[f32; 4]>,
    /// **CONTINENTS (WG-7):** the STRAITS this section holds — the inland seas that separate
    /// one landmass from the next ([`meld_proto::coast::Strait`]). A re-sent section replaces
    /// its own, exactly as it replaces its own peaks.
    pub straits: Vec<meld_proto::coast::Strait>,
    /// The coast's own shape: bays and isles ([`meld_proto::coast::Lobe`]).
    pub lobes: Vec<meld_proto::coast::Lobe>,
    /// Inland water: standing bodies and river chains.
    pub basins: Vec<meld_proto::coast::Basin>,
    pub rivers: Vec<meld_proto::coast::RiverNode>,
}

/// One resolved effect for hit feedback (a damage or heal on a combatant).
pub struct HitEffect {
    pub target: String,
    pub kind: String,
    pub amount: Option<i32>,
    pub hp_after: i32,
    /// A critical hit — the client pops it bigger + gold ("CRIT!").
    pub crit: bool,
    /// Elemental modifier flag ("weak"/"resist"/"immune"/"absorb"/"normal") —
    /// drives the WEAK!/RESIST!/IMMUNE!/ABSORB! feedback (Psyker-gated).
    pub modifier: Option<String>,
}

/// A biome seam (chokepoint) for the client to wall + gate.
#[derive(Clone)]
pub struct SeamLine {
    pub x: f64,
    pub gap_y: f64,
    pub gap_half_width: f64,
    pub biome_from: String,
    pub biome_to: String,
}

/// A gear row for the inventory screen.
#[derive(Clone)]
pub struct GearLine {
    pub gear_id: String,
    pub name: String,
    pub slot: String,
    /// Which class this item is for (`meld_world::CLASS_KEYS`); empty means
    /// unrestricted (e.g. the starter weapon).
    pub class_key: String,
    /// `"insured"` or `"ephemeral"` (the chest colours `blue`/`red` still parse).
    pub insurance: String,
    /// GR-5 weapon family wire word; empty = unrestricted.
    pub family: String,
    /// GR-5 armor weight wire word; empty = unrestricted.
    pub armor_weight: String,
    /// AD-1 rolled affixes (already described by `meld_proto::affixes`).
    pub affixes: Vec<meld_proto::affixes::Affix>,
    /// AD-1 unique key; empty for ordinary loot.
    pub unique_key: String,
    /// AD-1 set key; empty when not part of a set.
    pub set_key: String,
    pub tier: i32,
    /// Which of the caller's heroes has this equipped, if any.
    pub equipped_hero_slot: Option<usize>,
    pub max_durability: i32,
    pub base_max_durability: i32,
    pub atk_bonus: i32,
    pub def_bonus: i32,
    pub spd_bonus: i32,
    /// Materials one affix reroll on this piece would eat — the server's number,
    /// scaled to the piece's tier (`[forge] reroll_material_per_tier`). 0 on run
    /// loot, which is not in the Vault for a smith to work on.
    pub reroll_cost: i32,
}

/// One row on a town vendor's shelf (EC-2) — the server prices and describes it.
#[derive(Clone, Debug, Default)]
pub struct ShopLine {
    pub item_kind: String,
    pub name: String,
    pub description: String,
    pub price_chits: i64,
}

/// What the Broker pays for one material, at the caller's Mercantile level (MS-1).
#[derive(Clone, Debug, Default)]
pub struct BrokerQuote {
    pub item_kind: String,
    pub name: String,
    pub price_chits: i64,
}

/// One craftable recipe as the Forge & Alembic lists it (MS-1). The server owns the
/// level gate and the input list, so the panel never holds a second copy of the recipe
/// table that could drift from the real one.
#[derive(Clone, Debug, Default)]
pub struct RecipeLine {
    pub recipe: String,
    pub name: String,
    pub skill: String,
    pub required_level: i32,
    pub skill_level: i32,
    pub craftable: bool,
    pub output_quantity: i32,
    /// `(item_kind, quantity)` per input, in the server's order.
    pub inputs: Vec<(String, i32)>,
}

/// One piece the Requisition counter stocks (EC-2): plain city-made gear for chits.
#[derive(Clone, Debug, Default)]
pub struct GearShopLine {
    pub slot: String,
    pub class_key: String,
    pub name: String,
    pub price_chits: i64,
    pub atk: i32,
    pub def: i32,
    pub spd: i32,
}

/// One active class-pair synergy or runnable combo (AD-2), as the party screen
/// renders it — the server describes them so the words never drift from the rules.
#[derive(Clone, Debug, Default)]
pub struct DepthLine {
    pub name: String,
    /// The mechanical effect (synergy) or the sequence (combo).
    pub detail: String,
    pub description: String,
    /// Combos only: the payoff bonus as a percentage.
    pub bonus_pct: i32,
}

/// One row of the Vanguard Board as the city panel renders it.
///
/// The board records HOW a run got deep, not only how far, and the endpoint has always sent
/// all of it — the client kept three fields, so the Wall was a list of names and numbers with
/// nothing to look at when you clicked one.
#[derive(Clone, Debug, Default)]
pub struct VanguardLine {
    pub rank: i32,
    pub username: String,
    pub max_distance: i32,
    /// The run level that posting was standing at when it set the mark.
    pub at_level: i32,
    /// Fights taken and fights fled on the way out. Going quietly is a real way to travel
    /// (the Pacifist unlock), so both halves matter.
    pub fights: i32,
    pub flees: i32,
    /// The END FIGHT's mark, if this posting felled it, and how long it took.
    pub star: Option<String>,
    pub clear_ms: Option<i64>,
}

/// One row of the Hunt Board as the Bounty Board panel renders it (AD-4). Every
/// number is the server's answer, including the reward.
#[derive(Clone, Debug, Default)]
pub struct HuntLine {
    pub key: String,
    pub name: String,
    pub objective: String,
    pub blurb: String,
    pub progress: i32,
    pub target: i32,
    pub claimable: bool,
    pub claimed: bool,
    pub reward_chits: i64,
    pub reward_material: String,
    pub reward_material_qty: i32,
    /// Finishing this one also hands over a rolled piece of gear.
    pub reward_gear: bool,
    /// Where to go to work it — the server's own answer, from the placement tables.
    pub where_to_look: String,
    /// Whether this hunt has been TAKEN. Only an accepted hunt is credited.
    pub accepted: bool,
}

impl HuntLine {
    /// Where this hunt belongs on the board: finished work first, then what is still in
    /// hand, then what has already been paid.
    ///
    /// A sort key rather than a comparison written at the call site, because the board's row
    /// order IS its claim order — the digit keys and the row chips both index straight into
    /// the list — so exactly one place may decide it.
    pub fn board_order(&self) -> u8 {
        match (self.claimed, self.claimable) {
            (false, true) => 0,
            (false, false) => 1,
            _ => 2,
        }
    }
}

/// One bounty contract as the Quests column renders it (AD-4). Every number is the
/// server's answer.
#[derive(Clone, Debug, Default)]
pub struct BountyLine {
    pub bounty_id: String,
    pub state: String,
    pub mark_name: String,
    pub boss_kind: String,
    pub distance: i32,
    pub venue: String,
    pub where_to_look: String,
    pub power: f64,
    pub expires_in_secs: i64,
    pub reward_chits: i64,
    pub reward_material: String,
    pub reward_material_qty: i32,
    pub reward_gear: bool,
    pub reward_rank_xp: i64,
}

/// The Den's board: rank, standing contracts, and what is already settled.
#[derive(Clone, Debug, Default)]
pub struct BountyBoard {
    pub rank: i32,
    pub rank_title: String,
    pub rank_xp_to_next: i64,
    pub active: Vec<BountyLine>,
    pub history: Vec<BountyLine>,
}

/// A meld-skill row for the level-up screen.
pub struct SkillLine {
    pub kind: String,
    pub level: i32,
    pub xp: i64,
}

/// One unlock, as the banner and the locked party-builder row need it.
#[derive(Debug, Clone)]
pub struct UnlockLine {
    pub key: String,
    pub name: String,
    /// `party_slot` or `class`.
    pub kind: String,
    pub class_key: Option<String>,
    pub slot: Option<i32>,
    pub trigger_text: String,
    pub banner: String,
}

/// One hero's stat gains for the classic "LEVEL UP!" screen (before, after).
#[derive(Clone)]
pub struct HeroLevelUpLine {
    pub name: String,
    pub class_key: String,
    pub level: i32,
    pub max_hp: (i32, i32),
    pub str_: (i32, i32),
    pub mnd: (i32, i32),
    pub dex: (i32, i32),
    pub wll: (i32, i32),
}

/// The caller's earned overworld class perks ("party sense"), mirrored from the
/// `run.perks` message. The client gates avatar glow, mob nameplates, the minimap,
/// and the battle ATB reveal by these. Defaults to no perks (aggro mult 1.0).
#[derive(Clone, Copy)]
pub struct PerksLine {
    pub explorer_glow: f32,
    pub hunter_intel: u8,
    pub explorer_map: u8,
    pub explorer_map_radius: f32,
    /// World-units at which a Shifter reveals dungeon entrances (0 = no Shifter).
    pub shifter_dungeon_radius: f32,
    /// Whether a Shifter can read a dropped item's permanence before picking it up.
    pub shifter_item_sense: bool,
    /// Dungeon cells within which a Shifter reveals ARMED traps (0 = none).
    pub shifter_trap_radius: f32,
    pub hunter_threat: u8,
    pub hunter_reveal_radius: f32,
    /// World-units at which a Smithwright reveals ORE veins / a Keeper REAGENT beds.
    /// The server already force-includes them in the snapshot; these are for the HUD
    /// hint and the minimap.
    pub smithwright_ore_radius: f32,
    pub keeper_reagent_radius: f32,
    pub resonant_regen: f32,
    pub psyker_hold_targets: u8,
    pub psyker_hold_seconds: f32,
    pub psyker_hold_cooldown: f32,
    pub psyker_hold_radius: f32,
    pub psyker_mind_link: bool,
    pub phoenix_guard_aggro_mult: f32,
}

impl Default for PerksLine {
    fn default() -> Self {
        Self {
            explorer_glow: 0.0,
            hunter_intel: 0,
            explorer_map: 0,
            explorer_map_radius: 0.0,
            shifter_dungeon_radius: 0.0,
            shifter_item_sense: false,
            shifter_trap_radius: 0.0,
            hunter_threat: 0,
            hunter_reveal_radius: 0.0,
            smithwright_ore_radius: 0.0,
            keeper_reagent_radius: 0.0,
            resonant_regen: 0.0,
            psyker_hold_targets: 0,
            psyker_hold_seconds: 0.0,
            psyker_hold_cooldown: 0.0,
            psyker_hold_radius: 0.0,
            psyker_mind_link: false,
            phoenix_guard_aggro_mult: 1.0,
        }
    }
}

/// A hero row for the party screen (name/class/level/stats live here, not battle).
#[derive(Clone)]
pub struct HeroLine {
    pub name: String,
    pub class_key: String,
    pub level: i32,
    pub str_: i32,
    pub mnd: i32,
    pub dex: i32,
    pub wll: i32,
    pub max_hp: i32,
    /// Current HP this run (wounds persist across battles until healed).
    pub hp: i32,
    /// This run's total XP and the level curve's threshold to advance —
    /// shared by the whole player's party, so every hero carries the same
    /// values.
    pub xp: i64,
    pub xp_to_next: i64,
    /// Formation rank: `true` = back row (halved damage, targeted less).
    pub back_row: bool,
    /// What still has hold of this hero, out of combat included — afflictions no longer
    /// expire, so most of them are felt out here: a distracted hero's controls are reversed
    /// and a blinded one can barely see.
    pub afflictions: Vec<String>,
}

impl HeroLine {
    /// Down, and needing a raise rather than a bandage.
    pub fn fallen(&self) -> bool {
        self.hp <= 0
    }

    /// What is WRONG with this hero, in one short player-facing phrase — `"Fallen"`, or
    /// the conditions holding them (`"Poisoned, Web"`) — and empty when they are fine.
    ///
    /// One helper because every surface that shows a hero has to agree: the party cell,
    /// the plate over their head in the field, and anything added later. An affliction does
    /// not expire (`meld_proto::statuses`), so out here it is a standing fact about the
    /// hero, not a combat detail — and it went unsaid everywhere outside the battle screen.
    pub fn condition_label(&self) -> String {
        if self.fallen() {
            return "Fallen".to_string();
        }
        self.afflictions
            .iter()
            .map(|a| {
                let mut c = a.replace('_', " ");
                if let Some(f) = c.get_mut(0..1) {
                    f.make_ascii_uppercase();
                }
                c
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

type InvPayload = (i64, Vec<(String, i32)>, Vec<GearLine>, Vec<(String, i32)>);
type ProgPayload = (Vec<SkillLine>, Vec<String>);

/// Events emitted from the network layer up to Bevy.
pub enum ServerMsg {
    Connected { player_id: String },
    Error { message: String },
    /// `run.started` — carries this run's terrain offset so the bin can seed the ground
    /// shader + entity Y (the lib netcode can't reach the render module directly).
    RunStarted {
        terrain_off: (f32, f32),
        peaks: Vec<[f32; 4]>,
        /// **CONTINENTS (WG-7):** this world's straits ([`meld_proto::coast::Strait`]).
        straits: Vec<meld_proto::coast::Strait>,
        /// **This WORLD's seed — its public name** (CANON D19). The world's own fact, never
        /// what we asked for: a client that shows the seed it requested rather than the one
        /// it got is the exact bug `tutorial` beside it exists to prevent.
        world_seed: u64,
        /// The coast's own shape: this world's bays and isles.
        lobes: Vec<meld_proto::coast::Lobe>,
        /// Inland water: this world's standing bodies and river chains.
        basins: Vec<meld_proto::coast::Basin>,
        rivers: Vec<meld_proto::coast::RiverNode>,
        /// **THE REGION DECOMPOSITION** ([`meld_proto::regions`]) — how this world is
        /// partitioned into cells, plus the `[biome_gate]` that decides which biome each may
        /// wear. The ground shader derives every fragment's cell from it, so a client that
        /// guessed instead would paint a world the server does not hold.
        regions: meld_proto::regions::Regions,
        tutorial: bool,
    },
    /// The caller's hero roster (name/class/level/stats) for the party panel.
    Party {
        heroes: Vec<HeroLine>,
        /// AD-2 build feedback: what this comp has active and what it can run.
        synergies: Vec<DepthLine>,
        combos: Vec<DepthLine>,
        /// `(ability key, one-line magnitudes)`. The registry's prose says what KIND of
        /// thing an ability is; the numbers are balance `[TUNABLE]`s the client cannot
        /// read, so the server resolves them and sends them along.
        abilities: Vec<(String, String)>,
        /// `(ability key, Adrenaline cost)` — Hunter skills only. Lets the battle menu
        /// grey out a skill the active hero can't currently afford instead of letting
        /// it submit and stall the hero for a turn it can never resolve.
        ability_costs: Vec<(String, i32)>,
    },
    /// The caller's earned overworld class perks (avatar glow, minimap, intel).
    Perks { perks: PerksLine },
    /// `onboarding.status` — what this account has already dismissed (the town
    /// welcome tour, the first-dive briefing). Sent once, post-connect-load —
    /// race-free w.r.t. a returning player's saved state (never rides the
    /// immediate `Connected` message, which fires before the load can land).
    OnboardingStatus { town_seen: bool, run_seen: bool },
    /// CL-1: what the account owns, plus anything just earned to announce.
    Unlocked {
        newly: Vec<UnlockLine>,
        owned: Vec<String>,
        party_slots: i32,
        banner: bool,
        /// PG-2: the account's all-time deepest distance — the bar every departure hub is
        /// gated on. The hub LIST comes from `meld_proto::hubs`, so this is the only number
        /// that has to travel.
        deepest_ever: i32,
    },
    /// The party gained a level — play the classic stat-gain screen.
    LevelUp {
        new_run_level: i32,
        levels_gained: i32,
        heroes: Vec<HeroLevelUpLine>,
    },
    /// Waypoints of the guaranteed clear path (world units) — drawn as a trail.
    WorldPath { points: Vec<(f64, f64)> },
    /// The web of extra trails (disjoint edges) drawn as dot-trails like the backbone.
    WorldWeb { edges: Vec<((f64, f64), (f64, f64))> },
    /// One overworld section's elevation grid + connectors (terraced verticality).
    /// Streamed at run start (initial chain) and as the frontier advances (endless).
    TerrainSection { section: TerrainSectionView },
    /// DG-6b: the client's cue to re-skin the environment as a secluded dungeon
    /// (`active`) — theme + play-area bounds drive the client-only enclosure — or to
    /// restore the overworld look (`!active`). Purely presentational.
    DungeonScene { active: bool, theme: String, floor: u32, width: u32, height: u32 },
    /// Walkable bounds + biome seams — the client frames the map with cliffs/water
    /// walls + gated chokepoints.
    WorldFrame {
        x_min: f64,
        x_max: f64,
        lateral: f64,
        west_return_border: f64,
        radial_arc_degrees: f64,
        seams: Vec<SeamLine>,
    },
    /// Current run backpack — drives the HUD. `items` are (item_kind, quantity),
    /// sorted; `chits` is the run's found-chits total; `gear` is looted red-chest
    /// gear as (name, atk_bonus).
    Backpack {
        items: Vec<(String, i32)>,
        chits: i64,
        gear: Vec<(String, i32)>,
    },
    /// Per-hero pouches, indexed by hero slot — the only items a hero can reach in a
    /// fight.
    Pouches {
        pouches: Vec<Vec<(String, i32)>>,
        capacity: i32,
    },
    Snapshot { entities: Vec<EntityView> },
    BattleStarted {
        battle_id: String,
        your_combatant_id: String,
        your_combatant_ids: Vec<String>,
        combatants: Vec<CombatantView>,
        monster_combatant: Option<String>,
        /// This is a fight we are WATCHING, not one we are in (`SOC-3`): no command
        /// menu, no loot report, and leaving it costs nothing. A flag rather than
        /// "`your_combatant_ids` is empty", because empty is also what a malformed
        /// roster looks like — and the back-compat fallback below turns that into a
        /// hero id of `""`, which would hand a watcher a menu aimed at nobody.
        spectating: bool,
    },
    TurnReady { combatant_id: String },
    /// An action resolved — drives hit feedback (floating numbers + flash).
    ActionResolved {
        actor: String,
        action: String,
        /// A monster's *instant*-ability shout ("Venom Fang!") riding the
        /// resolution (telegraphed ones already shouted via `Telegraph`).
        callout: Option<String>,
        effects: Vec<HitEffect>,
    },
    /// A monster shouted a telegraphed ability and is channeling it — show a
    /// flashing shout bubble + charging pose until the cast lands.
    Telegraph {
        combatant_id: String,
        text: String,
    },
    /// A second party merged into the battle (raid merge) — add their combatants.
    CombatantsJoined { combatants: Vec<CombatantView> },
    /// `CR-11`: creatures answered a pack leader's call. The bodies arrive as
    /// `CombatantsJoined`; this is the announcement, so the player is told rather than
    /// left to notice three new health bars.
    Reinforcements { called_by: String, arrived: usize },
    Gauge { updates: Vec<(String, f64, i32, Vec<String>)> },
    /// Terminal battle resolution — on victory this feeds the after-action
    /// report banner (XP/chits/items/gear gained this encounter).
    BattleEnded {
        outcome: String,
        xp: i64,
        chits: i64,
        items: Vec<(String, i32)>,
        gear_drops: Vec<(String, meld_proto::enums::Insurance)>,
        /// What the fight COST: `(hero name, points off each insured piece they wore)`
        /// for every hero of ours that fell (GR-2). Reported on every outcome, because
        /// a hero that went down in a fight you won still went down.
        /// `(hero name, durability points, ephemeral pieces burned)` per hero that fell.
        worn: Vec<(String, i32, Vec<String>)>,
    },
    /// Ground loot a creature left behind, just walked over and banked (`CR-2`). Feeds
    /// the same report banner as a chest — the loot a kill leaves is as much a payout as
    /// the loot a chest holds, and it was the only one the player was never shown.
    LootPickedUp { items: Vec<(String, i32)> },
    /// The fight we were WATCHING closed (`SOC-3`) — it finished, we walked out of
    /// range, we were pulled into our own, or we asked to stop. Never a `BattleEnded`:
    /// a watcher earned nothing, so popping somebody else's haul over their screen
    /// would be a lie.
    WatchEnded { battle_id: String, reason: String },
    /// A treasure chest was opened — feeds the same loot report banner as
    /// `BattleEnded` (no XP line, chest-only chits/items/gear).
    ChestOpened {
        chits: i64,
        items: Vec<(String, i32)>,
        gear: Vec<(String, meld_proto::enums::Insurance)>,
    },
    /// An extraction channel began / broke.
    ChannelStarted { completes_at: u64, fill_ms: u64, method: String },
    ChannelInterrupted,
    /// This player's run ended (extracted / died / abandoned), with the count of
    /// items + gear banked and the chits banked (extract) or lost (death).
    RunEnded {
        result: String,
        banked: usize,
        chits: i64,
        gear: usize,
    },
    /// Vault + gear, for the overworld inventory screen.
    InventoryData {
        chits: i64,
        materials: Vec<(String, i32)>,
        gear: Vec<GearLine>,
        /// Materials withdrawn from the Vault, staged to seed the next dive's
        /// Backpack.
        pending: Vec<(String, i32)>,
    },
    /// The Apothecary's shelf (`GET /v1/vendors/apothecary`), for the shop panel.
    ShopStock { vendor: String, items: Vec<ShopLine> },
    /// The Requisition counter's plain-gear stock (EC-2).
    GearShopStock { gear: Vec<GearShopLine> },
    /// The recipe book, for the Forge & Alembic (MS-1).
    Recipes { recipes: Vec<RecipeLine> },
    /// The Broker's standing offer on every material (MS-1).
    BrokerQuotes { quotes: Vec<BrokerQuote> },
    /// The result of a craft or a forge, in the player's words.
    CraftResult { text: String },
    /// Why a Vault write was refused, in the player's words. Only ever a refusal — a
    /// successful equip shows itself in the list, and narrating it would be noise.
    VaultNotice { text: String },
    /// A harvest channel paid out one tick's worth.
    Harvested { kind: String, qty: i32 },
    /// A field station answered: what the smith did (or would not), and the jobs left.
    SmithResult { message: String, ok: bool, uses_left: i32 },
    /// The heat is open: strike on the yellow. `bands` are fractions of the bar.
    TempoStarted {
        job_id: String,
        service: String,
        strikes: i32,
        sweep_ms: i64,
        bands: Vec<(f64, f64)>,
    },
    /// The seasonal Vanguard Board (`GET /v1/leaderboards/vanguard`), for the
    /// Vanguard Wall in Last City (P1-1). `you` is the caller's own rank, if any.
    /// The Hunt Board (`GET /v1/hunts`), for the Bounty Board in Last City (AD-4).
    HuntBoard { hunts: Vec<HuntLine> },
    /// The Den's bounty board (`GET /v1/bounties`), for the menu's Quests column (AD-4).
    Bounties { board: BountyBoard },
    /// A posted hunt moved while diving (`run.hunt_progress`).
    HuntProgress { name: String, progress: i32, target: i32, complete: bool },
    /// An anchor stopped a Shift (`world.shift_held`) — the region did not retile, and the
    /// land took it out of whatever was holding it (CANON §W3).
    /// Carries the WIRE type rather than a tuple of its fields. As a 4-tuple it silently
    /// dropped `max_hp` — the server populated a field the only client that reads it never
    /// decoded — and the two it did decode were positional enough that nothing used them.
    ShiftHeld { anchors: Vec<ww::HeldAnchor> },
    /// The tell: a ring of the Shifting Lands is about to swap biome (`world.shift_warning`).
    ShiftWarning {
        inner_radius: f64,
        outer_radius: f64,
        biome: String,
        lands_in_ms: u64,
        caught: bool,
    },
    /// It landed (`world.shift`). The retiled `world.terrain_section` messages arrive
    /// with it and are what actually repaint the ground — this is the words and the
    /// damage, not the render.
    Shifted { biome: String, from_biome: String, damage: Vec<i32> },
    /// The server moved this player (`movement.position_correction`) — today, because a
    /// Shift strewed the new land's props on top of them. The local avatar chases the
    /// snapshot exponentially for responsiveness, so a teleport has to say so or it
    /// renders as sliding across the map for a second.
    PositionCorrection { x: f64, y: f64 },
    VanguardBoard {
        season: i32,
        entries: Vec<VanguardLine>,
        you: Option<i32>,
    },
    /// Persistent per-account hero names (`GET /v1/heroes`), for the Equip/Status
    /// tabs when there's no active run's `PartyRoster` to source names from
    /// (e.g. opening the storage chest from the City).
    /// PT-2: the account's saved party loadouts.
    Loadouts { list: Vec<LoadoutLine> },
    HeroNames {
        names: Vec<String>,
        /// Each slot's persisted class key (GR-7); empty when never recorded.
        classes: Vec<String>,
    },
    /// Authoritative snapshot of this run's not-yet-banked loot gear (found
    /// this run; empty once a fresh dive starts). Sent whenever it changes —
    /// new loot, or an equip/unequip — so the Equip tab stays in sync.
    RunGear { gear: Vec<GearLine> },
    /// Meld skills + class unlocks, for the overworld level-up screen.
    ProgressData {
        skills: Vec<SkillLine>,
        classes: Vec<String>,
    },
    /// Co-op lobby state — members are (player_id, username, ready).
    LobbyState {
        code: String,
        host: String,
        members: Vec<(String, String, bool)>,
    },
    /// The lobby was disbanded / this player left it.
    LobbyClosed,
    Disconnected,
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    Http,
    WsConnecting,
    Ready,
    Dead,
}

/// The (ticket, player_id, session_token) login result, or an error string.
type LoginResult = Result<(String, String, String), String>;

struct Inner {
    base: String,
    phase: Phase,
    ws_tx: Option<WsSender>,
    ws_rx: Option<WsReceiver>,
    http_rx: Option<mpsc::Receiver<LoginResult>>,
    inv_rx: Option<mpsc::Receiver<InvPayload>>,
    prog_rx: Option<mpsc::Receiver<ProgPayload>>,
    heroes_rx: Option<mpsc::Receiver<(Vec<String>, Vec<String>)>>,
    loadouts_rx: Option<mpsc::Receiver<Vec<LoadoutLine>>>,
    vanguard_rx: Option<mpsc::Receiver<(i32, Vec<VanguardLine>, Option<i32>)>>,
    hunts_rx: Option<mpsc::Receiver<Vec<HuntLine>>>,
    bounties_rx: Option<mpsc::Receiver<BountyBoard>>,
    shop_rx: Option<mpsc::Receiver<(String, Vec<ShopLine>)>>,
    gear_shop_rx: Option<mpsc::Receiver<Vec<GearShopLine>>>,
    recipes_rx: Option<mpsc::Receiver<Vec<RecipeLine>>>,
    broker_rx: Option<mpsc::Receiver<Vec<BrokerQuote>>>,
    craft_rx: Option<mpsc::Receiver<String>>,
    /// Replies from Vault WRITES (equip/unequip). A refusal that reaches nobody is the
    /// same as a dead button from the player's side, and this is the only way back.
    vault_rx: Option<mpsc::Receiver<String>>,
    ticket: String,
    player_id: String,
    /// Bearer token for authenticated HTTP (vault/gear/players).
    session_token: String,
    seq: u32,
    input_seq: u32,
    cmds: VecDeque<ClientCmd>,
    out: VecDeque<ServerMsg>,
    /// Current run backpack counts (item_kind -> quantity), maintained from
    /// `run.started` + `run.backpack_update` so the overworld HUD can show your
    /// Town Portals + gathered materials.
    backpack: std::collections::HashMap<String, i32>,
    /// Run-scoped chits found so far (banked on extraction), for the HUD.
    run_chits: i64,
    /// Looted red-chest gear this run as (name, atk_bonus), for the HUD.
    run_gear: Vec<(String, i32)>,
    /// This run's not-yet-banked loot gear, structured for the Equip tab
    /// (mirrors `run.gear` snapshots from the server). Separate from
    /// `run_gear` above, which stays a flat display-only list for the HUD.
    run_loot_gear: Vec<GearLine>,
}

/// Bevy-side handle. Cloneable (shared `Rc`), single-threaded (NonSend resource).
#[derive(Clone)]
pub struct Net(Rc<RefCell<Inner>>);

/// Create the network layer. No I/O happens until the first `Connect` command.
pub fn start(base: String) -> Net {
    Net(Rc::new(RefCell::new(Inner {
        base,
        phase: Phase::Idle,
        ws_tx: None,
        ws_rx: None,
        http_rx: None,
        inv_rx: None,
        prog_rx: None,
        heroes_rx: None,
        loadouts_rx: None,
        vanguard_rx: None,
        hunts_rx: None,
        bounties_rx: None,
        shop_rx: None,
            gear_shop_rx: None,
            recipes_rx: None,
            broker_rx: None,
            craft_rx: None,
            vault_rx: None,
        ticket: String::new(),
        player_id: String::new(),
        session_token: String::new(),
        seq: 1,
        input_seq: 0,
        cmds: VecDeque::new(),
        out: VecDeque::new(),
        backpack: std::collections::HashMap::new(),
        run_chits: 0,
        run_gear: Vec::new(),
        run_loot_gear: Vec::new(),
    })))
}

impl Net {
    /// Queue a command (processed on the next `poll`).
    pub fn send(&self, cmd: ClientCmd) {
        self.0.borrow_mut().cmds.push_back(cmd);
    }

    /// Kick off an authenticated GET of vault + gear (→ `InventoryData`).
    pub fn fetch_inventory(&self) {
        self.0.borrow_mut().fetch_inventory();
    }

    /// Kick off an authenticated GET of the player profile (→ `ProgressData`).
    pub fn fetch_progress(&self) {
        self.0.borrow_mut().fetch_progress();
    }

    /// Equip a gear item to `Some(hero_slot)`, or unequip it with `None`, over
    /// HTTP, then refresh the inventory (→ a fresh `InventoryData`).
    pub fn equip_gear(&self, gear_id: String, hero_slot: Option<usize>) {
        self.0.borrow_mut().equip_gear(gear_id, hero_slot);
    }

    /// Unequip `free_first`, then equip `gear_id` to `hero_slot` — the two-handed
    /// path (GR-5): a player who picks a spear meant "and put the shield away",
    /// not "show me a 409".
    pub fn equip_gear_freeing(&self, free_first: String, gear_id: String, hero_slot: usize) {
        self.0.borrow_mut().equip_gear_freeing(free_first, gear_id, hero_slot);
    }

    /// Withdraw `qty` of a material from the Vault (storage chest) into the
    /// pending-backpack queue, then refresh the inventory.
    pub fn withdraw_material(&self, item_kind: String, qty: i32) {
        self.0.borrow_mut().withdraw_material(item_kind, qty);
    }

    /// Kick off an authenticated GET of the caller's persistent hero names
    /// (→ `HeroNames`) — for the Equip/Status tabs when opened outside of an
    /// active run (no `PartyRoster` to source names from).
    pub fn fetch_hero_names(&self) {
        self.0.borrow_mut().fetch_hero_names();
    }

    /// Kick off an authenticated GET of a town vendor's shelf (→ `ShopStock`).
    pub fn fetch_shop(&self) {
        self.0.borrow_mut().fetch_shop();
    }

    /// Buy `qty` of an item from the Apothecary, then refresh the shelf and the
    /// Vault so the chit balance the player sees is the server's, not a guess.
    pub fn buy_item(&self, item_kind: String, qty: i32) {
        self.0.borrow_mut().buy_item(item_kind, qty);
    }

    /// Buy another draw on a piece's affixes (MS-1).
    pub fn reroll_gear(&self, gear_id: String, material: String) {
        self.0.borrow_mut().reroll_gear(gear_id, material);
    }

    /// Buy back max durability a death chewed off (MS-1 / GR-2's repair sink).
    pub fn repair_gear(&self, gear_id: String) {
        self.0.borrow_mut().repair_gear(gear_id);
    }

    /// Kick off an authenticated GET of the Broker's price list (MS-1).
    pub fn fetch_broker(&self) {
        self.0.borrow_mut().fetch_broker();
    }

    /// Sell `qty` of a material to the Broker, then refresh the Vault.
    pub fn sell_material(&self, item_kind: String, qty: i32) {
        self.0.borrow_mut().sell_material(item_kind, qty);
    }

    /// Kick off an authenticated GET of the recipe book (MS-1).
    pub fn fetch_recipes(&self) {
        self.0.borrow_mut().fetch_recipes();
    }

    /// Run one recipe, then refresh the Vault.
    pub fn craft(&self, recipe: String) {
        self.0.borrow_mut().craft(recipe);
    }

    /// Forge one piece of gear from refined stock, optionally quenched in a trophy.
    pub fn forge(&self, slot: String, material: String, catalyst: Option<String>) {
        self.0.borrow_mut().forge(slot, material, catalyst);
    }

    /// Kick off an authenticated GET of the Requisition counter's gear stock (EC-2).
    pub fn fetch_gear_shop(&self) {
        self.0.borrow_mut().fetch_gear_shop();
    }

    /// Buy one plain piece of gear for chits, then refresh the Vault.
    pub fn buy_gear(&self, slot: String, class_key: String) {
        self.0.borrow_mut().buy_gear(slot, class_key);
    }

    /// Kick off an authenticated GET of the account's saved party loadouts (PT-2).
    pub fn fetch_loadouts(&self) {
        self.0.borrow_mut().fetch_loadouts();
    }

    /// Save (or overwrite) a named party composition, then refresh the list so what
    /// the panel shows is the server's answer rather than an optimistic guess.
    pub fn save_loadout(&self, name: String, classes: Vec<String>) {
        self.0.borrow_mut().save_loadout(name, classes);
    }

    /// Dress one hero from the spare gear (GR-5 "equip best").
    pub fn equip_best(&self, hero_slot: usize) {
        self.0.borrow_mut().equip_best(hero_slot);
    }

    /// Forget a named loadout, then refresh.
    pub fn delete_loadout(&self, name: String) {
        self.0.borrow_mut().delete_loadout(name);
    }

    pub fn rename_loadout(&self, from: String, to: String) {
        self.0.borrow_mut().rename_loadout(from, to);
    }

    /// Apply a saved loadout: the SERVER sets the classes and re-equips the gear it
    /// captured, re-validating every piece. The client sends only the name — it never
    /// says which gear to wear, so there is no request in which it could ask for gear
    /// it does not own.
    pub fn apply_loadout(&self, name: String) {
        self.0.borrow_mut().apply_loadout(name);
    }

    /// Kick off an authenticated GET of the Den's bounty board (→ `Bounties`), which
    /// also expires and re-rolls contracts server-side (AD-4).
    pub fn fetch_bounties(&self) {
        self.0.borrow_mut().fetch_bounties();
    }

    /// Take the Den's payment for a felled mark, then refresh the board and the Vault.
    pub fn claim_bounty(&self, bounty_id: String) {
        self.0.borrow_mut().claim_bounty(bounty_id);
    }

    /// Kick off an authenticated GET of the Hunt Board (→ `HuntBoard`) — the Bounty
    /// Board in Last City (AD-4).
    pub fn fetch_hunts(&self) {
        self.0.borrow_mut().fetch_hunts();
    }

    /// Take the reward for a finished hunt, then refresh the board and the Vault.
    pub fn accept_hunt(&self, key: String) {
        self.0.borrow_mut().accept_hunt(key);
    }

    pub fn claim_hunt(&self, key: String) {
        self.0.borrow_mut().claim_hunt(key);
    }

    /// Kick off an authenticated GET of the live Vanguard Board
    /// (→ `VanguardBoard`) — the Vanguard Wall in Last City.
    pub fn fetch_vanguard(&self) {
        self.0.borrow_mut().fetch_vanguard();
    }

    /// Advance the state machine: fire queued commands, pump HTTP + WS.
    pub fn poll(&self) {
        self.0.borrow_mut().step();
    }

    /// Pop the next server event, if any.
    pub fn try_recv(&self) -> Option<ServerMsg> {
        self.0.borrow_mut().out.pop_front()
    }
}

impl Inner {
    fn step(&mut self) {
        // 1. Drain queued commands.
        let cmds: Vec<ClientCmd> = self.cmds.drain(..).collect();
        for cmd in cmds {
            match cmd {
                ClientCmd::Connect { username, password } if self.phase == Phase::Idle => {
                    self.http_rx = Some(spawn_login(&self.base, &username, &password));
                    self.phase = Phase::Http;
                }
                ClientCmd::Connect { .. } => {} // already connecting/connected
                other if self.phase == Phase::Ready => self.send_cmd(other),
                _ => { /* not connected yet — drop movement/attack */ }
            }
        }

        // 2. HTTP login result → open the socket.
        if self.phase == Phase::Http {
            if let Some(rx) = &self.http_rx {
                match rx.try_recv() {
                    Ok(Ok((ticket, player_id, session_token))) => {
                        self.http_rx = None;
                        self.session_token = session_token;
                        self.open_socket(ticket, player_id);
                    }
                    Ok(Err(e)) => {
                        // Auth failed (wrong password, network, etc.). Return to Idle so
                        // the login screen can surface the error and let the user retry.
                        self.http_rx = None;
                        self.out.push_back(ServerMsg::Error { message: e });
                        self.phase = Phase::Idle;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.http_rx = None;
                        self.out.push_back(ServerMsg::Error {
                            message: "login task dropped".into(),
                        });
                        self.phase = Phase::Dead;
                    }
                }
            }
        }

        // 2b. Drain any HTTP data fetches (inventory / progress screens).
        if let Some(rx) = &self.inv_rx {
            if let Ok((chits, materials, gear, pending)) = rx.try_recv() {
                self.inv_rx = None;
                self.out.push_back(ServerMsg::InventoryData {
                    chits,
                    materials,
                    gear,
                    pending,
                });
            }
        }
        if let Some(rx) = &self.prog_rx {
            if let Ok((skills, classes)) = rx.try_recv() {
                self.prog_rx = None;
                self.out.push_back(ServerMsg::ProgressData { skills, classes });
            }
        }
        if let Some(rx) = &self.heroes_rx {
            if let Ok((names, classes)) = rx.try_recv() {
                self.heroes_rx = None;
                self.out.push_back(ServerMsg::HeroNames { names, classes });
            }
        }
        if let Some(rx) = &self.loadouts_rx {
            if let Ok(list) = rx.try_recv() {
                self.loadouts_rx = None;
                self.out.push_back(ServerMsg::Loadouts { list });
            }
        }

        if let Some(rx) = &self.shop_rx {
            if let Ok((vendor, items)) = rx.try_recv() {
                self.shop_rx = None;
                self.out.push_back(ServerMsg::ShopStock { vendor, items });
            }
        }
        if let Some(rx) = &self.gear_shop_rx {
            if let Ok(gear) = rx.try_recv() {
                self.gear_shop_rx = None;
                self.out.push_back(ServerMsg::GearShopStock { gear });
            }
        }
        if let Some(rx) = &self.recipes_rx {
            if let Ok(recipes) = rx.try_recv() {
                self.recipes_rx = None;
                self.out.push_back(ServerMsg::Recipes { recipes });
            }
        }
        if let Some(rx) = &self.broker_rx {
            if let Ok(quotes) = rx.try_recv() {
                self.broker_rx = None;
                self.out.push_back(ServerMsg::BrokerQuotes { quotes });
            }
        }
        if let Some(rx) = &self.craft_rx {
            if let Ok(text) = rx.try_recv() {
                self.craft_rx = None;
                self.out.push_back(ServerMsg::CraftResult { text });
            }
        }
        if let Some(rx) = &self.vault_rx {
            if let Ok(text) = rx.try_recv() {
                self.vault_rx = None;
                self.out.push_back(ServerMsg::VaultNotice { text });
            }
        }
        if let Some(rx) = &self.vanguard_rx {
            if let Ok((season, entries, you)) = rx.try_recv() {
                self.vanguard_rx = None;
                self.out.push_back(ServerMsg::VanguardBoard { season, entries, you });
            }
        }
        if let Some(rx) = &self.hunts_rx {
            if let Ok(hunts) = rx.try_recv() {
                self.hunts_rx = None;
                self.out.push_back(ServerMsg::HuntBoard { hunts });
            }
        }
        if let Some(rx) = &self.bounties_rx {
            if let Ok(board) = rx.try_recv() {
                self.bounties_rx = None;
                self.out.push_back(ServerMsg::Bounties { board });
            }
        }

        // 3. Drain socket events.
        let mut events = Vec::new();
        if let Some(rx) = self.ws_rx.as_mut() {
            while let Some(ev) = rx.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            self.on_ws_event(ev);
        }
    }

    /// GET `/v1/vault` then `/v1/vault/gear` (Bearer auth); deliver combined.
    fn fetch_inventory(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.inv_rx = Some(rx);
        spawn_inventory_fetch(self.base.clone(), self.session_token.clone(), tx);
    }

    /// POST equip (with a `{"hero_slot": N}` body) or unequip (empty body) for a
    /// gear item, then refresh the inventory so the overlay reflects the new
    /// loadout (a 409 leaves it unchanged). Loadout changes take effect at the
    /// next dive (vault-gear.md).
    fn equip_gear(&mut self, gear_id: String, hero_slot: Option<usize>) {
        if self.session_token.is_empty() || gear_id.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let (verb, body) = match hero_slot {
            Some(slot) => (
                "equip",
                serde_json::to_vec(&json!({ "hero_slot": slot })).unwrap_or_default(),
            ),
            None => ("unequip", Vec::new()),
        };
        let mut req = ehttp::Request::post(format!("{base}/v1/vault/gear/{gear_id}/{verb}"), body);
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.inv_rx = Some(rx);
        let (ntx, nrx) = mpsc::channel();
        self.vault_rx = Some(nrx);
        ehttp::fetch(req, move |res| {
            // A refusal has to reach the player. Dropping the response is what made a full
            // slot look like a dead button: the server said why in a 409 and nobody read it.
            if let Some(msg) = save_refusal(&res) {
                let _ = ntx.send(msg);
            }
            // Regardless of 200/409, re-read the vault so the UI shows truth.
            spawn_inventory_fetch(base, token, tx);
        });
    }

    /// Unequip one item and equip another in its place, sequentially so the
    /// server sees the hand freed before the two-handed weapon arrives.
    fn equip_gear_freeing(&mut self, free_first: String, gear_id: String, hero_slot: usize) {
        if self.session_token.is_empty() || gear_id.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut req = ehttp::Request::post(
            format!("{base}/v1/vault/gear/{free_first}/unequip"),
            Vec::new(),
        );
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.inv_rx = Some(rx);
        ehttp::fetch(req, move |_res| {
            let body = serde_json::to_vec(&json!({ "hero_slot": hero_slot })).unwrap_or_default();
            let mut req =
                ehttp::Request::post(format!("{base}/v1/vault/gear/{gear_id}/equip"), body);
            req.headers.insert("Authorization", format!("Bearer {token}"));
            req.headers.insert("Content-Type", "application/json");
            ehttp::fetch(req, move |_res| {
                spawn_inventory_fetch(base, token, tx);
            });
        });
    }

    /// POST a Vault (storage chest) material withdrawal, then refresh the
    /// inventory so the overlay reflects the new pending-backpack queue (a 409
    /// — not enough in stock — leaves it unchanged).
    fn withdraw_material(&mut self, item_kind: String, qty: i32) {
        if self.session_token.is_empty() || item_kind.is_empty() || qty <= 0 {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let body = serde_json::to_vec(&json!({ "quantity": qty })).unwrap_or_default();
        let mut req = ehttp::Request::post(
            format!("{base}/v1/vault/materials/{item_kind}/withdraw"),
            body,
        );
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.inv_rx = Some(rx);
        ehttp::fetch(req, move |_res| {
            spawn_inventory_fetch(base, token, tx);
        });
    }

    /// GET `/v1/heroes` (Bearer auth) for persistent hero names.
    fn fetch_hero_names(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.heroes_rx = Some(rx);
        let token = self.session_token.clone();
        let mut req = ehttp::Request::get(format!("{}/v1/heroes", self.base));
        req.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(req, move |res| {
            let (mut names, mut classes) = (Vec::new(), Vec::new());
            if let Ok(resp) = &res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    if let Some(arr) = v["names"].as_array() {
                        names = arr.iter().filter_map(|n| n.as_str().map(String::from)).collect();
                    }
                    if let Some(arr) = v["classes"].as_array() {
                        classes = arr.iter().filter_map(|c| c.as_str().map(String::from)).collect();
                    }
                }
            }
            let _ = tx.send((names, classes));
        });
    }

    fn fetch_loadouts(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.loadouts_rx = Some(rx);
        spawn_loadouts_fetch(self.base.clone(), self.session_token.clone(), tx);
    }

    fn save_loadout(&mut self, name: String, classes: Vec<String>) {
        if self.session_token.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut req = ehttp::Request::post(
            format!("{base}/v1/party/loadouts"),
            serde_json::to_vec(&serde_json::json!({ "name": name, "classes": classes }))
                .unwrap_or_default(),
        );
        req.headers.insert("Content-Type", "application/json");
        req.headers.insert("Authorization", format!("Bearer {token}"));
        // The server validates against the account's unlocks and can refuse, so the list is
        // re-read rather than assumed — but INSIDE the write's callback. Fired alongside it,
        // the read raced the write and returned the list without the new row.
        let (tx, rx) = mpsc::channel();
        self.loadouts_rx = Some(rx);
        let (etx, erx) = mpsc::channel();
        self.craft_rx = Some(erx);
        ehttp::fetch(req, move |res| {
            // And say so when it refuses: a save that silently does nothing is the same
            // bug from the player's side whether the cause is a race or a rejection.
            if let Some(msg) = save_refusal(&res) {
                let _ = etx.send(msg);
            }
            spawn_loadouts_fetch(base, token, tx);
        });
    }

    fn apply_loadout(&mut self, name: String) {
        if self.session_token.is_empty() {
            return;
        }
        let token = self.session_token.clone();
        let mut req = ehttp::Request::post(
            format!("{}/v1/party/loadouts/{name}/apply", self.base),
            Vec::new(),
        );
        req.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(req, |_| {});
        // The equip state the server just rewrote is what the panels read.
        self.fetch_hero_names();
        self.fetch_inventory();
    }

    fn delete_loadout(&mut self, name: String) {
        if self.session_token.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut req = ehttp::Request {
            method: "DELETE".to_string(),
            ..ehttp::Request::get(format!("{base}/v1/party/loadouts/{name}"))
        };
        req.headers.insert("Authorization", format!("Bearer {token}"));
        // Re-read INSIDE the write's callback, the same way `save_loadout` does. Fired
        // alongside it — which is how this shipped — the read races the DELETE and comes
        // back with the row still in it, so a deleted party stays on screen until something
        // else happens to refresh the list.
        let (tx, rx) = mpsc::channel();
        self.loadouts_rx = Some(rx);
        ehttp::fetch(req, move |_| {
            spawn_loadouts_fetch(base, token, tx);
        });
    }

    /// Rename a saved party, keeping the gear it stored (`PT-2`).
    ///
    /// Its own endpoint rather than save-new-then-delete-old, because a save captures the
    /// party's CURRENT gear — so the shortcut would rewrite a loadout's contents as the
    /// price of fixing a typo.
    fn rename_loadout(&mut self, from: String, to: String) {
        if self.session_token.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut req = ehttp::Request::post(
            format!("{base}/v1/party/loadouts/{from}/rename"),
            serde_json::to_vec(&serde_json::json!({ "new_name": to })).unwrap_or_default(),
        );
        req.headers.insert("Content-Type", "application/json");
        req.headers.insert("Authorization", format!("Bearer {token}"));
        let (tx, rx) = mpsc::channel();
        self.loadouts_rx = Some(rx);
        // The server refuses a name already in use rather than eating the other row, so
        // say so — a rename that silently did nothing is the same bug from this side.
        let (etx, erx) = mpsc::channel();
        self.craft_rx = Some(erx);
        ehttp::fetch(req, move |res| {
            if let Some(msg) = save_refusal(&res) {
                let _ = etx.send(msg);
            }
            spawn_loadouts_fetch(base, token, tx);
        });
    }

    /// GET `/v1/vendors/apothecary` (Bearer auth) for the shop panel.
    fn fetch_shop(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.shop_rx = Some(rx);
        let token = self.session_token.clone();
        let mut req = ehttp::Request::get(format!("{}/v1/vendors/apothecary", self.base));
        req.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(req, move |res| {
            let (mut vendor, mut items) = (String::new(), Vec::new());
            if let Ok(resp) = &res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    vendor = v["name"].as_str().unwrap_or("Vendor").to_string();
                    for s in v["data"].as_array().into_iter().flatten() {
                        items.push(ShopLine {
                            item_kind: s["item_kind"].as_str().unwrap_or("").to_string(),
                            name: s["name"].as_str().unwrap_or("").to_string(),
                            description: s["description"].as_str().unwrap_or("").to_string(),
                            price_chits: s["price_chits"].as_i64().unwrap_or(0),
                        });
                    }
                }
            }
            let _ = tx.send((vendor, items));
        });
    }

    /// POST a purchase, then re-read the Vault: the chit balance a player sees must
    /// be the server's, never the client's arithmetic.
    fn buy_item(&mut self, item_kind: String, qty: i32) {
        if self.session_token.is_empty() || item_kind.is_empty() || qty <= 0 {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let body = serde_json::to_vec(&json!({ "item_kind": item_kind, "quantity": qty }))
            .unwrap_or_default();
        let mut req =
            ehttp::Request::post(format!("{base}/v1/vendors/apothecary/buy"), body);
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.inv_rx = Some(rx);
        ehttp::fetch(req, move |_res| {
            spawn_inventory_fetch(base, token, tx);
        });
    }

    /// POST `/v1/vault/gear/:id/reroll` — buy another draw on a piece's affixes. The
    /// stats are untouched: what a smith sells is a chance, not a better item.
    fn reroll_gear(&mut self, gear_id: String, material: String) {
        if self.session_token.is_empty() || gear_id.is_empty() || material.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let body = serde_json::to_vec(&json!({ "material": material })).unwrap_or_default();
        let mut req =
            ehttp::Request::post(format!("{base}/v1/vault/gear/{gear_id}/reroll"), body);
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.craft_rx = Some(rx);
        let (itx, irx) = mpsc::channel();
        self.inv_rx = Some(irx);
        ehttp::fetch(req, move |res| {
            let _ = tx.send(reroll_reply_text(&res));
            spawn_inventory_fetch(base, token, itx);
        });
    }

    /// POST `/v1/party/heroes/:slot/equip-best` — let the SERVER dress this hero from the
    /// spare gear. One call, one atomic answer: doing the picking here would mean firing an
    /// equip per slot and hoping, which is the race that made saving a party look broken.
    fn equip_best(&mut self, hero_slot: usize) {
        if self.session_token.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut req = ehttp::Request::post(
            format!("{base}/v1/party/heroes/{hero_slot}/equip-best"),
            Vec::new(),
        );
        req.headers.insert("Authorization", format!("Bearer {token}"));
        let (tx, rx) = mpsc::channel();
        self.craft_rx = Some(rx);
        let (itx, irx) = mpsc::channel();
        self.inv_rx = Some(irx);
        ehttp::fetch(req, move |res| {
            let _ = tx.send(equip_best_reply(&res));
            spawn_inventory_fetch(base, token, itx);
        });
    }

    /// POST `/v1/vault/gear/:id/repair` — buy back max durability a death chewed off.
    fn repair_gear(&mut self, gear_id: String) {
        if self.session_token.is_empty() || gear_id.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut req = ehttp::Request::post(
            format!("{base}/v1/vault/gear/{gear_id}/repair"),
            Vec::new(),
        );
        req.headers.insert("Authorization", format!("Bearer {token}"));
        let (tx, rx) = mpsc::channel();
        self.craft_rx = Some(rx);
        let (itx, irx) = mpsc::channel();
        self.inv_rx = Some(irx);
        ehttp::fetch(req, move |res| {
            let _ = tx.send(repair_reply_text(&res));
            spawn_inventory_fetch(base, token, itx);
        });
    }

    /// GET `/v1/vendors/broker` — what the Broker pays for each material, already
    /// scaled to the caller's Mercantile level by the server (MS-1).
    fn fetch_broker(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.broker_rx = Some(rx);
        let token = self.session_token.clone();
        let mut req = ehttp::Request::get(format!("{}/v1/vendors/broker", self.base));
        req.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(req, move |res| {
            let mut quotes = Vec::new();
            if let Ok(resp) = &res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    for q in v["data"].as_array().into_iter().flatten() {
                        quotes.push(BrokerQuote {
                            item_kind: q["item_kind"].as_str().unwrap_or("").to_string(),
                            name: q["name"].as_str().unwrap_or("").to_string(),
                            price_chits: q["price_chits"].as_i64().unwrap_or(0),
                        });
                    }
                }
            }
            let _ = tx.send(quotes);
        });
    }

    /// POST a sale, then re-read the Vault — the chits and the remaining stack a player
    /// sees must be the server's answer, never the client's arithmetic.
    fn sell_material(&mut self, item_kind: String, qty: i32) {
        if self.session_token.is_empty() || item_kind.is_empty() || qty <= 0 {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let body = serde_json::to_vec(&json!({ "item_kind": item_kind, "quantity": qty }))
            .unwrap_or_default();
        let mut req = ehttp::Request::post(format!("{base}/v1/vendors/broker/sell"), body);
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.inv_rx = Some(rx);
        ehttp::fetch(req, move |_res| {
            spawn_inventory_fetch(base, token, tx);
        });
    }

    /// GET `/v1/crafting/recipes` — the recipe book with the caller's own level gates
    /// already resolved by the server (MS-1).
    fn fetch_recipes(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.recipes_rx = Some(rx);
        let token = self.session_token.clone();
        let mut req = ehttp::Request::get(format!("{}/v1/crafting/recipes", self.base));
        req.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(req, move |res| {
            let mut recipes = Vec::new();
            if let Ok(resp) = &res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    for r in v["data"].as_array().into_iter().flatten() {
                        recipes.push(RecipeLine {
                            recipe: r["recipe"].as_str().unwrap_or("").to_string(),
                            name: r["name"].as_str().unwrap_or("").to_string(),
                            skill: r["skill"].as_str().unwrap_or("").to_string(),
                            required_level: r["required_level"].as_i64().unwrap_or(1) as i32,
                            skill_level: r["skill_level"].as_i64().unwrap_or(1) as i32,
                            craftable: r["craftable"].as_bool().unwrap_or(false),
                            output_quantity: r["output_quantity"].as_i64().unwrap_or(1) as i32,
                            inputs: r["inputs"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|i| {
                                    (
                                        i["item_kind"].as_str().unwrap_or("").to_string(),
                                        i["quantity"].as_i64().unwrap_or(0) as i32,
                                    )
                                })
                                .collect(),
                        });
                    }
                }
            }
            let _ = tx.send(recipes);
        });
    }

    /// POST a craft, then re-read the Vault. The reply is turned into one line of
    /// player-facing text here so the panel never has to parse JSON — and a REFUSAL is
    /// reported just as loudly as a success, because "nothing happened" is the worst
    /// answer a crafting screen can give.
    fn craft(&mut self, recipe: String) {
        if self.session_token.is_empty() || recipe.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let body = serde_json::to_vec(&json!({ "recipe": recipe })).unwrap_or_default();
        let mut req = ehttp::Request::post(format!("{base}/v1/crafting/craft"), body);
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.craft_rx = Some(rx);
        let (itx, irx) = mpsc::channel();
        self.inv_rx = Some(irx);
        ehttp::fetch(req, move |res| {
            let _ = tx.send(craft_reply_text(&res));
            spawn_inventory_fetch(base, token, itx);
        });
    }

    /// POST a forge, then re-read the Vault. Same one-line reply as `craft`, but the
    /// success case names the STATS, since that is the whole reason to forge.
    fn forge(&mut self, slot: String, material: String, catalyst: Option<String>) {
        if self.session_token.is_empty() || slot.is_empty() || material.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut payload = json!({ "slot": slot, "material": material });
        if let Some(c) = catalyst {
            payload["catalyst"] = json!(c);
        }
        let body = serde_json::to_vec(&payload).unwrap_or_default();
        let mut req = ehttp::Request::post(format!("{base}/v1/crafting/forge"), body);
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.craft_rx = Some(rx);
        let (itx, irx) = mpsc::channel();
        self.inv_rx = Some(irx);
        ehttp::fetch(req, move |res| {
            let _ = tx.send(forge_reply_text(&res));
            spawn_inventory_fetch(base, token, itx);
        });
    }

    /// GET `/v1/vendors/requisition` — the counter's plain-gear stock for the caller's
    /// own roster (EC-2).
    fn fetch_gear_shop(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.gear_shop_rx = Some(rx);
        let token = self.session_token.clone();
        let mut req = ehttp::Request::get(format!("{}/v1/vendors/requisition", self.base));
        req.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(req, move |res| {
            let mut gear = Vec::new();
            if let Ok(resp) = &res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    for g in v["data"].as_array().into_iter().flatten() {
                        gear.push(GearShopLine {
                            slot: g["slot"].as_str().unwrap_or("").to_string(),
                            class_key: g["class_key"].as_str().unwrap_or("").to_string(),
                            name: g["name"].as_str().unwrap_or("").to_string(),
                            price_chits: g["price_chits"].as_i64().unwrap_or(0),
                            atk: g["stats"]["atk"].as_i64().unwrap_or(0) as i32,
                            def: g["stats"]["def"].as_i64().unwrap_or(0) as i32,
                            spd: g["stats"]["spd"].as_i64().unwrap_or(0) as i32,
                        });
                    }
                }
            }
            let _ = tx.send(gear);
        });
    }

    /// POST a gear purchase, then re-read the Vault so the chits and the new piece the
    /// player sees are the server's answer rather than the client's arithmetic.
    fn buy_gear(&mut self, slot: String, class_key: String) {
        if self.session_token.is_empty() || slot.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let body = serde_json::to_vec(&json!({ "slot": slot, "class_key": class_key }))
            .unwrap_or_default();
        let mut req = ehttp::Request::post(format!("{base}/v1/vendors/requisition/buy"), body);
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.inv_rx = Some(rx);
        ehttp::fetch(req, move |_res| {
            spawn_inventory_fetch(base, token, tx);
        });
    }

    /// GET `/v1/bounties` (Bearer auth). A `403` is not an error: it is an account that
    /// has not earned the Den yet, and the menu simply has no Quests row for it.
    fn fetch_bounties(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.bounties_rx = Some(rx);
        spawn_bounties_fetch(self.base.clone(), self.session_token.clone(), tx);
    }

    /// POST `/v1/bounties/:id/claim`, then re-read the board and the Vault.
    fn claim_bounty(&mut self, bounty_id: String) {
        if self.session_token.is_empty() || bounty_id.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut req =
            ehttp::Request::post(format!("{base}/v1/bounties/{bounty_id}/claim"), Vec::new());
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.vault_rx = Some(rx);
        let (itx, irx) = mpsc::channel();
        self.inv_rx = Some(irx);
        let (btx, brx) = mpsc::channel();
        self.bounties_rx = Some(brx);
        ehttp::fetch(req, move |res| {
            let _ = tx.send(bounty_claim_text(&res));
            spawn_bounties_fetch(base.clone(), token.clone(), btx);
            spawn_inventory_fetch(base, token, itx);
        });
    }

    /// GET `/v1/hunts` (Bearer auth) — every posted hunt with this account's progress.
    fn fetch_hunts(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.hunts_rx = Some(rx);
        spawn_hunts_fetch(self.base.clone(), self.session_token.clone(), tx);
    }

    /// POST `/v1/hunts/:key/accept`, then re-read the board so the row's new state is the
    /// server's answer. No Vault re-read: taking a hunt pays nothing.
    fn accept_hunt(&mut self, key: String) {
        if self.session_token.is_empty() || key.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut req = ehttp::Request::post(format!("{base}/v1/hunts/{key}/accept"), Vec::new());
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (htx, hrx) = mpsc::channel();
        self.hunts_rx = Some(hrx);
        ehttp::fetch(req, move |_res| {
            spawn_hunts_fetch(base, token, htx);
        });
    }

    /// POST `/v1/hunts/:key/claim`, then re-read the board and the Vault so what the
    /// panel shows is the server's answer rather than an optimistic guess.
    fn claim_hunt(&mut self, key: String) {
        if self.session_token.is_empty() || key.is_empty() {
            return;
        }
        let base = self.base.clone();
        let token = self.session_token.clone();
        let mut req = ehttp::Request::post(format!("{base}/v1/hunts/{key}/claim"), Vec::new());
        req.headers.insert("Authorization", format!("Bearer {token}"));
        req.headers.insert("Content-Type", "application/json");
        let (tx, rx) = mpsc::channel();
        self.vault_rx = Some(rx);
        let (itx, irx) = mpsc::channel();
        self.inv_rx = Some(irx);
        let (htx, hrx) = mpsc::channel();
        self.hunts_rx = Some(hrx);
        ehttp::fetch(req, move |res| {
            let _ = tx.send(hunt_claim_text(&res));
            spawn_hunts_fetch(base.clone(), token.clone(), htx);
            spawn_inventory_fetch(base, token, itx);
        });
    }

    /// GET `/v1/leaderboards/vanguard` (Bearer auth) for the live seasonal board.
    /// The caller's own rank is read off the same page rather than a second
    /// `/me` round-trip — the board is the top 100, so an unlisted caller is
    /// simply unranked as far as the wall is concerned.
    fn fetch_vanguard(&mut self) {
        // No token check: the current-season board is PUBLIC so the login screen can
        // show it before anyone has logged in. The header still rides along when a
        // token exists, so `you` (the caller's own rank) resolves once signed in.
        let (tx, rx) = mpsc::channel();
        self.vanguard_rx = Some(rx);
        let token = self.session_token.clone();
        let me = self.player_id.clone();
        let mut req = ehttp::Request::get(format!("{}/v1/leaderboards/vanguard", self.base));
        if !token.is_empty() {
            req.headers.insert("Authorization", format!("Bearer {token}"));
        }
        ehttp::fetch(req, move |res| {
            let (mut season, mut entries, mut you) = (0, Vec::new(), None);
            if let Ok(resp) = &res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    season = v["season"].as_i64().unwrap_or(0) as i32;
                    for e in v["data"].as_array().into_iter().flatten() {
                        let rank = e["rank"].as_i64().unwrap_or(0) as i32;
                        if e["player_id"].as_str() == Some(me.as_str()) {
                            you = Some(rank);
                        }
                        entries.push(VanguardLine {
                            rank,
                            username: e["username"].as_str().unwrap_or("?").to_string(),
                            max_distance: e["max_distance"].as_i64().unwrap_or(0) as i32,
                            at_level: e["at_level"].as_i64().unwrap_or(0) as i32,
                            fights: e["fights"].as_i64().unwrap_or(0) as i32,
                            flees: e["flees"].as_i64().unwrap_or(0) as i32,
                            star: e["star"].as_str().map(str::to_string),
                            clear_ms: e["clear_ms"].as_i64(),
                        });
                    }
                }
            }
            let _ = tx.send((season, entries, you));
        });
    }

    /// GET `/v1/players/me` (Bearer auth) for meld skills + class unlocks.
    fn fetch_progress(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.prog_rx = Some(rx);
        let token = self.session_token.clone();
        let mut req = ehttp::Request::get(format!("{}/v1/players/me", self.base));
        req.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(req, move |res| {
            let mut skills = Vec::new();
            let mut classes = Vec::new();
            if let Ok(resp) = &res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    if let Some(arr) = v["meld_skills"].as_array() {
                        skills = arr
                            .iter()
                            .map(|s| SkillLine {
                                kind: s["skill_kind"].as_str().unwrap_or("?").to_string(),
                                level: s["level"].as_i64().unwrap_or(1) as i32,
                                xp: s["xp"].as_i64().unwrap_or(0),
                            })
                            .collect();
                    }
                    if let Some(arr) = v["class_unlocks"].as_array() {
                        classes = arr
                            .iter()
                            .filter_map(|c| c.as_str().map(String::from))
                            .collect();
                    }
                }
            }
            let _ = tx.send((skills, classes));
        });
    }

    fn open_socket(&mut self, ticket: String, player_id: String) {
        let ws_url = format!("{}/v1/realtime", self.base.replacen("http", "ws", 1));
        match ewebsock::connect(&ws_url, ewebsock::Options::default()) {
            Ok((tx, rx)) => {
                self.ws_tx = Some(tx);
                self.ws_rx = Some(rx);
                self.ticket = ticket;
                self.player_id = player_id;
                self.seq = 1;
                self.phase = Phase::WsConnecting;
            }
            Err(e) => {
                self.out.push_back(ServerMsg::Error {
                    message: format!("ws connect: {e}"),
                });
                self.phase = Phase::Dead;
            }
        }
    }

    fn on_ws_event(&mut self, ev: WsEvent) {
        match ev {
            WsEvent::Opened => {
                // First frame must be session.authenticate (seq 1).
                self.send_env(
                    ws::Authenticate::TYPE,
                    json!({ "ticket": self.ticket, "resume": null }),
                );
            }
            WsEvent::Message(WsMessage::Text(t)) => self.handle_text(&t),
            WsEvent::Message(_) => {}
            WsEvent::Error(e) => {
                self.out.push_back(ServerMsg::Error { message: e });
                self.phase = Phase::Dead;
            }
            WsEvent::Closed => {
                self.out.push_back(ServerMsg::Disconnected);
                self.phase = Phase::Dead;
            }
        }
    }

    fn send_cmd(&mut self, cmd: ClientCmd) {
        match cmd {
            // The client's direct enter is always a solo (private) dive; co-op
            // goes through the lobby. (Bot tests that want grouping send raw JSON
            // without `solo`.)
            ClientCmd::EnterMaze { party, tutorial, hub } => self.send_env(
                wr::EnterMaze::TYPE,
                json!({ "party": party, "solo": true, "tutorial": tutorial, "hub": hub }),
            ),
            ClientCmd::Move { dx, dy } => {
                self.input_seq += 1;
                self.send_env(
                    wm::MoveIntent::TYPE,
                    json!({
                        "input_seq": self.input_seq,
                        "move_dir": { "x": dx, "y": dy },
                        "client_pos": { "x": 0.0, "y": 0.0 }
                    }),
                );
            }
            // v4 (random) not v7 for action_id — v7 needs a system clock, which
            // Uniqueness is all the server needs here.
            ClientCmd::Attack {
                battle_id,
                actor,
                target,
            } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "actor_combatant_id": actor,
                    "action": "attack",
                    "skill_kind": null,
                    "item_id": null,
                    "target_ids": [target]
                }),
            ),
            ClientCmd::Defend { battle_id, actor } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "actor_combatant_id": actor,
                    "action": "defend",
                    "skill_kind": null,
                    "item_id": null,
                    "target_ids": null
                }),
            ),
            ClientCmd::Flee { battle_id, actor } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "actor_combatant_id": actor,
                    "action": "flee",
                    "skill_kind": null,
                    "item_id": null,
                    "target_ids": null
                }),
            ),
            ClientCmd::Skill {
                battle_id,
                actor,
                target,
                skill_kind,
            } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "actor_combatant_id": actor,
                    "action": "skill",
                    "skill_kind": skill_kind,
                    "item_id": null,
                    "target_ids": [target]
                }),
            ),
            ClientCmd::Item {
                battle_id,
                actor,
                item_id,
                target,
            } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "actor_combatant_id": actor,
                    "action": "item",
                    "skill_kind": null,
                    "item_id": item_id,
                    "target_ids": [target]
                }),
            ),
            ClientCmd::UseItem { item_kind, hero_slot } => self.send_env(
                wr::UseItem::TYPE,
                json!({ "item_kind": item_kind, "hero_slot": hero_slot }),
            ),
            ClientCmd::MoveItem { item_kind, hero_slot, to_pouch } => self.send_env(
                wr::MoveItem::TYPE,
                json!({
                    "item_kind": item_kind,
                    "hero_slot": hero_slot,
                    "to_pouch": to_pouch,
                    "quantity": 1
                }),
            ),
            ClientCmd::Extract => self.send_env(
                wr::BeginExtraction::TYPE,
                json!({ "method": "portal", "portal_entity_id": "portal", "item_id": null }),
            ),
            ClientCmd::TownPortal => self.send_env(
                wr::BeginExtraction::TYPE,
                json!({ "method": "town_portal", "portal_entity_id": null, "item_id": null }),
            ),
            ClientCmd::PsykerHold { entity_id } => {
                self.send_env(wr::PsykerHold::TYPE, json!({ "entity_id": entity_id }))
            }
            ClientCmd::Harvest { entity_id } => {
                self.send_env(wr::Harvest::TYPE, json!({ "entity_id": entity_id }))
            }
            ClientCmd::CancelHarvest => self.send_env(wr::CancelHarvest::TYPE, json!({})),
            ClientCmd::OpenChest { entity_id } => {
                self.send_env(wr::OpenChest::TYPE, json!({ "entity_id": entity_id }))
            }
            ClientCmd::EnterDungeon { entity_id } => {
                self.send_env(wr::EnterDungeon::TYPE, json!({ "entity_id": entity_id }))
            }
            ClientCmd::BuildStation { kind } => {
                self.send_env(wr::BuildStation::TYPE, json!({ "kind": kind }))
            }
            ClientCmd::BuildStructureAt { function, at, yaw } => self.send_env(
                wr::BuildStructure::TYPE,
                json!({
                    "function": function,
                    "at": { "x": at.0, "y": at.1 },
                    "yaw": yaw,
                }),
            ),
            ClientCmd::BuildStructure { function } => {
                self.send_env(wr::BuildStructure::TYPE, json!({ "function": function }))
            }
            ClientCmd::RepairStructure { entity_id } => {
                self.send_env(wr::RepairStructure::TYPE, json!({ "entity_id": entity_id }))
            }
            ClientCmd::DemolishStructure { entity_id } => {
                self.send_env(wr::DemolishStructure::TYPE, json!({ "entity_id": entity_id }))
            }
            ClientCmd::SmithRequest { entity_id, gear_id, service, material, recipe } => self
                .send_env(
                    wr::SmithRequest::TYPE,
                    json!({
                        "entity_id": entity_id,
                        "gear_id": gear_id,
                        "service": service,
                        "material": material,
                        "recipe": recipe,
                    }),
                ),
            ClientCmd::TeardownStation { entity_id } => {
                self.send_env(wr::TeardownStation::TYPE, json!({ "entity_id": entity_id }))
            }
            ClientCmd::Strike { job_id, at } => {
                self.send_env(wr::Strike::TYPE, json!({ "job_id": job_id, "at": at }))
            }
            ClientCmd::JoinBattle => self.send_env(wr::JoinBattle::TYPE, json!({})),
            ClientCmd::WatchBattle => self.send_env(wr::WatchBattle::TYPE, json!({})),
            ClientCmd::StopWatching => self.send_env(wr::StopWatching::TYPE, json!({})),
            ClientCmd::RenameHero { slot, name } => {
                self.send_env(wr::RenameHero::TYPE, json!({ "slot": slot, "name": name }))
            }
            ClientCmd::SetFormation { slot, back_row } => {
                self.send_env(wr::SetFormation::TYPE, json!({ "slot": slot, "back_row": back_row }))
            }
            ClientCmd::EquipLoot { gear_id, hero_slot } => self.send_env(
                wr::EquipLoot::TYPE,
                json!({ "gear_id": gear_id, "hero_slot": hero_slot }),
            ),
            ClientCmd::LobbyCreate { party } => {
                self.send_env(wl::Create::TYPE, json!({ "party": party }))
            }
            ClientCmd::LobbyJoin { code, party } => {
                self.send_env(wl::Join::TYPE, json!({ "code": code, "party": party }))
            }
            ClientCmd::LobbyReady { ready } => {
                self.send_env(wl::Ready::TYPE, json!({ "ready": ready }))
            }
            ClientCmd::LobbyStart => self.send_env(wl::Start::TYPE, json!({})),
            ClientCmd::LobbyLeave => self.send_env(wl::Leave::TYPE, json!({})),
            ClientCmd::OnboardingTownSeen => self.send_env(wo::TownSeen::TYPE, json!({})),
            ClientCmd::OnboardingRunSeen => self.send_env(wo::RunSeen::TYPE, json!({})),
            ClientCmd::Connect { .. } => {}
        }
    }

    fn send_env(&mut self, ty: &str, payload: serde_json::Value) {
        if let Some(tx) = self.ws_tx.as_mut() {
            let env = json!({ "type": ty, "seq": self.seq, "ts": 0u64, "payload": payload });
            tx.send(WsMessage::Text(env.to_string()));
            self.seq += 1;
        }
    }

    /// Emit the current backpack (items + chits + looted gear) for the HUD.
    fn emit_backpack(&mut self) {
        let mut items: Vec<(String, i32)> =
            self.backpack.iter().map(|(k, v)| (k.clone(), *v)).collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));
        self.out.push_back(ServerMsg::Backpack {
            items,
            chits: self.run_chits,
            gear: self.run_gear.clone(),
        });
    }

    fn handle_text(&mut self, text: &str) {
        let raw: RawEnvelope = match serde_json::from_str(text) {
            Ok(r) => r,
            Err(_) => return,
        };
        match raw.msg_type.as_str() {
            "session.authenticated" => {
                self.phase = Phase::Ready;
                self.out.push_back(ServerMsg::Connected {
                    player_id: self.player_id.clone(),
                });
            }
            "session.error" => {
                if let Ok(e) = serde_json::from_value::<ws::Error>(raw.payload) {
                    self.out.push_back(ServerMsg::Error { message: e.message });
                }
            }
            "run.started" => {
                self.backpack.clear();
                self.run_chits = raw.payload["chits"].as_i64().unwrap_or(0);
                self.run_gear.clear();
                self.run_loot_gear.clear();
                self.out.push_back(ServerMsg::RunGear { gear: Vec::new() });
                if let Some(items) = raw.payload["backpack"].as_array() {
                    for it in items {
                        let kind = it["item_kind"].as_str().unwrap_or("").to_string();
                        let qty = it["quantity"].as_i64().unwrap_or(0) as i32;
                        if !kind.is_empty() {
                            *self.backpack.entry(kind).or_insert(0) += qty;
                        }
                    }
                }
                for g in raw.payload["backpack_gear"].as_array().into_iter().flatten() {
                    let name = g["name"].as_str().unwrap_or("gear").to_string();
                    let atk = g["atk_bonus"].as_i64().unwrap_or(0) as i32;
                    self.run_gear.push((name, atk));
                }
                // This run's terrain offset — ride it to the bin (render module) so the
                // ground shader + entity Y grow the same (per-run-varied) hills.
                let terrain_off = match raw.payload["terrain_offset"].as_array() {
                    Some(a) if a.len() == 2 => (
                        a[0].as_f64().unwrap_or(0.0) as f32,
                        a[1].as_f64().unwrap_or(0.0) as f32,
                    ),
                    _ => (0.0, 0.0),
                };
                // Authored climbable peaks (mountains) — each `[cx, cz, radius, height]`.
                let peaks: Vec<[f32; 4]> = raw.payload["peaks"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| {
                                let a = p.as_array()?;
                                Some([
                                    a.first()?.as_f64()? as f32,
                                    a.get(1)?.as_f64()? as f32,
                                    a.get(2)?.as_f64()? as f32,
                                    a.get(3)?.as_f64()? as f32,
                                ])
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // Whether this WORLD is the guided one — the server's fact, not our own
                // keypress. An older server omits it and it reads false, which is the safe
                // way round: no walkthrough over a run that may not be guided.
                let tutorial = raw.payload["tutorial"].as_bool().unwrap_or(false);
                // CONTINENTS (WG-7): this world's straits, each the eight numbers of
                // `coast::Strait`. An older server omits them and the list reads empty,
                // which is the safe way round — a world with no inland seas, i.e. exactly
                // what the fan was before this shipped.
                let straits: Vec<meld_proto::coast::Strait> = raw.payload["straits"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| {
                                let a = s.as_array()?;
                                let mut out = [0.0f32; 8];
                                for (i, slot) in out.iter_mut().enumerate() {
                                    *slot = a.get(i)?.as_f64()? as f32;
                                }
                                Some(out)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // This world's NAME (CANON D19). `as_u64` rather than `as_f64`: a seed is a
                // full u64 and f64 loses every bit past 2^53, which would quietly hand the
                // player a seed that regenerates a DIFFERENT world than the one they are in.
                let world_seed = raw.payload["world_seed"].as_u64().unwrap_or(0);
                // Bays and isles — four floats each, same shape as `peaks`.
                let lobes: Vec<meld_proto::coast::Lobe> = raw.payload["lobes"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|l| {
                                let a = l.as_array()?;
                                let mut out = [0.0f32; 4];
                                for (i, slot) in out.iter_mut().enumerate() {
                                    *slot = a.get(i)?.as_f64()? as f32;
                                }
                                Some(out)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // Inland water — four floats each, like the lobes.
                let quads = |key: &str| -> Vec<[f32; 4]> {
                    raw.payload[key]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| {
                                    let a = v.as_array()?;
                                    let mut out = [0.0f32; 4];
                                    for (i, slot) in out.iter_mut().enumerate() {
                                        *slot = a.get(i)?.as_f64()? as f32;
                                    }
                                    Some(out)
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                };
                let (basins, rivers) = (quads("basins"), quads("rivers"));
                // Absent on an older server: `Regions::default()` reads `ring_step == 0`,
                // which every reader — the shader included — treats as "no world here".
                let regions: meld_proto::regions::Regions =
                    serde_json::from_value(raw.payload["regions"].clone()).unwrap_or_default();
                self.out.push_back(ServerMsg::RunStarted {
                    terrain_off,
                    peaks,
                    straits,
                    world_seed,
                    lobes,
                    basins,
                    rivers,
                    regions,
                    tutorial,
                });
                self.emit_backpack();
                if let Some(pts) = raw.payload["path"].as_array() {
                    let points: Vec<(f64, f64)> = pts
                        .iter()
                        .filter_map(|p| Some((p["x"].as_f64()?, p["y"].as_f64()?)))
                        .collect();
                    if !points.is_empty() {
                        self.out.push_back(ServerMsg::WorldPath { points });
                    }
                }
                // The web of extra trails: each edge is a `[ {x,y}, {x,y} ]` pair.
                if let Some(edges_json) = raw.payload["web"].as_array() {
                    let edges: Vec<((f64, f64), (f64, f64))> = edges_json
                        .iter()
                        .filter_map(|e| {
                            let a = e.get(0)?;
                            let b = e.get(1)?;
                            Some((
                                (a["x"].as_f64()?, a["y"].as_f64()?),
                                (b["x"].as_f64()?, b["y"].as_f64()?),
                            ))
                        })
                        .collect();
                    if !edges.is_empty() {
                        self.out.push_back(ServerMsg::WorldWeb { edges });
                    }
                }
                // Map bounds + biome seams → the client frames the map with walls.
                if let Some(b) = raw.payload["bounds"].as_object() {
                    let seams = raw.payload["seams"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .map(|s| SeamLine {
                                    x: s["x"].as_f64().unwrap_or(0.0),
                                    gap_y: s["gap_y"].as_f64().unwrap_or(0.0),
                                    gap_half_width: s["gap_half_width"].as_f64().unwrap_or(4.0),
                                    biome_from: s["biome_from"].as_str().unwrap_or("").to_string(),
                                    biome_to: s["biome_to"].as_str().unwrap_or("").to_string(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    self.out.push_back(ServerMsg::WorldFrame {
                        x_min: b["x_min"].as_f64().unwrap_or(-4.0),
                        x_max: b["x_max"].as_f64().unwrap_or(0.0),
                        lateral: b["lateral"].as_f64().unwrap_or(28.0),
                        west_return_border: b["west_return_border"].as_f64().unwrap_or(-2.5),
                        radial_arc_degrees: b["radial_arc_degrees"].as_f64().unwrap_or(0.0),
                        seams,
                    });
                }
            }
            "run.pouches" => {
                let cap = raw.payload["pouches"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|p| p["capacity"].as_i64())
                    .unwrap_or(0) as i32;
                let mut pouches: Vec<Vec<(String, i32)>> = Vec::new();
                for p in raw.payload["pouches"].as_array().into_iter().flatten() {
                    let slot = p["hero_slot"].as_i64().unwrap_or(0).max(0) as usize;
                    let items: Vec<(String, i32)> = p["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|i| {
                            let k = i["item_kind"].as_str()?.to_string();
                            Some((k, i["quantity"].as_i64().unwrap_or(0) as i32))
                        })
                        .collect();
                    if pouches.len() <= slot {
                        pouches.resize(slot + 1, Vec::new());
                    }
                    pouches[slot] = items;
                }
                self.out.push_back(ServerMsg::Pouches { pouches, capacity: cap });
            }
            "run.backpack_update" => {
                // A chest-open pays out in the same message shape as any other
                // backpack change (kill loot, town-portal drops); `cause` tags
                // which one this is, so the chest report only fires for chests.
                let mut is_chest = false;
                let mut chest_items: Vec<(String, i32)> = Vec::new();
                // What a felled creature LEFT ON THE GROUND and we just walked over
                // (`cause: pickup:<kind>`, `CR-2`). Auto-collected, which used to mean
                // silently: the only trace was a counter ticking somewhere off-screen,
                // so a kill that dropped something read exactly like one that did not.
                // It pays out through the same report the chest does, because it is the
                // same question — "what did I just get".
                let mut picked_up: Vec<(String, i32)> = Vec::new();
                // Units a harvest channel just paid out, so the overworld can pop
                // "+1 Bog Myrrh" over the player's head. The bar filling is only half the
                // feedback; this is the half that says what you actually got.
                let mut harvested: Vec<(String, i32)> = Vec::new();
                for ch in raw.payload["changes"].as_array().into_iter().flatten() {
                    let kind = ch["item"]["item_kind"].as_str().unwrap_or("").to_string();
                    let qty = ch["item"]["quantity"].as_i64().unwrap_or(0) as i32;
                    if kind.is_empty() {
                        continue;
                    }
                    let signed = if ch["delta"].as_str() == Some("removed") { -qty } else { qty };
                    // ONE place decides what a `cause` is worth showing, so the three
                    // payout surfaces cannot disagree about which changes are a payout.
                    match (signed > 0).then(|| payout_of(ch["cause"].as_str().unwrap_or(""))).flatten() {
                        Some(Payout::Chest) => {
                            is_chest = true;
                            chest_items.push((kind.clone(), signed));
                        }
                        Some(Payout::Harvest) => harvested.push((kind.clone(), signed)),
                        Some(Payout::Pickup) => picked_up.push((kind.clone(), signed)),
                        None => {}
                    }
                    let e = self.backpack.entry(kind).or_insert(0);
                    *e += signed;
                    if *e <= 0 {
                        let k = ch["item"]["item_kind"].as_str().unwrap_or("").to_string();
                        self.backpack.remove(&k);
                    }
                }
                let chits_delta = raw.payload["chits_delta"].as_i64().unwrap_or(0);
                self.run_chits += chits_delta;
                if self.run_chits < 0 {
                    self.run_chits = 0;
                }
                let mut chest_gear: Vec<(String, meld_proto::enums::Insurance)> = Vec::new();
                for g in raw.payload["gear_added"].as_array().into_iter().flatten() {
                    let name = g["name"].as_str().unwrap_or("gear").to_string();
                    if is_chest {
                        // An unparseable word reads as Ephemeral, the same way the gear
                        // tooltip resolves it: believing a temporary piece is safe costs
                        // the player the piece, and the reverse costs them nothing.
                        let ins = g["insurance"]
                            .as_str()
                            .and_then(meld_proto::enums::Insurance::from_wire)
                            .unwrap_or(meld_proto::enums::Insurance::Ephemeral);
                        chest_gear.push((name.clone(), ins));
                    }
                    let atk = g["atk_bonus"].as_i64().unwrap_or(0) as i32;
                    self.run_gear.push((name, atk));
                }
                self.emit_backpack();
                for (kind, qty) in harvested {
                    self.out.push_back(ServerMsg::Harvested { kind, qty });
                }
                if is_chest {
                    self.out.push_back(ServerMsg::ChestOpened {
                        chits: chits_delta,
                        items: chest_items,
                        gear: chest_gear,
                    });
                }
                // The spoils of a fight we did not have: one report per pickup EVENT, not
                // per stack — the server hands over everything in range in a single
                // message, so a body that dropped three things is one banner.
                if !picked_up.is_empty() {
                    self.out.push_back(ServerMsg::LootPickedUp { items: picked_up });
                }
            }
            "run.gear" => {
                let gear: Vec<GearLine> = raw.payload["gear"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|g| GearLine {
                        gear_id: g["gear_id"].as_str().unwrap_or("").to_string(),
                        name: g["name"].as_str().unwrap_or("?").to_string(),
                        slot: g["slot"].as_str().unwrap_or("").to_string(),
                        class_key: g["class_key"].as_str().unwrap_or("").to_string(),
                        insurance: g["insurance"].as_str().unwrap_or("ephemeral").to_string(),
                        family: g["family"].as_str().unwrap_or("").to_string(),
                        armor_weight: g["armor_weight"].as_str().unwrap_or("").to_string(),
                        affixes: serde_json::from_value(g["affixes"].clone()).unwrap_or_default(),
                        unique_key: g["unique_key"].as_str().unwrap_or("").to_string(),
                        set_key: g["set_key"].as_str().unwrap_or("").to_string(),
                        tier: g["tier"].as_i64().unwrap_or(0) as i32,
                        equipped_hero_slot: g["equipped_hero_slot"].as_i64().map(|s| s as usize),
                        max_durability: g["max_durability"].as_i64().unwrap_or(0) as i32,
                        base_max_durability: g["base_max_durability"].as_i64().unwrap_or(0) as i32,
                        atk_bonus: g["atk_bonus"].as_i64().unwrap_or(0) as i32,
                        def_bonus: g["def_bonus"].as_i64().unwrap_or(0) as i32,
                        spd_bonus: g["spd_bonus"].as_i64().unwrap_or(0) as i32,
                        reroll_cost: g["reroll_cost"].as_i64().unwrap_or(0) as i32,
                    })
                    .collect();
                self.run_loot_gear = gear.clone();
                self.out.push_back(ServerMsg::RunGear { gear });
            }
            "run.party" => {
                let heroes = raw.payload["heroes"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|h| HeroLine {
                                name: h["name"].as_str().unwrap_or("Hero").to_string(),
                                class_key: h["class_key"].as_str().unwrap_or("explorer").to_string(),
                                level: h["level"].as_i64().unwrap_or(1) as i32,
                                str_: h["str_"].as_i64().unwrap_or(0) as i32,
                                mnd: h["mnd"].as_i64().unwrap_or(0) as i32,
                                dex: h["dex"].as_i64().unwrap_or(0) as i32,
                                wll: h["wll"].as_i64().unwrap_or(0) as i32,
                                max_hp: h["max_hp"].as_i64().unwrap_or(0) as i32,
                                hp: h["hp"].as_i64().unwrap_or(0) as i32,
                                xp: h["xp"].as_i64().unwrap_or(0),
                                xp_to_next: h["xp_to_next"].as_i64().unwrap_or(0),
                                back_row: h["back_row"].as_bool().unwrap_or(false),
                                afflictions: h["afflictions"]
                                    .as_array()
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(|s| s.as_str().map(String::from))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let depth = |key: &str, detail_key: &str| -> Vec<DepthLine> {
                    raw.payload[key]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .map(|d| DepthLine {
                                    name: d["name"].as_str().unwrap_or("").to_string(),
                                    detail: d[detail_key].as_str().unwrap_or("").to_string(),
                                    description: d["description"].as_str().unwrap_or("").to_string(),
                                    bonus_pct: d["bonus_pct"].as_i64().unwrap_or(0) as i32,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                };
                let synergies = depth("synergies", "effect");
                let combos = depth("combos", "sequence");
                let abilities = raw.payload["abilities"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|a| {
                                (
                                    a["key"].as_str().unwrap_or_default().to_string(),
                                    a["effect"].as_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let ability_costs = raw.payload["abilities"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|a| {
                                let cost = a["adrenaline_cost"].as_i64()?;
                                Some((a["key"].as_str().unwrap_or_default().to_string(), cost as i32))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.out.push_back(ServerMsg::Party {
                    heroes,
                    synergies,
                    combos,
                    abilities,
                    ability_costs,
                });
            }
            "run.perks" => {
                let p = &raw.payload;
                let f = |k: &str| p[k].as_f64().unwrap_or(0.0) as f32;
                let u = |k: &str| p[k].as_u64().unwrap_or(0) as u8;
                let perks = PerksLine {
                    explorer_glow: f("explorer_glow"),
                    hunter_intel: u("hunter_intel"),
                    explorer_map: u("explorer_map"),
                    explorer_map_radius: f("explorer_map_radius"),
                    shifter_dungeon_radius: f("shifter_dungeon_radius"),
                    shifter_item_sense: p["shifter_item_sense"].as_bool().unwrap_or(false),
                    shifter_trap_radius: f("shifter_trap_radius"),
                    hunter_threat: u("hunter_threat"),
                    hunter_reveal_radius: f("hunter_reveal_radius"),
                    smithwright_ore_radius: f("smithwright_ore_radius"),
                    keeper_reagent_radius: f("keeper_reagent_radius"),
                    resonant_regen: f("resonant_regen"),
                    psyker_hold_targets: u("psyker_hold_targets"),
                    psyker_hold_seconds: f("psyker_hold_seconds"),
                    psyker_hold_cooldown: f("psyker_hold_cooldown"),
                    psyker_hold_radius: f("psyker_hold_radius"),
                    psyker_mind_link: p
                        .get("psyker_mind_link")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    // Neutral default is 1.0 (no Phoenix Guard), not 0.0.
                    phoenix_guard_aggro_mult: p["phoenix_guard_aggro_mult"].as_f64().unwrap_or(1.0) as f32,
                };
                self.out.push_back(ServerMsg::Perks { perks });
            }
            "run.hunt_progress" => {
                let p = &raw.payload;
                self.out.push_back(ServerMsg::HuntProgress {
                    name: p["name"].as_str().unwrap_or("A hunt").to_string(),
                    progress: p["progress"].as_i64().unwrap_or(0) as i32,
                    target: p["target"].as_i64().unwrap_or(1) as i32,
                    complete: p["complete"].as_bool().unwrap_or(false),
                });
            }
            "movement.position_correction" => {
                let p = &raw.payload["position"];
                self.out.push_back(ServerMsg::PositionCorrection {
                    x: p["x"].as_f64().unwrap_or(0.0),
                    y: p["y"].as_f64().unwrap_or(0.0),
                });
            }
            "world.shift_held" => {
                let anchors: Vec<ww::HeldAnchor> =
                    serde_json::from_value(raw.payload["anchors"].clone()).unwrap_or_default();
                self.out.push_back(ServerMsg::ShiftHeld { anchors });
            }
            "world.shift_warning" => {
                let p = &raw.payload;
                self.out.push_back(ServerMsg::ShiftWarning {
                    inner_radius: p["inner_radius"].as_f64().unwrap_or(0.0),
                    outer_radius: p["outer_radius"].as_f64().unwrap_or(0.0),
                    biome: p["biome"].as_str().unwrap_or("").to_string(),
                    lands_in_ms: p["lands_in_ms"].as_u64().unwrap_or(0),
                    caught: p["caught"].as_bool().unwrap_or(false),
                });
            }
            "world.shift" => {
                let p = &raw.payload;
                self.out.push_back(ServerMsg::Shifted {
                    biome: p["biome"].as_str().unwrap_or("").to_string(),
                    from_biome: p["from_biome"].as_str().unwrap_or("").to_string(),
                    damage: p["damage"]
                        .as_array()
                        .map(|a| a.iter().map(|v| v.as_i64().unwrap_or(0) as i32).collect())
                        .unwrap_or_default(),
                });
            }
            "run.unlocked" => {
                let newly = raw.payload["unlocks"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|u| UnlockLine {
                                key: u["key"].as_str().unwrap_or_default().to_string(),
                                name: u["name"].as_str().unwrap_or("Unlocked").to_string(),
                                kind: u["kind"].as_str().unwrap_or("class").to_string(),
                                class_key: u["class_key"].as_str().map(str::to_string),
                                slot: u["slot"].as_i64().map(|n| n as i32),
                                trigger_text: u["trigger_text"].as_str().unwrap_or_default().to_string(),
                                banner: u["banner"].as_str().unwrap_or_default().to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.out.push_back(ServerMsg::Unlocked {
                    newly,
                    owned: raw.payload["owned"]
                        .as_array()
                        .map(|a| {
                            a.iter().filter_map(|v| v.as_str()).map(str::to_string).collect()
                        })
                        .unwrap_or_default(),
                    party_slots: raw.payload["party_slots"].as_i64().unwrap_or(1) as i32,
                    banner: raw.payload["banner"].as_bool().unwrap_or(false),
                    deepest_ever: raw.payload["deepest_ever"].as_i64().unwrap_or(0) as i32,
                });
            }
            "onboarding.status" => {
                self.out.push_back(ServerMsg::OnboardingStatus {
                    town_seen: raw.payload["town_seen"].as_bool().unwrap_or(false),
                    run_seen: raw.payload["run_seen"].as_bool().unwrap_or(false),
                });
            }
            "run.level_up" => {
                let pair = |h: &Value, key: &str| {
                    (
                        h[format!("{key}_before")].as_i64().unwrap_or(0) as i32,
                        h[format!("{key}_after")].as_i64().unwrap_or(0) as i32,
                    )
                };
                let heroes = raw.payload["heroes"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|h| HeroLevelUpLine {
                                name: h["name"].as_str().unwrap_or("Hero").to_string(),
                                class_key: h["class_key"].as_str().unwrap_or("explorer").to_string(),
                                level: h["level"].as_i64().unwrap_or(1) as i32,
                                max_hp: pair(h, "max_hp"),
                                str_: pair(h, "str"),
                                mnd: pair(h, "mnd"),
                                dex: pair(h, "dex"),
                                wll: pair(h, "wll"),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.out.push_back(ServerMsg::LevelUp {
                    new_run_level: raw.payload["new_run_level"].as_i64().unwrap_or(1) as i32,
                    levels_gained: raw.payload["levels_gained"].as_i64().unwrap_or(1) as i32,
                    heroes,
                });
            }
            "lobby.state" => {
                let members = raw.payload["members"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|m| {
                                (
                                    m["player_id"].as_str().unwrap_or("").to_string(),
                                    m["username"].as_str().unwrap_or("").to_string(),
                                    m["ready"].as_bool().unwrap_or(false),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.out.push_back(ServerMsg::LobbyState {
                    code: raw.payload["code"].as_str().unwrap_or("").to_string(),
                    host: raw.payload["host_player_id"].as_str().unwrap_or("").to_string(),
                    members,
                });
            }
            "lobby.closed" => self.out.push_back(ServerMsg::LobbyClosed),
            "world.snapshot" => {
                if let Ok(s) = serde_json::from_value::<wm::Snapshot>(raw.payload) {
                    let entities = s
                        .entities
                        .into_iter()
                        .map(|e| {
                            // Server tags monsters `mob:<kind>:<faction>`, the portal
                            // `portal`, and players with their avatar state (`active`, …).
                            let mut radius = 0.0;
                            let mut bodies_required: u8 = 1;
                            let mut opened = false;
                            let mut chest_tier = 0;
                            let mut quarry = false;
                            let mut expects_parties = 0u8;
                            let mut held = false;
                            let mut clashing = false;
                            let mut boss: Option<String> = None;
                            let (kind, monster_kind, faction) = match e.avatar_state.as_deref() {
                                Some("portal") => (EntityKind::Portal, None, None),
                                Some("stair") => (EntityKind::Stair, None, None),
                                Some(s) if s.starts_with("trap:") => (
                                    EntityKind::Trap,
                                    Some(s["trap:".len()..].to_string()),
                                    None,
                                ),
                                Some(s) if s.starts_with("chest:") => {
                                    // chest:<tier>:<open>
                                    opened = s.ends_with(":1");
                                    chest_tier = s["chest:".len()..]
                                        .split(':')
                                        .next()
                                        .and_then(|t| t.parse().ok())
                                        .unwrap_or(0);
                                    (EntityKind::Chest, None, None)
                                }
                                Some(s) if s.starts_with("mob:") => {
                                    let t = parse_mob_state(s);
                                    quarry = t.quarry;
                                    held = t.held;
                                    clashing = t.clashing;
                                    boss = t.boss.map(str::to_string);
                                    expects_parties = t.parties;
                                    (
                                        EntityKind::Monster,
                                        Some(t.kind.to_string()),
                                        (!t.faction.is_empty()).then(|| t.faction.to_string()),
                                    )
                                }
                                Some(s) if s.starts_with("resource:") => {
                                    (EntityKind::Resource, Some(s["resource:".len()..].to_string()), None)
                                }
                                Some(s) if s.starts_with("loot:") => {
                                    (EntityKind::Loot, Some(s["loot:".len()..].to_string()), None)
                                }
                                Some(s) if s.starts_with("obstacle:") => {
                                    // obstacle:<kind>:<radius>
                                    let rest = &s["obstacle:".len()..];
                                    let (k, r) = rest.rsplit_once(':').unwrap_or((rest, "1"));
                                    radius = r.parse().unwrap_or(1.0);
                                    (EntityKind::Obstacle, Some(k.to_string()), None)
                                }
                                Some(s) if s.starts_with("station:") => {
                                    // station:<kind>:<uses_left> — the remaining jobs
                                    // ride `bodies_required`, the existing "how many"
                                    // field, rather than growing the wire a number that
                                    // only one tag uses.
                                    let rest = &s["station:".len()..];
                                    let (k, u) = rest.rsplit_once(':').unwrap_or((rest, "0"));
                                    bodies_required = u.parse().unwrap_or(0);
                                    (EntityKind::Station, Some(k.to_string()), None)
                                }
                                Some(s) if s.starts_with("structure:") => {
                                    // structure:<function>:<hp_pct>:<building>
                                    let mut it = s["structure:".len()..].split(':');
                                    let f = it.next().unwrap_or("").to_string();
                                    bodies_required = it.next().and_then(|v| v.parse().ok()).unwrap_or(100);
                                    // Still going up rides `opened`, the existing
                                    // "is it in its other state" flag, rather than
                                    // growing the wire a bool one tag uses.
                                    opened = it.next() == Some("1");
                                    (EntityKind::Structure, Some(f), None)
                                }
                                Some(s) if s.starts_with("entrance:") => {
                                    // entrance:<dungeon>:<bodies>
                                    let rest = &s["entrance:".len()..];
                                    let (n, b) = rest.rsplit_once(':').unwrap_or((rest, "1"));
                                    bodies_required = b.parse().unwrap_or(1);
                                    (EntityKind::Entrance, Some(n.to_string()), None)
                                }
                                _ => (EntityKind::Player, None, None),
                            };
                            let battling = matches!(kind, EntityKind::Player)
                                && e.avatar_state.as_deref() == Some("in_battle");
                            let is_mob = matches!(kind, EntityKind::Monster);
                            EntityView {
                                id: e.entity_id,
                                x: e.position.x,
                                y: e.position.y,
                                kind,
                                monster_kind,
                                faction,
                                radius,
                                battling,
                                level: e.level.unwrap_or(0),
                                opened,
                                chest_tier,
                                mob_level: is_mob.then_some(e.mob_level).flatten(),
                                hp: is_mob.then_some(e.hp).flatten(),
                                max_hp: is_mob.then_some(e.max_hp).flatten(),
                                encounter_class: if is_mob { e.encounter_class } else { None },
                                aggression: if is_mob { e.aggression } else { None },
                                quarry,
                                expects_parties,
                                held,
                                boss,
                                clashing,
                                bodies_required,
                            }
                        })
                        .collect();
                    self.out.push_back(ServerMsg::Snapshot { entities });
                }
            }
            "world.terrain_section" => {
                if let Ok(t) = serde_json::from_value::<ww::TerrainSection>(raw.payload) {
                    let section = TerrainSectionView {
                        index: t.index,
                        start_x: t.start_x,
                        end_x: t.end_x,
                        y_min: t.y_min,
                        cell: t.cell,
                        cols: t.cols,
                        rows: t.rows,
                        levels: t.levels,
                        connectors: t
                            .connectors
                            .into_iter()
                            .map(|c| ConnectorView {
                                kind: c.kind,
                                x: c.position.x,
                                y: c.position.y,
                                lo: c.lo,
                                hi: c.hi,
                                radius: c.radius,
                            })
                            .collect(),
                        path: t.path.into_iter().map(|p| (p.x, p.y)).collect(),
                        biome: t.biome,
                        radial_half: t.radial_half,
                        corridor_lateral: t.corridor_lateral,
                        peaks: t.peaks,
                        straits: t.straits,
                        lobes: t.lobes,
                        basins: t.basins,
                        rivers: t.rivers,
                    };
                    self.out.push_back(ServerMsg::TerrainSection { section });
                }
            }
            "world.dungeon_scene" => {
                if let Ok(s) = serde_json::from_value::<ww::DungeonScene>(raw.payload) {
                    self.out.push_back(ServerMsg::DungeonScene {
                        active: s.active,
                        theme: s.theme,
                        floor: s.floor,
                        width: s.width,
                        height: s.height,
                    });
                }
            }
            "battle.started" => {
                if let Ok(b) = serde_json::from_value::<wb::Started>(raw.payload) {
                    let mut combatants: Vec<CombatantView> =
                        b.allies.iter().map(CombatantView::from_wire).collect();
                    combatants.extend(b.enemies.iter().map(CombatantView::from_wire));
                    let monster_combatant = b.enemies.first().map(|c| c.combatant_id.clone());
                    let your_combatant_ids = if b.your_combatant_ids.is_empty() {
                        vec![b.your_combatant_id.clone()]
                    } else {
                        b.your_combatant_ids.clone()
                    };
                    self.out.push_back(ServerMsg::BattleStarted {
                        battle_id: b.battle_id,
                        your_combatant_id: b.your_combatant_id,
                        // A watcher controls nothing, so the back-compat fallback above
                        // (empty ⇒ `vec![your_combatant_id]`) must not apply: it would
                        // hand them one hero id of `""` and a menu aimed at nobody.
                        your_combatant_ids: if b.spectating { Vec::new() } else { your_combatant_ids },
                        combatants,
                        monster_combatant,
                        spectating: b.spectating,
                    });
                }
            }
            "battle.watch_ended" => {
                self.out.push_back(ServerMsg::WatchEnded {
                    battle_id: raw.payload["battle_id"].as_str().unwrap_or("").to_string(),
                    reason: raw.payload["reason"].as_str().unwrap_or("finished").to_string(),
                });
            }
            "battle.action_resolved" => {
                if let Ok(r) = serde_json::from_value::<wb::ActionResolved>(raw.payload) {
                    let action = serde_json::to_value(r.action)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default();
                    let effects = r
                        .effects
                        .into_iter()
                        .map(|e| {
                            let kind = serde_json::to_value(e.kind)
                                .ok()
                                .and_then(|v| v.as_str().map(String::from))
                                .unwrap_or_default();
                            let modifier = e.modifier_flag.and_then(|m| {
                                serde_json::to_value(m)
                                    .ok()
                                    .and_then(|v| v.as_str().map(String::from))
                            });
                            HitEffect {
                                target: e.target_id,
                                kind,
                                crit: e.status.as_deref() == Some("crit"),
                                amount: e.amount,
                                hp_after: e.hp_after,
                                modifier,
                            }
                        })
                        .collect();
                    self.out.push_back(ServerMsg::ActionResolved {
                        actor: r.actor_id,
                        action,
                        callout: r.callout_text,
                        effects,
                    });
                }
            }
            "battle.telegraph_started" => {
                if let Ok(t) = serde_json::from_value::<wb::TelegraphStarted>(raw.payload) {
                    self.out.push_back(ServerMsg::Telegraph {
                        combatant_id: t.combatant_id,
                        text: t.callout_text,
                    });
                }
            }
            "battle.turn_ready" => {
                if let Ok(t) = serde_json::from_value::<wb::TurnReady>(raw.payload) {
                    self.out.push_back(ServerMsg::TurnReady {
                        combatant_id: t.combatant_id,
                    });
                }
            }
            "battle.party_joined" => {
                if let Ok(p) = serde_json::from_value::<wb::PartyJoined>(raw.payload) {
                    let combatants = p.joining_allies.iter().map(CombatantView::from_wire).collect();
                    self.out.push_back(ServerMsg::CombatantsJoined { combatants });
                }
            }
            // CR-11: a pack leader called and the overworld answered. The same door the
            // ally merge comes through — `is_player` is what puts a combatant on a side —
            // but it carries WHO called, so the arrival can be announced instead of three
            // creatures appearing out of nowhere.
            "battle.reinforcements" => {
                if let Ok(p) = serde_json::from_value::<wb::Reinforcements>(raw.payload) {
                    let combatants: Vec<CombatantView> =
                        p.joining_enemies.iter().map(CombatantView::from_wire).collect();
                    let n = combatants.len();
                    self.out.push_back(ServerMsg::CombatantsJoined { combatants });
                    self.out.push_back(ServerMsg::Reinforcements {
                        called_by: p.called_by,
                        arrived: n,
                    });
                }
            }
            "battle.gauge_update" => {
                if let Ok(g) = serde_json::from_value::<wb::GaugeUpdate>(raw.payload) {
                    let updates = g
                        .combatants
                        .into_iter()
                        .map(|c| (c.combatant_id, c.gauge, c.hp, c.statuses))
                        .collect();
                    self.out.push_back(ServerMsg::Gauge { updates });
                }
            }
            "battle.ended" => {
                if let Ok(e) = serde_json::from_value::<wb::Ended>(raw.payload) {
                    let outcome = serde_json::to_value(e.outcome)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_else(|| "over".to_string());
                    // Our own XP award, if any (the payload lists every participant).
                    let xp = e
                        .xp_awards
                        .iter()
                        .find(|a| a.player_id == self.player_id)
                        .map(|a| a.xp)
                        .unwrap_or(0);
                    let items = e
                        .loot
                        .into_iter()
                        .map(|i| (i.item_kind, i.quantity))
                        .collect();
                    // Carry the insurance with the name: the tally is the last thing a
                    // player reads before deciding what to take back out (`GR-6`).
                    let gear_drops =
                        e.gear_drops.into_iter().map(|g| (g.name, g.insurance)).collect();
                    let worn = e
                        .gear_worn
                        .into_iter()
                        .map(|w| (w.hero_name, w.durability_lost, w.ephemeral_burned))
                        .collect();
                    self.out.push_back(ServerMsg::BattleEnded {
                        outcome,
                        xp,
                        chits: e.chits_found,
                        items,
                        gear_drops,
                        worn,
                    });
                }
            }
            "run.channel_started" => {
                if let Ok(c) = serde_json::from_value::<wr::ChannelStarted>(raw.payload) {
                    self.out.push_back(ServerMsg::ChannelStarted {
                        completes_at: c.completes_at,
                        fill_ms: c.fill_ms,
                        method: c.method,
                    });
                }
            }
            "run.channel_interrupted" => self.out.push_back(ServerMsg::ChannelInterrupted),
            "run.tempo_started" => {
                if let Ok(t) = serde_json::from_value::<wr::TempoStarted>(raw.payload) {
                    self.out.push_back(ServerMsg::TempoStarted {
                        job_id: t.job_id,
                        service: t.service,
                        strikes: t.strikes,
                        sweep_ms: t.sweep_ms,
                        bands: t.bands.iter().map(|b| (b.lo, b.hi)).collect(),
                    });
                }
            }
            "run.smith_result" => {
                if let Ok(r) = serde_json::from_value::<wr::SmithResult>(raw.payload) {
                    // The gear itself changed in the Vault, so the inventory the bench
                    // is reading has to catch up before the next keypress.
                    if r.ok {
                        self.fetch_inventory();
                    }
                    self.out.push_back(ServerMsg::SmithResult {
                        message: r.message,
                        ok: r.ok,
                        uses_left: r.uses_left,
                    });
                }
            }
            "run.member_result" => {
                if let Ok(m) = serde_json::from_value::<wr::MemberResult>(raw.payload) {
                    // Only our own copy carries `banked`; others are notifications.
                    if m.player_id == self.player_id {
                        let result = serde_json::to_value(m.result)
                            .ok()
                            .and_then(|v| v.as_str().map(String::from))
                            .unwrap_or_default();
                        let banked = m.banked.map(|b| b.len()).unwrap_or(0);
                        self.out.push_back(ServerMsg::RunEnded {
                            result,
                            banked,
                            chits: m.chits,
                            gear: m.gear_banked.len(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// GET `/v1/vault` then `/v1/vault/gear` (Bearer auth) and deliver the combined
/// (chits, materials, gear) tuple on `tx`. Shared by the initial inventory open
/// and the post-equip refresh.
/// Turn a craft reply into one line for the panel. A refusal is reported as loudly as
/// a success: the server's own message ("Insufficient materials (need 2 heartoak_bark)",
/// "alchemy level 1 is below the required level 9") is already the right sentence.
/// What KIND of payout a `run.backpack_update` change is, when it is one at all.
///
/// The wire carries every backpack delta down one message — a chest, a harvest tick,
/// ground loot walked over, a spend, a drop on death — and the `cause` string is the only
/// thing that tells them apart. So the mapping lives HERE, once, rather than as three
/// `starts_with` checks scattered down the parse: they were already subtly different from
/// each other (`== "chest"` against `starts_with("harvest")`), which is how a fourth
/// payout gets added and shows up on none of the surfaces that were supposed to greet it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Payout {
    /// A treasure chest was opened: the loot-report banner.
    Chest,
    /// A harvest channel paid out a unit: the over-head "+1 Bog Myrrh" pop.
    Harvest,
    /// Ground loot a felled creature left behind, walked over and banked (`CR-2`): the
    /// same banner the chest raises, because it answers the same question.
    Pickup,
}

/// The payout a backpack change's `cause` announces, or `None` when the change is
/// bookkeeping the player has already watched happen (a spend, a craft, a death).
pub(crate) fn payout_of(cause: &str) -> Option<Payout> {
    match cause {
        "chest" => Some(Payout::Chest),
        c if c.starts_with("harvest") => Some(Payout::Harvest),
        c if c.starts_with("pickup:") => Some(Payout::Pickup),
        _ => None,
    }
}

/// What a monster's `avatar_state` tag says about it.
#[derive(Debug, Default, PartialEq, Eq)]
struct MobTag<'a> {
    /// Creature content id — the HOST creature, even for a boss.
    kind: &'a str,
    /// Creature faction; drives the colour.
    faction: &'a str,
    /// FS-4: which named boss this is, if it is one — the identity a Gatekeeper, an
    /// end-fight peer or a bounty mark actually fights and renders as.
    boss: Option<&'a str>,
    quarry: bool,
    held: bool,
    clashing: bool,
    /// How many PARTIES this fight is sized for, when that is more than one (`FS-4`).
    /// Absent on everything ordinary, so a plate only ever appears where it is a warning.
    parties: u8,
}

/// Split a monster's `avatar_state` — `mob:<kind>:<faction>[:token…]` — into its parts.
///
/// The trailing tokens are optional, some are per-viewer (AD-4), and there can be
/// SEVERAL: a pinned creature can also be a quarry, and either can also be mid-clash
/// (`CR-2`) and a named boss (FS-4). So every trailing part is read, not just the first
/// — reading only the first meant the second marker vanished depending on which one
/// happened to be appended earlier, which is the kind of bug that looks like a rendering
/// glitch. (And this is one function because reading the faction with a `split_once`
/// swallowed `hostile:quarry` whole.)
///
/// A token may be a bare flag (`held`) or a `key:value` pair (`boss:ironmaw`), like the
/// combatant `statuses` tokens it mirrors — so a value is CONSUMED by its key rather than
/// being read as a flag of its own.
fn parse_mob_state(state: &str) -> MobTag<'_> {
    let mut parts = state.strip_prefix("mob:").unwrap_or(state).split(':');
    let kind = parts.next().unwrap_or("");
    let faction = parts.next().unwrap_or("");
    let mut tag = MobTag { kind, faction, ..Default::default() };
    while let Some(m) = parts.next() {
        match m {
            "quarry" => tag.quarry = true,
            "held" => tag.held = true,
            "clash" => tag.clashing = true,
            "boss" => tag.boss = parts.next().filter(|k| !k.is_empty()),
            "parties" => {
                tag.parties = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0)
            }
            _ => {}
        }
    }
    tag
}

/// GET the Den's board and hand it back over `tx`.
fn spawn_bounties_fetch(base: String, token: String, tx: mpsc::Sender<BountyBoard>) {
    let mut req = ehttp::Request::get(format!("{base}/v1/bounties"));
    req.headers.insert("Authorization", format!("Bearer {token}"));
    ehttp::fetch(req, move |res| {
        let _ = tx.send(bounty_board(&res));
    });
}

fn bounty_board(res: &Result<ehttp::Response, String>) -> BountyBoard {
    let Some(v) = reply_json(res) else {
        return BountyBoard::default();
    };
    let lines = |key: &str| -> Vec<BountyLine> {
        v[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|b| BountyLine {
                bounty_id: b["bounty_id"].as_str().unwrap_or("").to_string(),
                state: b["state"].as_str().unwrap_or("active").to_string(),
                mark_name: b["mark_name"].as_str().unwrap_or("A mark").to_string(),
                boss_kind: b["boss_kind"].as_str().unwrap_or("").to_string(),
                distance: b["distance"].as_i64().unwrap_or(0) as i32,
                venue: b["venue"].as_str().unwrap_or("overworld").to_string(),
                where_to_look: b["where_to_look"].as_str().unwrap_or("").to_string(),
                power: b["power"].as_f64().unwrap_or(1.0),
                expires_in_secs: b["expires_in_secs"].as_i64().unwrap_or(0),
                reward_chits: b["reward_chits"].as_i64().unwrap_or(0),
                reward_material: b["reward_material"].as_str().unwrap_or("").to_string(),
                reward_material_qty: b["reward_material_qty"].as_i64().unwrap_or(0) as i32,
                reward_gear: b["reward_gear"].as_bool().unwrap_or(false),
                reward_rank_xp: b["reward_rank_xp"].as_i64().unwrap_or(0),
            })
            .collect()
    };
    BountyBoard {
        rank: v["rank"].as_i64().unwrap_or(0) as i32,
        rank_title: v["rank_title"].as_str().unwrap_or("Unblooded").to_string(),
        rank_xp_to_next: v["rank_xp_to_next"].as_i64().unwrap_or(0),
        active: lines("active"),
        history: lines("history"),
    }
}

/// What the Den said when you asked to be paid — its own words on a refusal.
fn bounty_claim_text(res: &Result<ehttp::Response, String>) -> String {
    let Some(v) = reply_json(res) else {
        return "the Den did not answer".to_string();
    };
    if let Some(msg) = v["error"]["message"].as_str() {
        return msg.to_string();
    }
    let mut line = format!(
        "the Den pays {}c for {}",
        v["reward_chits"].as_i64().unwrap_or(0),
        v["mark_name"].as_str().unwrap_or("the mark")
    );
    if let Some(gear) = v["reward_gear"].as_str().filter(|g| !g.is_empty()) {
        line.push_str(&format!(", and {gear}"));
    }
    if v["ranked_up"].as_bool().unwrap_or(false) {
        line.push_str(&format!(
            " - hunter rank {} ({})",
            v["rank"].as_i64().unwrap_or(0),
            v["rank_title"].as_str().unwrap_or("")
        ));
    }
    line
}

/// GET the Hunt Board and hand the rows back over `tx`.
fn spawn_hunts_fetch(base: String, token: String, tx: mpsc::Sender<Vec<HuntLine>>) {
    let mut req = ehttp::Request::get(format!("{base}/v1/hunts"));
    req.headers.insert("Authorization", format!("Bearer {token}"));
    ehttp::fetch(req, move |res| {
        let _ = tx.send(hunt_lines(&res));
    });
}

fn hunt_lines(res: &Result<ehttp::Response, String>) -> Vec<HuntLine> {
    let Some(v) = reply_json(res) else {
        return Vec::new();
    };
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|h| HuntLine {
            key: h["key"].as_str().unwrap_or("").to_string(),
            name: h["name"].as_str().unwrap_or("?").to_string(),
            objective: h["objective"].as_str().unwrap_or("").to_string(),
            blurb: h["blurb"].as_str().unwrap_or("").to_string(),
            progress: h["progress"].as_i64().unwrap_or(0) as i32,
            target: h["target"].as_i64().unwrap_or(1) as i32,
            claimable: h["claimable"].as_bool().unwrap_or(false),
            claimed: h["claimed"].as_bool().unwrap_or(false),
            reward_chits: h["reward_chits"].as_i64().unwrap_or(0),
            reward_material: h["reward_material"].as_str().unwrap_or("").to_string(),
            reward_material_qty: h["reward_material_qty"].as_i64().unwrap_or(0) as i32,
            reward_gear: h["reward_gear"].as_bool().unwrap_or(false),
            where_to_look: h["where_to_look"].as_str().unwrap_or("").to_string(),
            accepted: h["accepted"].as_bool().unwrap_or(false),
        })
        .collect()
}

/// What the board said when you asked to be paid — its own words on a refusal.
fn hunt_claim_text(res: &Result<ehttp::Response, String>) -> String {
    let Some(v) = reply_json(res) else {
        return "the board did not answer".to_string();
    };
    if let Some(msg) = v["error"]["message"].as_str() {
        return msg.to_string();
    }
    let chits = v["reward_chits"].as_i64().unwrap_or(0);
    let qty = v["reward_material_qty"].as_i64().unwrap_or(0);
    let mat = v["reward_material"].as_str().unwrap_or("");
    let gear = v["reward_gear"].as_str().unwrap_or("");
    let mut paid = format!("{chits}c");
    if qty > 0 && !mat.is_empty() {
        paid.push_str(&format!(", {qty} {}", mat.replace('_', " ")));
    }
    if !gear.is_empty() {
        paid.push_str(&format!(", and {gear}"));
    }
    format!("the board pays {paid}")
}

fn craft_reply_text(res: &Result<ehttp::Response, String>) -> String {
    let Some(v) = reply_json(res) else {
        return "the workshop did not answer".to_string();
    };
    if let Some(msg) = v["error"]["message"].as_str() {
        return msg.to_string();
    }
    let name = v["name"].as_str().or(v["crafted"].as_str()).unwrap_or("something");
    let qty = v["quantity"].as_i64().unwrap_or(1);
    let spent: Vec<String> = v["spent"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            format!(
                "{} {}",
                m["quantity"].as_i64().unwrap_or(0),
                m["item_kind"].as_str().unwrap_or("")
            )
        })
        .collect();
    if spent.is_empty() {
        format!("made {qty}x {name}")
    } else {
        format!("made {qty}x {name} from {}", spent.join(" + "))
    }
}

/// Same, for a forge — the success case names the STATS, since that is the whole
/// reason to forge rather than buy.
fn forge_reply_text(res: &Result<ehttp::Response, String>) -> String {
    let Some(v) = reply_json(res) else {
        return "the forge did not answer".to_string();
    };
    if let Some(msg) = v["error"]["message"].as_str() {
        return msg.to_string();
    }
    let name = v["forged"].as_str().unwrap_or("a piece");
    let stats = [("atk", &v["stats"]["atk"]), ("def", &v["stats"]["def"]), ("spd", &v["stats"]["spd"])]
        .into_iter()
        .filter_map(|(n, val)| val.as_i64().filter(|v| *v > 0).map(|v| format!("+{v} {n}")))
        .collect::<Vec<_>>()
        .join(" ");
    let quenched = if v["catalyzed"].as_bool().unwrap_or(false) { " (quenched)" } else { "" };
    let affixes = v["affixes"].as_array().map(|a| a.len()).unwrap_or(0);
    let affix_note = if affixes > 0 { format!(", {affixes} affix(es)") } else { String::new() };
    format!(
        "forged {name}{quenched} - tier {} {stats}{affix_note}",
        v["tier"].as_i64().unwrap_or(0)
    )
}

/// A reroll's reply as one line: the affixes it drew, or why it refused (a Forging
/// level too low, or a bill it could not pay).
fn reroll_reply_text(res: &Result<ehttp::Response, String>) -> String {
    let Some(v) = reply_json(res) else {
        return "the smith did not answer".to_string();
    };
    reroll_line(&v)
}

pub fn reroll_line(v: &Value) -> String {
    if let Some(msg) = v["error"]["message"].as_str() {
        return msg.to_string();
    }
    // The reply carries affixes in their wire form (key + magnitude), so let the
    // registry read them out — the same `describe` the gear tooltip shows.
    let names: Vec<String> = serde_json::from_value::<Vec<meld_proto::affixes::Affix>>(
        v["affixes"].clone(),
    )
    .unwrap_or_default()
    .iter()
    .map(|a| a.describe())
    .collect();
    // What it ate belongs on the line too: the cost climbs with the piece's tier, so
    // "3 stock" on a starter blade and "11" on a deep one is the reading a smith wants.
    let cost = v["spent"]["materials"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            format!(
                "{} {}",
                m["quantity"].as_i64().unwrap_or(0),
                m["item_kind"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join(" + ");
    let bill = if cost.is_empty() {
        String::new()
    } else {
        format!(" (spent {cost})")
    };
    if names.is_empty() {
        format!("rerolled - it came up bare{bill}")
    } else {
        format!("rerolled: {}{bill}", names.join(", "))
    }
}

/// A repair's reply as one line. It bills only for what it actually restored, so the
/// number is worth showing.
fn repair_reply_text(res: &Result<ehttp::Response, String>) -> String {
    let Some(v) = reply_json(res) else {
        return "the smith did not answer".to_string();
    };
    repair_line(&v)
}

pub fn repair_line(v: &Value) -> String {
    if let Some(msg) = v["error"]["message"].as_str() {
        return msg.to_string();
    }
    format!(
        "repaired +{} durability for {}c",
        v["restored"].as_i64().unwrap_or(0),
        v["spent_chits"].as_i64().unwrap_or(0)
    )
}

fn reply_json(res: &Result<ehttp::Response, String>) -> Option<Value> {
    res.as_ref().ok()?.text().and_then(|t| serde_json::from_str::<Value>(t).ok())
}

/// What "equip best" did, in a line: which slots changed, or that nothing better was spare.
fn equip_best_reply(res: &Result<ehttp::Response, String>) -> String {
    if let Some(msg) = save_refusal(res) {
        return msg;
    }
    let Some(v) = res
        .as_ref()
        .ok()
        .and_then(|r| r.text())
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
    else {
        return "equipped".to_string();
    };
    let rows = v["data"]["changed"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return "nothing spare beats what this hero already wears".to_string();
    }
    let what: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "{} -> {}",
                r["slot"].as_str().unwrap_or("?").replace('_', " "),
                r["name"].as_str().unwrap_or("?")
            )
        })
        .collect();
    format!("equipped {}: {}", rows.len(), what.join(", "))
}

/// The server's own words when a write is refused, or `None` when it went through.
fn save_refusal(res: &Result<ehttp::Response, String>) -> Option<String> {
    match res {
        Ok(r) if r.ok => None,
        Ok(r) => Some(
            r.text()
                .and_then(|t| serde_json::from_str::<Value>(t).ok())
                .and_then(|v| {
                    v["error"]["message"].as_str().map(String::from).or_else(|| {
                        v["message"].as_str().map(String::from)
                    })
                })
                .unwrap_or_else(|| format!("the server refused that ({})", r.status)),
        ),
        Err(e) => Some(format!("could not reach the server: {e}")),
    }
}

/// Read the caller's saved loadouts. Standalone (like `spawn_inventory_fetch`) so a WRITE
/// can chain the re-read inside its own completion callback: firing the read next to the
/// write instead raced it, and the list came back without the row just saved — which is
/// exactly what "I couldn't name and save my party" looks like from the outside.
fn spawn_loadouts_fetch(base: String, token: String, tx: mpsc::Sender<Vec<LoadoutLine>>) {
    let mut req = ehttp::Request::get(format!("{base}/v1/party/loadouts"));
    req.headers.insert("Authorization", format!("Bearer {token}"));
    ehttp::fetch(req, move |res| {
        let mut list = Vec::new();
        if let Ok(resp) = &res {
            if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                for row in v["data"].as_array().into_iter().flatten() {
                    let name = row["name"].as_str().unwrap_or_default().to_string();
                    let classes: Vec<String> = row["classes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c.as_str().map(String::from))
                        .collect();
                    if !name.is_empty() {
                        list.push(LoadoutLine { name, classes });
                    }
                }
            }
        }
        let _ = tx.send(list);
    });
}

fn spawn_inventory_fetch(base: String, token: String, tx: mpsc::Sender<InvPayload>) {
    let gear_url = format!("{base}/v1/vault/gear");
    let mut req = ehttp::Request::get(format!("{base}/v1/vault"));
    req.headers.insert("Authorization", format!("Bearer {token}"));
    ehttp::fetch(req, move |vault_res| {
        let mut chits = 0i64;
        let mut materials = Vec::new();
        let mut pending = Vec::new();
        if let Ok(resp) = &vault_res {
            if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                chits = v["chits"].as_i64().unwrap_or(0);
                let stacks = |arr: &Value| -> Vec<(String, i32)> {
                    arr.as_array()
                        .map(|a| {
                            a.iter()
                                .map(|m| {
                                    (
                                        m["item_kind"].as_str().unwrap_or("?").to_string(),
                                        m["quantity"].as_i64().unwrap_or(0) as i32,
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                };
                materials = stacks(&v["materials"]);
                pending = stacks(&v["pending"]);
            }
        }
        let mut greq = ehttp::Request::get(&gear_url);
        greq.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(greq, move |gear_res| {
            let mut gear = Vec::new();
            if let Ok(resp) = &gear_res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    if let Some(arr) = v["data"].as_array() {
                        gear = arr
                            .iter()
                            .map(|g| GearLine {
                                gear_id: g["gear_id"].as_str().unwrap_or("").to_string(),
                                name: g["name"].as_str().unwrap_or("?").to_string(),
                                slot: g["slot"].as_str().unwrap_or("").to_string(),
                                class_key: g["class_key"].as_str().unwrap_or("").to_string(),
                                insurance: g["insurance"].as_str().unwrap_or("insured").to_string(),
                                family: g["family"].as_str().unwrap_or("").to_string(),
                                armor_weight: g["armor_weight"].as_str().unwrap_or("").to_string(),
                                affixes: serde_json::from_value(g["affixes"].clone()).unwrap_or_default(),
                                unique_key: g["unique_key"].as_str().unwrap_or("").to_string(),
                                set_key: g["set_key"].as_str().unwrap_or("").to_string(),
                                tier: g["tier"].as_i64().unwrap_or(0) as i32,
                                equipped_hero_slot: g["equipped_hero_slot"].as_i64().map(|s| s as usize),
                                max_durability: g["max_durability"].as_i64().unwrap_or(0) as i32,
                                base_max_durability: g["base_max_durability"].as_i64().unwrap_or(0)
                                    as i32,
                                atk_bonus: g["atk_bonus"].as_i64().unwrap_or(0) as i32,
                                def_bonus: g["def_bonus"].as_i64().unwrap_or(0) as i32,
                                spd_bonus: g["spd_bonus"].as_i64().unwrap_or(0) as i32,
                                reroll_cost: g["reroll_cost"].as_i64().unwrap_or(0) as i32,
                            })
                            .collect();
                    }
                }
            }
            let _ = tx.send((chits, materials, gear, pending));
        });
    });
}

/// Kick off register (idempotent) + login via `ehttp`; the result arrives on the
/// returned channel, off a background thread.
fn spawn_login(base: &str, username: &str, password: &str) -> mpsc::Receiver<LoginResult> {
    let (tx, rx) = mpsc::channel();
    let body = serde_json::to_vec(&json!({ "username": username, "password": password }))
        .unwrap_or_default();

    let mut reg = ehttp::Request::post(format!("{base}/v1/auth/register"), body.clone());
    reg.headers.insert("Content-Type", "application/json");

    let login_url = format!("{base}/v1/auth/login");
    ehttp::fetch(reg, move |reg_res| {
        // Proceed to login only if the account exists now: 201 (just created) or 409
        // (already existed). Any OTHER register failure — e.g. 400 "Password must be
        // 8–128 chars." — means no account was created, so surface THAT reason
        // directly instead of a misleading "wrong password" from the follow-up login.
        match &reg_res {
            Ok(resp) if resp.status == 201 || resp.status == 409 => {}
            Ok(resp) => {
                let msg = resp
                    .text()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                    .and_then(|v| v["error"]["message"].as_str().map(String::from))
                    .unwrap_or_else(|| format!("Sign-up failed (status {}).", resp.status));
                let _ = tx.send(Err(msg));
                return;
            }
            Err(e) => {
                let _ = tx.send(Err(format!("Sign-up request failed: {e}")));
                return;
            }
        }
        let mut login = ehttp::Request::post(&login_url, body);
        login.headers.insert("Content-Type", "application/json");
        ehttp::fetch(login, move |res| {
            let result: LoginResult = match res {
                Ok(resp) if resp.ok => match resp.text() {
                    Some(t) => match serde_json::from_str::<serde_json::Value>(t) {
                        Ok(v) => Ok((
                            v["realtime_ticket"].as_str().unwrap_or_default().to_string(),
                            v["player"]["player_id"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            v["session_token"].as_str().unwrap_or_default().to_string(),
                        )),
                        Err(e) => Err(format!("login parse: {e}")),
                    },
                    None => Err("login: empty body".into()),
                },
                // The account exists (register said 409/201) but login was rejected →
                // the password is wrong. Anything else is an unexpected status.
                Ok(resp) if resp.status == 401 => Err("wrong-password".into()),
                Ok(resp) => Err(format!("login status {}", resp.status)),
                Err(e) => Err(format!("login request: {e}")),
            };
            let _ = tx.send(result);
        });
    });
    rx
}

#[cfg(test)]
mod mob_state_tests {
    use super::*;

    fn tag<'a>(kind: &'a str, faction: &'a str) -> MobTag<'a> {
        MobTag { kind, faction, ..Default::default() }
    }

    #[test]
    fn a_mob_state_keeps_its_faction_whatever_marker_it_carries() {
        assert_eq!(
            parse_mob_state("mob:forest_bloom_stalker:fungal"),
            tag("forest_bloom_stalker", "fungal")
        );
        // The marker must not be read as part of the faction — the faction drives the
        // creature's colour, and `fungal:quarry` is not a colour.
        assert_eq!(
            parse_mob_state("mob:forest_bloom_stalker:fungal:quarry"),
            MobTag { quarry: true, ..tag("forest_bloom_stalker", "fungal") }
        );
        // CL-2's pin rides the same slot, and must not be read as a quarry either.
        assert_eq!(
            parse_mob_state("mob:forest_bloom_stalker:fungal:held"),
            MobTag { held: true, ..tag("forest_bloom_stalker", "fungal") }
        );
        // CR-2's clash is a third marker in the same slot.
        assert_eq!(
            parse_mob_state("mob:forest_bloom_stalker:fungal:clash"),
            MobTag { clashing: true, ..tag("forest_bloom_stalker", "fungal") }
        );
        // A factionless mob, and an unknown trailing token, both stay readable.
        assert_eq!(parse_mob_state("mob:dune_wyrm"), tag("dune_wyrm", ""));
        assert_eq!(parse_mob_state("mob:dune_wyrm:wyrm:something"), tag("dune_wyrm", "wyrm"));
    }

    /// Markers COMPOSE. The server appends `held` before `clash` and the per-viewer cull
    /// appends `quarry` last, so reading only the first trailing part meant a creature
    /// that was pinned *and* the quarry of your hunt lost its QUARRY plate — a bug that
    /// looks like a rendering glitch and only shows up on the one creature you care most
    /// about seeing.
    #[test]
    fn every_marker_on_a_mob_state_is_read_not_just_the_first() {
        assert_eq!(
            parse_mob_state("mob:dune_wyrm:wyrm:held:clash:quarry"),
            MobTag { quarry: true, held: true, clashing: true, ..tag("dune_wyrm", "wyrm") }
        );
        assert_eq!(
            parse_mob_state("mob:dune_wyrm:wyrm:held:quarry"),
            MobTag { quarry: true, held: true, ..tag("dune_wyrm", "wyrm") }
        );
    }

    /// FS-4: a boss NAMES itself on the overworld, and its name is a `key:value` token
    /// rather than a flag — so the value must be consumed by its key and never read as a
    /// marker of its own. A Gatekeeper, an end-fight peer and a bounty mark all overlay a
    /// host creature, so without this token the client has nothing but the host's wildlife
    /// kind and draws the thing the whole walk out is pointed at as a boar.
    #[test]
    fn a_boss_names_itself_and_still_composes_with_every_marker() {
        assert_eq!(
            parse_mob_state("mob:bog_stinger:undead:boss:choirmother"),
            MobTag { boss: Some("choirmother"), ..tag("bog_stinger", "undead") }
        );
        // The boss key rides in FRONT of the state markers, and none of them is lost to
        // it — nor is the key itself read as one of them.
        assert_eq!(
            parse_mob_state("mob:bog_stinger:construct:boss:ironmaw:held:clash:quarry"),
            MobTag {
                boss: Some("ironmaw"),
                quarry: true,
                held: true,
                clashing: true,
                ..tag("bog_stinger", "construct")
            }
        );
        // Order is not load-bearing: the token set is a set.
        assert_eq!(
            parse_mob_state("mob:dune_wyrm:wyrm:clash:boss:rustfang"),
            MobTag { boss: Some("rustfang"), clashing: true, ..tag("dune_wyrm", "wyrm") }
        );
        // A truncated pair names nobody rather than naming the empty string, which would
        // resolve to no sprite and no title while still reading as "this is a boss".
        assert_eq!(parse_mob_state("mob:dune_wyrm:wyrm:boss"), tag("dune_wyrm", "wyrm"));
        assert_eq!(parse_mob_state("mob:dune_wyrm:wyrm:boss:"), tag("dune_wyrm", "wyrm"));
        // Ordinary fauna names no boss at all.
        assert_eq!(parse_mob_state("mob:thornback_boar:beast").boss, None);
    }

    /// The scale marker is a `key:value` in the same SET as the others, so a Colossus that
    /// is ALSO pinned and ALSO your quarry keeps all three — the failure mode this parser
    /// was rewritten for.
    #[test]
    fn a_raid_marker_composes_with_every_other_marker() {
        let t = parse_mob_state("mob:bog_stinger:construct:boss:ironmaw:parties:3:held:quarry");
        assert_eq!(t.parties, 3);
        assert_eq!(t.boss, Some("ironmaw"));
        assert!(t.held && t.quarry);
        // Order must not matter.
        let t2 = parse_mob_state("mob:bog_stinger:construct:quarry:parties:2:boss:ironmaw");
        assert_eq!(t2.parties, 2);
        assert_eq!(t2.boss, Some("ironmaw"));
        assert!(t2.quarry);
        // Ordinary fauna carries no scale at all, so nothing draws a plate for it.
        assert_eq!(parse_mob_state("mob:bog_stinger:beast").parties, 0);
        // A malformed count reads as ordinary rather than as a guess.
        assert_eq!(parse_mob_state("mob:x:y:parties:nine").parties, 0);
    }
}

#[cfg(test)]
mod payout_tests {
    use super::*;

    /// The bug this pins: ground loot was banked with `cause: pickup:<kind>` and nothing
    /// read it, so a creature that died fighting another creature and left something
    /// behind was indistinguishable from one that left nothing — the only trace was a
    /// counter ticking somewhere off-screen.
    #[test]
    fn ground_loot_is_a_payout_the_player_gets_told_about() {
        assert_eq!(payout_of("pickup:bog_myrrh"), Some(Payout::Pickup));
    }

    #[test]
    fn a_chest_and_a_harvest_tick_keep_their_own_surfaces() {
        assert_eq!(payout_of("chest"), Some(Payout::Chest));
        assert_eq!(payout_of("harvest"), Some(Payout::Harvest));
        assert_eq!(payout_of("harvest:bloom_herb"), Some(Payout::Harvest));
    }

    /// Bookkeeping is not a payout. Spending ore on a wall, losing a bag on death or
    /// paying for a craft are all things the player just watched themselves do, and a
    /// banner over each would train them to dismiss the banner.
    #[test]
    fn a_spend_is_not_a_payout() {
        for cause in ["build", "craft", "death", "flee", "battle_loot", "potion_drop", ""] {
            assert_eq!(payout_of(cause), None, "`{cause}` raised a payout banner");
        }
    }
}

#[cfg(test)]
mod hero_condition_tests {
    use super::*;

    fn hero(hp: i32, afflictions: &[&str]) -> HeroLine {
        HeroLine {
            name: "Ash".into(),
            class_key: "explorer".into(),
            level: 5,
            str_: 1,
            mnd: 1,
            dex: 1,
            wll: 1,
            max_hp: 50,
            hp,
            xp: 0,
            xp_to_next: 1,
            back_row: false,
            afflictions: afflictions.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_healthy_hero_has_nothing_to_report() {
        assert_eq!(hero(50, &[]).condition_label(), "");
        assert!(!hero(50, &[]).fallen());
    }

    /// Being down outranks whatever else is on you: you need a raise, not a cure.
    #[test]
    fn fallen_wins_over_every_affliction() {
        let h = hero(0, &["poison", "web"]);
        assert!(h.fallen());
        assert_eq!(h.condition_label(), "Fallen");
    }

    #[test]
    fn conditions_read_as_words_not_wire_keys() {
        assert_eq!(hero(30, &["poison"]).condition_label(), "Poison");
        assert_eq!(hero(30, &["poison", "web"]).condition_label(), "Poison, Web");
        // Multi-word keys ride the wire with an underscore in them.
        assert_eq!(hero(30, &["gang_up"]).condition_label(), "Gang up");
    }
}

#[cfg(test)]
mod smith_reply_tests {
    use super::*;

    #[test]
    fn a_reroll_reads_out_the_affixes_it_actually_drew() {
        // The server answers with affixes in WIRE form — `key` + `magnitude`, no
        // display name — so the line has to go through the registry to say anything.
        let drew = serde_json::json!({
            "gear_id": "g1",
            "affixes": [
                { "key": "atk_flat", "magnitude": 4 },
                { "key": "def_flat", "magnitude": 2 },
            ],
            "spent": {
                "materials": [{ "item_kind": "dune_ingot", "quantity": 7 }],
                "chits": 90,
            },
        });
        let line = reroll_line(&drew);
        assert!(line.starts_with("rerolled: "), "{line}");
        assert!(line.contains("spent 7 dune_ingot"), "the bill belongs on the line: {line}");
        assert!(line.contains(','), "both affixes belong on the line: {line}");
        for a in [
            meld_proto::affixes::Affix {
                key: "atk_flat".into(),
                magnitude: 4,
                element: None,
                ally_class: None,
            },
            meld_proto::affixes::Affix {
                key: "def_flat".into(),
                magnitude: 2,
                element: None,
                ally_class: None,
            },
        ] {
            assert!(line.contains(&a.describe()), "{line} is missing {}", a.describe());
        }

        // A bare draw is still an answer, and a refusal is the server's own sentence.
        assert!(reroll_line(&serde_json::json!({ "affixes": [] })).contains("came up bare"));
        let refused = serde_json::json!({
            "error": { "code": "conflict", "message": "A reroll needs 1 dune_ingot and 40 chits." }
        });
        assert_eq!(reroll_line(&refused), "A reroll needs 1 dune_ingot and 40 chits.");
    }

    #[test]
    fn a_repair_says_what_it_restored_and_what_it_cost() {
        // It bills only for what it actually restored, so both numbers are the answer
        // to "was that worth it".
        let done = serde_json::json!({ "restored": 6, "spent_chits": 120 });
        assert_eq!(repair_line(&done), "repaired +6 durability for 120c");
        let refused = serde_json::json!({
            "error": { "code": "conflict", "message": "Nothing to repair, or not enough chits." }
        });
        assert_eq!(repair_line(&refused), "Nothing to repair, or not enough chits.");
    }
}
