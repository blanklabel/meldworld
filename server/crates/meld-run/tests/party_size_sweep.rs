//! **AT WHAT DEPTH DO YOU REALLY NEED FOUR HEROES?**
//!
//! With `encounter_party_scale` retired (`CR-14`) a creature is fixed by its level, so
//! "how many heroes do I need" finally has an answer that does not move when you answer
//! it. This plays the REAL encounter the world generates at a depth, with the real engine,
//! at one / two / three / four heroes, and reports where each party size stops winning.
//!
//! It is a sweep rather than an assertion for most of its output: the numbers are
//! `[TUNABLE]` and pinning them would make every balance change break this file. What it
//! DOES assert is the shape the design claims — mustering must buy something, the wall
//! must arrive, and it must arrive later for a bigger party.
//!
//! ⚠️ **UNGEARED, and that is a FLOOR rather than a verdict.** `EW-0` measures gear at
//! ~3.5x survivability, re-measured at 3.7x less damage taken at level 100. A geared party
//! reaches meaningfully deeper than anything printed here.
//!
//! ⚠️ **AND IT DOES NOT DRINK.** The starting kit is ~1130 HP of healing on a 2648 HP
//! party, so a measurement without potions understates a party by ~42% (see the end-fight
//! note in `balance.toml`). Both omissions push the same way: the real wall is deeper.
//!
//! Run it for the table: `cargo test -p meld-run --test party_size_sweep -- --nocapture`.

use meld_balance::Balance;
use meld_proto::enums::{BattleActionKind, BattleOutcome, CharacterClass};
use meld_run::{build_battle, GearBonus, InstanceRun, PartyMember};

/// A party of `n`, in the shipped default composition order, cycled.
///
/// Explorer / Psyker / Resonant / Phoenix Guard rather than four of one class: the
/// question is what a PARTY does, and a party's answer to a deep creature is its healer
/// and its wall, not four copies of the same hero.
const COMP: [CharacterClass; 4] = [
    CharacterClass::Explorer,
    CharacterClass::Psyker,
    CharacterClass::Resonant,
    CharacterClass::PhoenixGuard,
];

struct Fought {
    won: bool,
    /// Seconds of engine time the fight took (100 ms ticks).
    seconds: f64,
    /// Share of the party's total HP still standing at the end, 0..1.
    hp_left: f64,
    /// How many enemies were in it — a pack is what makes depth bite.
    foes: usize,
    /// What the world called it. A champion or a gatekeeper is a different question from
    /// an ordinary pack, and mixing them into one table is what made the first run of this
    /// sweep read as non-monotonic: d300 and d600 lost with ONE foe while d800 won with
    /// five, because those two rings happened to have a boss nearest the sample point.
    class: String,
}

