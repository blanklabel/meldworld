//! Run & instance lifecycle for the spike (behaviors/run-lifecycle.md subset).
//!
//! Provides: base-run-level derivation, per-player ephemeral run state
//! (backpack + run level/XP), the victory/defeat outcome transitions, and the
//! bridge that assembles a [`meld_battle::Battle`] from an arena monster and a
//! party. Extraction channels, death durability (HTTP/DB side), and abandon are
//! the next slices; the run/battle spine they hang off is here.

use meld_balance::Balance;
use meld_battle::{Battle, Fighter};
use meld_proto::common::ItemStack;
use meld_proto::enums::{CharacterClass, CombatantKind, EncounterClass, RunResult};
use meld_proto::Id;
use meld_world::MonsterSpawn;

/// `base_run_level(hub) = round(1 + hub.distance × per_distance)` (CANON.md §B).
pub fn base_run_level(distance: i32, balance: &Balance) -> i32 {
    (1.0 + distance as f64 * balance.runs.base_run_level_per_distance).round() as i32
}

/// XP needed to advance from level `l`: `xp_to_next(L) = 80 × L^1.6` (CANON.md §B).
pub fn xp_to_next(level: i32) -> i64 {
    (80.0 * (level as f64).powf(1.6)).round() as i64
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
    pub fn award_xp(&mut self, xp: i64) -> i32 {
        self.xp += xp;
        let mut gained = 0;
        while self.xp >= xp_to_next(self.run_level) {
            self.xp -= xp_to_next(self.run_level);
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
        CharacterClass::Squire => "squire",
        CharacterClass::Dragoon => "dragoon",
        CharacterClass::Sage => "sage",
        CharacterClass::Ranger => "ranger",
        CharacterClass::AlchemistKnight => "alchemist_knight",
        CharacterClass::Bard => "bard",
        CharacterClass::Psyker => "psyker",
        CharacterClass::Resonant => "resonant",
    }
}

/// Assemble a battle from a party and one arena monster. `party` gives, per
/// player, the (player_id, combatant_id, class); the server owns combatant ids.
/// Per-player combatant inputs for a battle: (player_id, combatant_id, class,
/// equipped gear attack bonus).
pub type PartyMember = (Id, Id, CharacterClass, i32);

/// Build the ally `Fighter`s for a party (shared by battle start and raid merge).
pub fn party_fighters(party: &[PartyMember], runs: &InstanceRun, balance: &Balance) -> Vec<Fighter> {
    party
        .iter()
        .map(|(player_id, combatant_id, class, atk_bonus)| {
            let stats = balance
                .player
                .get(class_key(*class))
                .unwrap_or_else(|| balance.player.get("squire").expect("squire stats"));
            let level = runs
                .runs
                .iter()
                .find(|r| &r.player_id == player_id)
                .map(|r| r.run_level)
                .unwrap_or(1);
            let mut f = Fighter::new(
                combatant_id.clone(),
                CombatantKind::Player,
                Some(player_id.clone()),
                None,
                level,
                stats.base_hp,
                stats.base_atk + atk_bonus, // equipped gear
                stats.base_def,
                stats.speed_stat,
            );
            // Surface the class to the client (drives the per-hero command menu).
            f.class_key = class_key(*class).to_string();
            match *class {
                // A Psyker channels Foci instead of the martial kit; its slot count
                // grows with level: base + 1 per `psyker_focus_per_level`, capped.
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
                }
                // A Resonant regenerates a little HP each of its turns (innate).
                CharacterClass::Resonant => {
                    f.regen = balance.battle.resonant_regen_per_turn;
                }
                _ => {}
            }
            f
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub fn build_battle(
    battle_id: Id,
    party: &[PartyMember],
    monster: &MonsterSpawn,
    monster_combatant_id: Id,
    runs: &InstanceRun,
    balance: &Balance,
    seed: u64,
    // Per-hero starting HP, aligned with `party`. `None` means full HP. Used to
    // carry wounds across a run's encounters (no free heal between fights).
    hp_overrides: &[Option<i32>],
) -> Battle {
    let mut allies = party_fighters(party, runs, balance);
    for (f, hp) in allies.iter_mut().zip(hp_overrides.iter()) {
        if let Some(h) = hp {
            f.hp = (*h).clamp(0, f.max_hp);
        }
    }

    let enemy = Fighter::new(
        monster_combatant_id,
        CombatantKind::Monster,
        None,
        Some(monster.monster_kind.clone()),
        monster.level,
        monster.hp,
        monster.atk,
        monster.def,
        monster.speed_stat,
    );

    let encounter_class = match monster.encounter_class.as_str() {
        "gatekeeper" => EncounterClass::Gatekeeper,
        "elite" => EncounterClass::Elite,
        _ => EncounterClass::Standard,
    };

    Battle::new(
        battle_id,
        encounter_class,
        allies,
        vec![enemy],
        balance,
        seed,
    )
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
        let mut r = PlayerRun {
            run_id: "r".into(),
            player_id: "p".into(),
            username: "u".into(),
            character_class: CharacterClass::Squire,
            run_level: 1,
            xp: 0,
            backpack: vec![],
            max_distance_reached: 0,
            result: None,
            party_id: 0,
        };
        // xp_to_next(1) = 80.
        let gained = r.award_xp(200);
        assert!(gained >= 1);
        assert!(r.run_level >= 2);
    }

    #[test]
    fn build_battle_applies_hp_overrides() {
        use meld_proto::common::Position;
        let b = Balance::load_default().unwrap();
        let mut runs = InstanceRun::new("i".into(), 0, &b);
        runs.add_party(vec![(
            "p1".into(),
            "u1".into(),
            CharacterClass::Squire,
            "r1".into(),
        )]);
        let monster = MonsterSpawn {
            entity_id: "m".into(),
            monster_kind: "forest_bloom_stalker".into(),
            position: Position::new(10.0, 0.0),
            level: 1,
            encounter_class: "standard".into(),
            hp: 60,
            atk: 9,
            def: 2,
            speed_stat: 80,
            xp_reward: 60,
            defeated: false,
        };
        let party: Vec<PartyMember> = vec![("p1".into(), "c1".into(), CharacterClass::Squire, 0)];
        // Carry a wounded hero in: start at 17 HP rather than full.
        let battle = build_battle(
            "b".into(),
            &party,
            &monster,
            "mc".into(),
            &runs,
            &b,
            1,
            &[Some(17)],
        );
        let (allies, _) = battle.wire_combatants();
        assert_eq!(allies.len(), 1);
        assert_eq!(allies[0].hp, 17, "wounded HP carried into the new battle");
        assert!(allies[0].max_hp > 17, "max HP stays at the class base");
    }
}
