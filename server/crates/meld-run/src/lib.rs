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
    let steps = (level - 1).max(0) as f64;
    (balance.runs.xp_base as f64 * balance.runs.xp_growth_factor.powf(steps)).round() as i64
}

/// One player's ephemeral run state.
#[derive(Debug, Clone)]
pub struct PlayerRun {
    pub run_id: Id,
    pub player_id: Id,
    pub username: String,
    pub character_class: CharacterClass,
    pub run_level: i32,
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
        CharacterClass::Hunter => "hunter",
        CharacterClass::Dragoon => "dragoon",
        CharacterClass::Sage => "sage",
        CharacterClass::Ranger => "ranger",
        CharacterClass::AlchemistKnight => "alchemist_knight",
        CharacterClass::Bard => "bard",
        CharacterClass::Psyker => "psyker",
        CharacterClass::Resonant => "resonant",
        CharacterClass::Shifter => "shifter",
        CharacterClass::IronHull => "iron_hull",
    }
}

/// Max HP for a class at a given level (CANON.md §B attribute growth: Wll →
/// HP). Shared by `party_fighters` (battle setup) and level-up handling (a
/// level-up heals to the new max, unlike mid-run wounds which persist).
pub fn max_hp_at_level(class: CharacterClass, level: i32, balance: &Balance) -> i32 {
    let stats = balance
        .player
        .get(class_key(class))
        .unwrap_or_else(|| balance.player.get("hunter").expect("hunter stats"));
    let (_, _, _, wll) = stats.attributes_at(level);
    let grow = |attr: i32, base: i32, coef: f64| ((attr - base) as f64 * coef).round() as i32;
    stats.base_hp + grow(wll, stats.wll, balance.attributes.wll_to_hp)
}

/// One hero's summed combat bonuses from their own equipped gear (per-hero
/// equip slots — each hero in a party can wear different gear).
#[derive(Debug, Clone, Copy, Default)]
pub struct GearBonus {
    pub atk: i32,
    pub def: i32,
    pub spd: i32,
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
    let level_by_player: HashMap<&str, i32> = runs
        .runs
        .iter()
        .map(|r| (r.player_id.as_str(), r.run_level))
        .collect();
    party
        .iter()
        .enumerate()
        .map(|(i, (player_id, combatant_id, class, bonus))| {
            let stats = balance
                .player
                .get(class_key(*class))
                .unwrap_or_else(|| balance.player.get("hunter").expect("hunter stats"));
            let level = level_by_player
                .get(player_id.as_str())
                .copied()
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
            let atk = stats.base_atk + grow(str_, stats.str, a.str_to_atk) + bonus.atk; // + gear
            let def = stats.base_def + grow(wll, stats.wll, a.wll_to_def) + bonus.def; // + gear
            let speed = stats.speed_stat + grow(dex, stats.dex, a.dex_to_speed) + bonus.spd; // + gear
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
                    f.focus_max = (bb.psyker_focus_base as i32 + extra)
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
                // The Hunter (martial baseline) earns Adrenaline through basic attacks
                // and spends it on skills; it holds the front line. Starts at 0.
                CharacterClass::Hunter => {
                    f.adrenaline_max = balance.battle.hunter_adrenaline_max;
                }
                // Other martial classes hold the front line with no special resource.
                _ => {}
            }
            // The player's saved formation choice overrides the class default.
            if let Some(Some(row)) = row_overrides.get(i) {
                f.back_row = *row;
            }
            f
        })
        .collect()
}