/// Play the encounter the world actually generates nearest `distance`, with `heroes`
/// heroes at the level that depth grants, until somebody wins.
fn fight_at(
    distance: f64,
    heroes: usize,
    seed: u64,
    b: &Balance,
    ordinary_only: bool,
) -> Option<Fought> {
    let mut arena = meld_world::Arena::generate(b, seed, false);
    for _ in 0..4096 {
        if arena.ensure_frontier(b, distance + 60.0).is_empty() {
            break;
        }
    }
    // The creature closest to the ring we asked about, and the pack standing with it —
    // `group_around` is the same call a real touch makes, so this is the encounter a
    // player walking to that depth would meet rather than one creature in isolation.
    let target = (0..arena.monsters.len())
        .filter(|&i| !arena.monsters[i].defeated)
        .filter(|&i| !ordinary_only || arena.monsters[i].encounter_class != "gatekeeper")
        .filter(|&i| !ordinary_only || arena.monsters[i].encounter_class != "elite")
        .filter(|&i| !ordinary_only || arena.monsters[i].boss_kind.is_empty())
        .min_by(|&a, &c| {
            let d = |i: usize| {
                let p = arena.monsters[i].position;
                (p.x.hypot(p.y) - distance).abs()
            };
            d(a).total_cmp(&d(c))
        })?;
    let group = arena.group_around(target);
    // The strongest thing standing in it is what the encounter IS — the same rule
    // `build_battle` uses to pick the battle's own encounter class.
    let class = group
        .iter()
        .map(|&i| arena.monsters[i].encounter_class.clone())
        .max_by_key(|c| match c.as_str() {
            "gatekeeper" | "world_end" => 3,
            "elite" | "undead_rite" => 2,
            "leader" => 1,
            _ => 0,
        })
        .unwrap_or_default();
    let enemies: Vec<(&meld_world::MonsterSpawn, String)> = group
        .iter()
        .enumerate()
        .map(|(n, &i)| (&arena.monsters[i], format!("e{n}")))
        .collect();

    // The party, at the level that depth grants. `base_run_level` is the DEV/QA inverse of
    // `MELD_START_LEVEL` and exists exactly for this: level and depth are the same fact.
    let level = meld_run::base_run_level(distance as i32, b);
    let mut runs = InstanceRun::new("i".into(), distance as i32, b, 0);
    runs.add_party(
        (0..heroes)
            .map(|i| (format!("p{i}"), "u".into(), COMP[i % 4], format!("r{i}")))
            .collect(),
    );
    for r in runs.runs.iter_mut() {
        r.hero_levels = vec![level; heroes];
        r.hero_xp = vec![0; heroes];
        r.run_level = level;
    }
    let party: Vec<PartyMember> = (0..heroes)
        .map(|i| (format!("p{i}"), format!("h{i}"), COMP[i % 4], GearBonus::default()))
        .collect();

    let mut battle = build_battle(
        "b".into(),
        &party,
        &enemies,
        &runs,
        b,
        seed,
        &vec![None; heroes],
        &vec![None; heroes],
        meld_battle::Opening::Rolled,
    );
    let (mine, foes) = battle.wire_combatants();
    let hero_ids: Vec<String> = mine.iter().map(|c| c.combatant_id.clone()).collect();
    let foe_ids: Vec<String> = foes.iter().map(|c| c.combatant_id.clone()).collect();
    let party_hp: i32 = mine.iter().map(|c| c.max_hp).sum();
    let foe_count = foe_ids.len();

    // Everyone swings at whatever is still up. A deliberately DUMB policy: no skills, no
    // potions, no healer doing its job. `auto_battle`'s own note records that a weak
    // policy's number is a floor rather than a result, and a floor is what this wants —
    // "even swinging blindly, N heroes clear it" is the honest shape of a difficulty wall.
    let mut won = None;
    let mut swing = 0u64;
    // 20 minutes of engine time. A fight unresolved by then has not been won.
    for _ in 0..12_000 {
        for ev in battle.tick() {
            if let meld_battle::Event::Ended { outcome } = ev {
                won = Some(outcome == BattleOutcome::Victory);
            }
        }
        if won.is_some() {
            break;
        }
        let alive_foe = foe_ids
            .iter()
            .find(|id| battle.combatant_hp(id).is_some_and(|hp| hp > 0))
            .cloned();
        let Some(foe) = alive_foe else { break };
        for h in &hero_ids {
            if battle.combatant_hp(h).is_some_and(|hp| hp > 0) {
                swing += 1;
                // ⚠️ THE OUTCOME COMES BACK FROM `submit`, NOT FROM THE NEXT `tick`. The
                // killing blow ends the fight inside the call that lands it, so discarding
                // this return loses the `Ended` event — and the loop then falls out on
                // "no living foe" and scores a WIN as a loss. The first cut of this sweep
                // did exactly that and reported a level-3 party of four losing to one
                // creature at d25.
                if let Ok(evs) = battle.submit(
                    h,
                    format!("a{swing}"),
                    BattleActionKind::Attack,
                    Some(vec![foe.clone()]),
                    None,
                    None,
                ) {
                    for ev in evs {
                        if let meld_battle::Event::Ended { outcome } = ev {
                            won = Some(outcome == BattleOutcome::Victory);
                        }
                    }
                }
            }
        }
        if won.is_some() {
            break;
        }
    }
    let left: i32 = hero_ids.iter().filter_map(|h| battle.combatant_hp(h)).map(|h| h.max(0)).sum();
    Some(Fought {
        won: won.unwrap_or(false),
        seconds: battle.tick_count() as f64 / 10.0,
        hp_left: left as f64 / party_hp.max(1) as f64,
        foes: foe_count,
        class,
    })
}

/// The table. Not an assertion — a printout, because every number in it is `[TUNABLE]`.
///
/// ⚠️ **`#[ignore]`d, AND THAT IS NOT LAZINESS.** It generates and streams a world per cell
/// — 4 party sizes x 5 seeds x 10 depths, several of them past d1200 — which is 200 world
/// generations. In release that is ~3 minutes; **in the DEBUG build `make check` uses it ran
/// for 34 minutes before it was caught**, and a gate that takes an hour is a gate everyone
/// learns to skip, which is the same reasoning that keeps `qa/` out of CI. Run it on purpose:
///
/// ```sh
/// cargo test --release -p meld-run --test party_size_sweep -- --ignored --nocapture
/// ```
#[ignore = "generates 200 worlds; run it explicitly in release for the table"]
/// ⚠️ **FIVE SEEDS PER CELL, AND THAT IS NOT OPTIONAL.** The first cut sampled ONE world
/// per depth and read as non-monotonic — d400 lost at every party size while d600 and d800
/// won — because which species, which kit and which formation happen to stand nearest a
/// ring is a coin toss. `AGENTS.md` records the same lesson twice over for world geometry:
/// any per-seed number about this world is a coin toss waiting to be flipped. What a party
/// size buys is a WIN RATE across the encounters a depth actually produces.
#[test]
fn how_deep_can_each_party_size_go() {
    let b = Balance::load_default().unwrap();
    const SEEDS: [u64; 5] = [424_242, 7, 99, 1, 20_260_906];
    let depths = [25.0_f64, 50.0, 100.0, 150.0, 200.0, 300.0, 400.0, 600.0, 800.0, 1200.0];
    let started = std::time::Instant::now();
    println!(
        "\n  ORDINARY encounters, {} worlds each — ungeared, no potions, attack-only.\n           A FLOOR, not a verdict: gear is ~3.5x survivability and the kit is ~42% more \n           effective HP again. `won` is out of {}; `hp` is the median survivor share.\n\
         \n  depth  lvl  foes | {:>14} {:>14} {:>14} {:>14}\n           (won/5, median survivor HP, median seconds of the wins)",
        SEEDS.len(),
        SEEDS.len(),
        "1 hero",
        "2 heroes",
        "3 heroes",
        "4 heroes"
    );
    for &d in &depths {
        let mut cells = Vec::new();
        let mut foes: Vec<usize> = Vec::new();
        for n in 1..=4 {
            let fights: Vec<Fought> =
                SEEDS.iter().filter_map(|&s| fight_at(d, n, s, &b, true)).collect();
            let won = fights.iter().filter(|f| f.won).count();
            let mut hp: Vec<f64> = fights.iter().map(|f| f.hp_left).collect();
            hp.sort_by(f64::total_cmp);
            let med = hp.get(hp.len() / 2).copied().unwrap_or(0.0);
            // The other half of the design claim: mustering buys a SHORTER fight. Median
            // over the wins only — how long a fight you lost ran is a measure of how long
            // you survived it, which is a different question.
            let mut secs: Vec<f64> =
                fights.iter().filter(|f| f.won).map(|f| f.seconds).collect();
            secs.sort_by(f64::total_cmp);
            let med_s = secs.get(secs.len() / 2).copied().unwrap_or(0.0);
            // `ordinary_only` has to actually hold, or this table is quietly reporting
            // gatekeepers and calling them ordinary encounters — which is exactly how the
            // single-seed version read as non-monotonic.
            assert!(
                fights.iter().all(|f| f.class != "gatekeeper" && f.class != "world_end"),
                "a boss got into the ORDINARY table at d{d}"
            );
            foes.extend(fights.iter().map(|f| f.foes));
            cells.push(format!("{won}/{} {:>3.0}% {:>3.0}s", SEEDS.len(), med * 100.0, med_s));
        }
        foes.sort_unstable();
        println!(
            "  {:>5.0} {:>4} {:>5} | {:>14} {:>14} {:>14} {:>14}",
            d,
            meld_run::base_run_level(d as i32, &b),
            foes.get(foes.len() / 2).copied().unwrap_or(0),
            cells[0],
            cells[1],
            cells[2],
            cells[3]
        );
    }
    println!("\n  ({:.0}s)\n", started.elapsed().as_secs_f64());
}

/// **MUSTERING HAS TO BUY SOMETHING, AND THE WALL HAS TO ARRIVE.**
///
/// The two halves of `CR-14`'s claim, and the only part of the sweep worth asserting:
///
/// - at every depth a party of four ends a fight with MORE of its health than a lone hero
///   does — that is what bringing three friends to a fixed creature has to mean, and if it
///   ever stops being true something is scaling the world to the party again;
/// - and somewhere out there a lone hero loses a fight four heroes win, because a world
///   with no such depth is a world where party size is decoration.
#[test]
fn a_bigger_party_survives_deeper_and_the_wall_really_arrives() {
    let b = Balance::load_default().unwrap();
    let mut solo_wall = None;
    // SHALLOW ON PURPOSE. This runs in the DEBUG gate, and every extra ring is a world
    // streamed one section at a time — the printout above went to d1200 and cost `make
    // check` 34 minutes. d300 is where the wall measured, so sampling past it buys nothing
    // the assertion needs and costs minutes.
    for &d in &[100.0_f64, 200.0, 300.0] {
        let (Some(solo), Some(full)) = (fight_at(d, 1, 424_242, &b, true), fight_at(d, 4, 424_242, &b, true))
        else {
            continue;
        };
        // A lone hero must never come out of a fixed encounter in better shape than four.
        // `hp_left` is a FRACTION of each party's own pool, so this compares like with
        // like rather than rewarding the party for simply having more health.
        assert!(
            full.hp_left >= solo.hp_left - 0.01,
            "at d{d} a lone hero kept {:.0}% of its health and four kept {:.0}% — bringing \
             three more heroes to the same creature made the fight WORSE",
            solo.hp_left * 100.0,
            full.hp_left * 100.0
        );
        if !solo.won && full.won && solo_wall.is_none() {
            solo_wall = Some(d);
        }
    }
    assert!(
        solo_wall.is_some(),
        "a lone hero cleared every depth sampled out to d1600, so party size buys only \
         SPEED and never survival — the difficulty wall this game is built on is missing"
    );
}