/// One creature joining a battle: its spawn + the combatant id to give it.
pub type EnemyMember<'a> = (&'a MonsterSpawn, Id);

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
) -> Battle {
    let mut allies = party_fighters(party, runs, balance, row_overrides);
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
            let mut f = Fighter::new(
                cid.clone(),
                CombatantKind::Monster,
                None,
                Some(m.monster_kind.clone()),
                m.level,
                m.hp,
                m.atk,
                m.def,
                m.speed_stat,
            );
            f.faction = m.faction.clone();
            f.flees = m.flees;
            f
        })
        .collect();

    // The encounter class is the strongest present (gatekeeper > elite > standard).
    let encounter_class = enemies
        .iter()
        .map(|(m, _)| match m.encounter_class.as_str() {
            "gatekeeper" => EncounterClass::Gatekeeper,
            "elite" => EncounterClass::Elite,
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
            character_class: CharacterClass::Hunter,
            run_level: 1,
            xp: 0,
            backpack: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
            result: None,
            party_id: 0,
        };
        // xp_to_next(1) = xp_base = 80; xp_to_next(2) = 160 (doubling).
        let gained = r.award_xp(200, &b);
        assert!(gained >= 1);
        assert!(r.run_level >= 2);
        // 200 XP clears level 1 (80) but not level 1+2 (80+160=240): exactly one level.
        assert_eq!(gained, 1);
        assert_eq!(r.xp, 120);
    }

    #[test]
    fn xp_curve_doubles_each_level() {
        let b = Balance::load_default().unwrap();
        assert_eq!(xp_to_next(1, &b), 80);
        assert_eq!(xp_to_next(2, &b), 160);
        assert_eq!(xp_to_next(3, &b), 320);
        assert_eq!(xp_to_next(4, &b), 640);
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
            CharacterClass::Hunter,
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
    fn hunter_starts_with_an_empty_adrenaline_pool() {
        // The martial baseline earns its resource in-battle: the pool exists (max
        // from balance) but starts empty, and it holds the front line.
        let b = Balance::load_default().unwrap();
        let h = solo_fighter(CharacterClass::Hunter, 1, &b);
        assert_eq!(h.adrenaline_max, b.battle.hunter_adrenaline_max);
        assert_eq!(h.adrenaline, 0, "Adrenaline is banked in-fight, not granted");
        assert!(!h.back_row, "the Hunter holds the front line");
    }

    #[test]
    fn leveling_grows_stats_per_class_focus() {
        let b = Balance::load_default().unwrap();
        let sq1 = solo_fighter(CharacterClass::Hunter, 1, &b);
        let sq5 = solo_fighter(CharacterClass::Hunter, 5, &b);
        // The Hunter hardens: Str -> more atk, Wll -> more HP.
        assert!(sq5.atk > sq1.atk, "hunter atk grows with Str");
        assert!(sq5.max_hp > sq1.max_hp, "hunter HP grows with Wll");
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
            CharacterClass::Hunter,
            "r1".into(),
        )]);
        // Use a real generated creature as the enemy.
        let arena = meld_world::Arena::generate(&b, 5, true);
        let enemies = vec![(&arena.monsters[0], "mc".to_string())];
        let party: Vec<PartyMember> = vec![("p1".into(), "c1".into(), CharacterClass::Hunter, GearBonus::default())];
        // Carry a wounded hero in: start at 17 HP rather than full.
        let battle = build_battle("b".into(), &party, &enemies, &runs, &b, 1, &[Some(17)], &[]);
        let (allies, _) = battle.wire_combatants();
        assert_eq!(allies.len(), 1);
        assert_eq!(allies[0].hp, 17, "wounded HP carried into the new battle");
        assert!(allies[0].max_hp > 17, "max HP stays at the class base");
    }

    #[test]
    fn row_override_beats_the_class_default() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![
            ("p".into(), "u".into(), CharacterClass::Psyker, "r1".into()),
            ("p".into(), "u".into(), CharacterClass::Hunter, "r2".into()),
        ]);
        let party: Vec<PartyMember> = vec![
            ("p".into(), "c1".into(), CharacterClass::Psyker, GearBonus::default()), // class default: back
            ("p".into(), "c2".into(), CharacterClass::Hunter, GearBonus::default()), // class default: front
        ];
        // Override: send the Psyker to the front and pull the Hunter to the back.
        let fighters = party_fighters(&party, &runs, &b, &[Some(false), Some(true)]);
        assert!(!fighters[0].back_row, "Psyker forced to the front row");
        assert!(fighters[1].back_row, "Hunter forced to the back row");
        // An absent/None override keeps the class default.
        let dflt = party_fighters(&party, &runs, &b, &[]);
        assert!(dflt[0].back_row, "Psyker keeps its back-row default");
        assert!(!dflt[1].back_row, "Hunter keeps its front-row default");
    }

    #[test]
    fn gear_bonus_adds_into_atk_def_speed() {
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![("p".into(), "u".into(), CharacterClass::Hunter, "r".into())]);
        let bare: Vec<PartyMember> =
            vec![("p".into(), "c".into(), CharacterClass::Hunter, GearBonus::default())];
        let geared: Vec<PartyMember> = vec![(
            "p".into(),
            "c".into(),
            CharacterClass::Hunter,
            GearBonus { atk: 5, def: 3, spd: 2 },
        )];
        let f0 = party_fighters(&bare, &runs, &b, &[]).pop().unwrap();
        let f1 = party_fighters(&geared, &runs, &b, &[]).pop().unwrap();
        assert_eq!(f1.atk, f0.atk + 5);
        assert_eq!(f1.def, f0.def + 3);
        assert_eq!(f1.speed_stat, f0.speed_stat + 2);
    }
}
