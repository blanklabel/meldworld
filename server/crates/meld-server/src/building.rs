//! **What building actually decides, with no wire and no clock around it** (BD-1/BD-2).
//!
//! The three player intents — raise, mend, pack down — used to have their whole rule set
//! inline in `game.rs` handlers, which are `&mut self` methods on a state machine that owns
//! sessions, channels and a tick loop. That made the *only* way to exercise a build a
//! real-time bot over a websocket against Postgres: 34 `qa/` binaries, and not one of them
//! covered it. The join every player experiences — gather stock, then put a building up with
//! it — had never been tested at all.
//!
//! So the decisions live here as free functions over `(&mut Arena, &mut PlayerRun,
//! &Balance)`. No I/O, no `self`, no clock. The handlers keep the parts that are genuinely
//! about the wire (parsing a payload, "resolve the battle first", emitting a
//! `BackpackUpdate`) and delegate the rest.
//!
//! ⚠️ **The point is that the harness drives the REAL rule.** A harness that reimplements
//! the cost path tests a copy, and the copy is what drifts — this repo has been bitten by
//! exactly that shape more than once (`GearBonus` declared twice, the wall-collision line in
//! one mover and not the other, `is_water_kind` in three places). Extracting is what makes a
//! fast deterministic test honest.

use meld_balance::Balance;
use meld_proto::materials::MaterialClass;
use std::collections::HashMap;

/// What a build/mend/pack-down actually moved: the material kind and how much of it. The
/// sign is the caller's business — the handler knows whether it is reporting `removed` or
/// `added`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Charged {
    pub(crate) kind: String,
    pub(crate) qty: i32,
}

/// Spend `need` units of `class` out of the run's backpack, deepest tier first, summed
/// **across stacks**. Returns the kind spent.
///
/// ⚠️ Summing across stacks is load-bearing: a harvest channel banks one unit per tick as
/// its OWN stack, so material you just gathered is six stacks of one and never one stack of
/// six. Looking for a single stack big enough is what made a 6-cost structure unbuildable
/// from freshly-gathered stock while telling a player carrying exactly six that they needed
/// six.
pub(crate) fn spend(
    run: &mut meld_run::PlayerRun,
    class: MaterialClass,
    need: i32,
) -> Option<String> {
    let kind = affordable_kind(run, class, need)?;
    let mut left = need;
    for item in run.backpack.iter_mut().filter(|i| i.item_kind == kind) {
        let take = left.min(item.quantity);
        item.quantity -= take;
        left -= take;
        if left == 0 {
            break;
        }
    }
    run.backpack.retain(|i| i.quantity > 0);
    Some(kind)
}

/// Which kind we WOULD spend — the deepest tier the bag holds enough of. Chosen without
/// mutating anything, so a refusal can be free.
pub(crate) fn affordable_kind(
    run: &meld_run::PlayerRun,
    class: MaterialClass,
    need: i32,
) -> Option<String> {
    let mut totals: HashMap<String, (i32, i32)> = HashMap::new();
    for item in run
        .backpack
        .iter()
        .filter(|i| i.quantity > 0 && meld_proto::materials::is_class(&i.item_kind, class))
    {
        let tier = meld_proto::materials::material(&item.item_kind).map(|m| m.tier).unwrap_or(0);
        let e = totals.entry(item.item_kind.clone()).or_insert((0, tier));
        e.0 += item.quantity;
    }
    totals
        .into_iter()
        .filter(|(_, (have, _))| *have >= need)
        .max_by_key(|(_, (_, tier))| *tier)
        .map(|(k, _)| k)
}

/// How much of one class the bag holds, across stacks. Test-only: the production paths ask
/// [`affordable_kind`], which answers "can this be paid for" rather than "how much is
/// there" — a subtle difference that matters, since the deepest single kind must cover the
/// whole cost on its own.
#[cfg(test)]
pub(crate) fn held(run: &meld_run::PlayerRun, class: MaterialClass) -> i32 {
    run.backpack
        .iter()
        .filter(|i| meld_proto::materials::is_class(&i.item_kind, class))
        .map(|i| i.quantity)
        .sum()
}

/// **Raise a structure**: validate placement, then charge for it.
///
/// The order is the rule. Placement is checked BEFORE the stock is spent, because a refusal
/// that also charged you is the worst kind — and the arena is the only thing that knows the
/// ground (spacing, the clear path, another player standing too close).
pub(crate) fn raise(
    arena: &mut meld_world::Arena,
    run: &mut meld_run::PlayerRun,
    balance: &Balance,
    function: &str,
    player_id: &str,
    tick: u64,
) -> Result<Charged, String> {
    let def = meld_proto::structures::structure(function).ok_or("No such structure.")?;
    let (cost, _, _) = balance.building.spec(function).ok_or("No such structure.")?;
    // BD-1: what it is made of comes from the REGISTRY, never from here.
    let class = def.material;
    let kind = affordable_kind(run, class, cost)
        .ok_or_else(|| format!("{} takes {cost} {}.", def.name, class.wire()))?;
    arena
        .place_structure(balance, player_id, function, &kind, tick)
        .map_err(|why| why.message().to_string())?;
    spend(run, class, cost).expect("affordability was just checked");
    Ok(Charged { kind, qty: cost })
}

/// **Mend one** with a unit of the stock it was built from.
pub(crate) fn mend(
    arena: &mut meld_world::Arena,
    run: &mut meld_run::PlayerRun,
    balance: &Balance,
    entity_id: &str,
    player_id: &str,
) -> Result<Charged, String> {
    let reach = balance.world.interaction_radius_tiles;
    let target = arena.structure_at(player_id, entity_id, reach).ok_or("Nothing in reach.")?;
    if target.hp >= target.max_hp {
        let name = target.def().map(|d| d.name).unwrap_or("It");
        return Err(format!("The {name} is sound."));
    }
    let class = target.def().map(|d| d.material).unwrap_or(MaterialClass::Stone);
    let kind = spend(run, class, 1).ok_or_else(|| format!("No {} to mend it with.", class.wire()))?;
    arena.repair_structure(balance, entity_id);
    Ok(Charged { kind, qty: 1 })
}

/// **Pack one down** for part of its stock. Only the owner may.
pub(crate) fn pack_down(
    arena: &mut meld_world::Arena,
    run: &mut meld_run::PlayerRun,
    balance: &Balance,
    entity_id: &str,
    player_id: &str,
) -> Result<Charged, String> {
    let reach = balance.world.interaction_radius_tiles;
    let target = arena.structure_at(player_id, entity_id, reach).ok_or("Nothing in reach.")?;
    if target.owner_player_id != player_id {
        return Err("That is not yours to take down.".to_string());
    }
    let (kind, back) = arena.demolish_structure(balance, entity_id).ok_or("Nothing in reach.")?;
    if back > 0 {
        run.backpack.push(meld_proto::common::ItemStack {
            item_id: uuid::Uuid::now_v7().to_string(),
            item_kind: kind.clone(),
            quantity: back,
            insurance: None,
        });
    }
    Ok(Charged { kind, qty: back })
}

/// **The building harness.** A whole buildable world in a struct — no server, no database,
/// no websocket, no clock — so "can a player gather stock and put a building up with it"
/// answers in microseconds, identically every time.
///
/// It exists because the alternative was tried first: a `qa/` bot over the real wire. That
/// bot took 120 seconds per attempt, walked into the tutorial's scripted creature and stalled
/// at six units out, and every diagnostic cost another two minutes. Real-wire coverage is
/// still worth having for the join (`qa/tests/build_a_town.rs`), but it is a dreadful
/// instrument for asking twenty questions about costs and refusals.
///
/// Every method routes through the SAME functions the handlers call. The harness sets state
/// up and reads it back; it never reimplements a rule. That distinction is the whole value —
/// a harness with its own copy of the cost path would drift from the real one, which is the
/// failure this repo keeps meeting (`GearBonus` declared twice, the wall-collision line in
/// one mover and not the other, `is_water_kind` in three places).
#[cfg(test)]
pub(crate) struct BuildHarness {
    pub(crate) arena: meld_world::Arena,
    pub(crate) inst: meld_run::InstanceRun,
    pub(crate) balance: std::sync::Arc<Balance>,
    pub(crate) player: String,
    pub(crate) tick: u64,
}

#[cfg(test)]
impl BuildHarness {
    /// A streamed world with one player standing somewhere a structure may legally go.
    ///
    /// ⚠️ Standing legally is not a detail. Most of the world refuses a build on purpose —
    /// the clear path, another structure's spacing, a tree, another player — so a harness
    /// that dropped the avatar at the origin would watch every single build fail with
    /// `OnTheTrail` and read as "building is broken".
    pub(crate) fn new() -> Self {
        Self::for_player("p1")
    }

    pub(crate) fn for_player(player: &str) -> Self {
        let balance = std::sync::Arc::new(Balance::load_default().expect("balance loads"));
        let mut arena = meld_world::Arena::generate(&balance, 4242, false);
        for _ in 0..30 {
            arena.ensure_frontier(&balance, 900.0);
        }
        arena.add_avatar(player.to_string(), 5.0);
        let mut inst = meld_run::InstanceRun::new("i1".into(), 0, &balance, 0);
        inst.add_party(vec![(
            player.to_string(),
            "u".to_string(),
            meld_proto::enums::CharacterClass::Explorer,
            "r1".to_string(),
        )]);
        let mut h = Self { arena, inst, balance, player: player.to_string(), tick: 100 };
        assert!(h.stand_somewhere_legal(200.0), "no legal ground to build on at d200");
        h
    }

    /// Stand somewhere a structure may legally go. **Delegates to
    /// `Arena::stand_somewhere_buildable`** — the same probe the `MELD_BUILD` sandbox uses,
    /// so a test and a hand-played session agree about where you can build. It used to be a
    /// private copy here, which is the drift this repo keeps meeting.
    pub(crate) fn stand_somewhere_legal(&mut self, radius: f64) -> bool {
        let b = self.balance.clone();
        let p = self.player.clone();
        self.arena.stand_somewhere_buildable(&b, &p, radius)
    }

    fn run(&mut self) -> &mut meld_run::PlayerRun {
        let p = self.player.clone();
        self.inst.runs.iter_mut().find(|r| r.player_id == p).expect("the party has our player")
    }

    /// Put `qty` units of `kind` in the bag as ONE stack.
    pub(crate) fn give(&mut self, kind: &str, qty: i32) -> &mut Self {
        let item = meld_proto::common::ItemStack {
            item_id: uuid::Uuid::now_v7().to_string(),
            item_kind: kind.to_string(),
            quantity: qty,
            insurance: None,
        };
        self.run().backpack.push(item);
        self
    }

    /// Put `qty` units in the bag the way a HARVEST does: **one unit per stack**. This is
    /// the shape that broke building once already (a 6-cost structure was unbuildable from
    /// six freshly-gathered units), so the harness can reproduce it on demand.
    pub(crate) fn gather(&mut self, kind: &str, qty: i32) -> &mut Self {
        for _ in 0..qty {
            self.give(kind, 1);
        }
        self
    }

    pub(crate) fn held(&self, class: MaterialClass) -> i32 {
        let r = self.inst.runs.iter().find(|r| r.player_id == self.player).unwrap();
        held(r, class)
    }

    pub(crate) fn raise(&mut self, function: &str) -> Result<Charged, String> {
        let b = self.balance.clone();
        let (p, tick) = (self.player.clone(), self.tick);
        let arena = &mut self.arena;
        let run = self.inst.runs.iter_mut().find(|r| r.player_id == p).unwrap();
        raise(arena, run, &b, function, &p, tick)
    }

    pub(crate) fn mend(&mut self, entity_id: &str) -> Result<Charged, String> {
        let b = self.balance.clone();
        let p = self.player.clone();
        let arena = &mut self.arena;
        let run = self.inst.runs.iter_mut().find(|r| r.player_id == p).unwrap();
        mend(arena, run, &b, entity_id, &p)
    }

    pub(crate) fn pack_down(&mut self, entity_id: &str) -> Result<Charged, String> {
        let b = self.balance.clone();
        let p = self.player.clone();
        let arena = &mut self.arena;
        let run = self.inst.runs.iter_mut().find(|r| r.player_id == p).unwrap();
        pack_down(arena, run, &b, entity_id, &p)
    }

    pub(crate) fn newest_id(&self) -> String {
        self.arena.structures.last().map(|s| s.entity_id.clone()).expect("something was built")
    }

    /// A sorted snapshot of the bag, for proving a refusal moved nothing.
    pub(crate) fn bag(&self) -> Vec<(String, i32)> {
        let r = self.inst.runs.iter().find(|r| r.player_id == self.player).unwrap();
        let mut v: Vec<(String, i32)> =
            r.backpack.iter().map(|i| (i.item_kind.clone(), i.quantity)).collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod harness_tests {
    use super::*;

    /// The material a structure is made of, and what it costs — both off the registry and
    /// balance, never a second table here.
    fn stock_for(function: &str) -> (&'static str, i32) {
        let def = meld_proto::structures::structure(function).unwrap();
        let kind = meld_proto::materials::MATERIALS
            .iter()
            .find(|m| m.class == def.material)
            .unwrap()
            .key;
        let bal = Balance::load_default().unwrap();
        (kind, bal.building.spec(function).unwrap().0)
    }

    /// **THE LOOP THIS WHOLE PILLAR RESTS ON.** Gather stock the way a harvest channel hands
    /// it over — one unit at a time, each its own stack — then put a building up with it.
    /// Nothing tested this before: BD-2 shipped the primitive, BD-3 shipped anchors, and both
    /// assumed the join between gathering and building.
    #[test]
    fn a_player_can_gather_stock_and_build_with_it() {
        for function in ["wall", "anchor"] {
            let (kind, cost) = stock_for(function);
            let mut h = BuildHarness::new();
            h.gather(kind, cost);
            let class = meld_proto::structures::structure(function).unwrap().material;
            assert_eq!(h.held(class), cost, "{function}: the gather did not bank");

            let charged = h.raise(function).unwrap_or_else(|e| panic!("{function} refused: {e}"));
            assert_eq!(charged.kind, kind, "{function} charged the wrong material");
            assert_eq!(charged.qty, cost);
            assert_eq!(h.held(class), 0, "{function}: the stock was not spent");
            assert_eq!(h.arena.structures.len(), 1, "{function}: nothing was built");
        }
    }

    /// A build you cannot afford is REFUSED, and the refusal is FREE. A rejection that also
    /// charges you is the worst kind — the rule `Battle::precheck` enforces in combat.
    #[test]
    fn an_unaffordable_build_is_refused_and_costs_nothing() {
        let (kind, cost) = stock_for("anchor");
        let mut h = BuildHarness::new();
        h.gather(kind, cost - 1);
        let before = h.bag();
        let err = h.raise("anchor").expect_err("one unit short must be refused");
        assert!(err.contains("stone"), "the refusal should name the material: {err}");
        assert_eq!(h.bag(), before, "a refused build moved the backpack");
        assert!(h.arena.structures.is_empty(), "a refused build still put something up");
    }

    /// You cannot build a palisade out of masonry. The material comes from the registry, so
    /// holding the wrong kind is the same as holding nothing.
    #[test]
    fn the_wrong_material_does_not_pay_for_a_structure() {
        let mut h = BuildHarness::new();
        h.give("river_granite", 99);
        let err = h.raise("wall").expect_err("stone must not buy a timber palisade");
        assert!(err.contains("wood"), "should ask for wood: {err}");
        assert!(h.arena.structures.is_empty());
        h.raise("anchor").expect("the same stone buys the thing made of stone");
    }

    /// Mending charges one unit of the structure's OWN stock, refuses a sound one, and
    /// refuses when the bag has none — which is what makes an anchor deep in the ash a
    /// logistics problem rather than a chore.
    #[test]
    fn mending_is_paid_in_the_stock_it_was_built_from() {
        let (kind, cost) = stock_for("anchor");
        let mut h = BuildHarness::new();
        h.gather(kind, cost);
        h.raise("anchor").unwrap();
        let id = h.newest_id();

        // ⚠️ FINISH THE BUILD FIRST. A structure goes up WEAK and ramps, so a freshly
        // placed one is already below max HP — and "mend it" on a half-built structure is
        // a real request, not an error. Without this the test asked whether a building
        // still under construction counts as sound, which is a different question and has
        // the opposite answer.
        let build = h.arena.structures[0].build_ticks;
        h.arena.advance_builds(h.tick + build);
        assert_eq!(
            h.arena.structures[0].hp, h.arena.structures[0].max_hp,
            "the build ramp did not finish"
        );

        let err = h.mend(&id).expect_err("a full-HP structure is sound");
        assert!(err.contains("sound"), "{err}");

        let max = h.arena.structures[0].max_hp;
        h.arena.structures[0].hp = max / 2;
        let before = h.bag();
        let err = h.mend(&id).expect_err("no stock must refuse");
        assert!(err.contains("stone"), "{err}");
        assert_eq!(h.bag(), before, "a refused mend moved the backpack");

        h.give(kind, 1);
        let hp_before = h.arena.structures[0].hp;
        let c = h.mend(&id).expect("one unit mends it");
        assert_eq!(c.qty, 1, "mending should cost exactly one unit");
        assert!(h.arena.structures[0].hp > hp_before, "mending healed nothing");
        assert_eq!(h.held(MaterialClass::Stone), 0);
    }

    /// Packing one down returns SOME of the stock, in the same kind it was built from —
    /// never all of it, or moving a structure is free.
    #[test]
    fn packing_down_returns_part_of_the_same_stock() {
        let (kind, cost) = stock_for("wall");
        let mut h = BuildHarness::new();
        h.gather(kind, cost);
        h.raise("wall").unwrap();
        let id = h.newest_id();

        let back = h.pack_down(&id).expect("the owner may pack it down");
        assert_eq!(back.kind, kind, "packing down returned a different material");
        assert!(back.qty > 0, "packing down returned nothing");
        assert!(back.qty < cost, "packing down returned the FULL cost — moving one is free");
        assert_eq!(h.held(MaterialClass::Wood), back.qty, "the refund never reached the bag");
        assert!(h.arena.structures.is_empty(), "still standing after being packed down");
    }

    /// A structure goes up WEAK and ramps, so planting one in front of an oncoming Shift is
    /// a gamble on whether it finishes. Checked through the COST path as well as the arena's
    /// own test, because a handler that placed at full HP would pass every arena test.
    #[test]
    fn a_structure_goes_up_weak() {
        let (kind, cost) = stock_for("anchor");
        let mut h = BuildHarness::new();
        h.gather(kind, cost);
        h.raise("anchor").unwrap();
        let s = &h.arena.structures[0];
        assert!(s.hp < s.max_hp, "a structure at full HP on placement has no build timer");
        assert!(s.hp > 0, "a structure at zero HP on placement is already rubble");
    }

    /// The DEEPEST stock pays first. Hauling tier-4 timber home and spending it on a
    /// palisade is the intended behaviour, and this pins which way round it goes rather than
    /// leaving it to whichever kind the HashMap happened to hand back.
    #[test]
    fn the_deepest_stock_is_what_gets_spent() {
        let mut h = BuildHarness::new();
        let cost = h.balance.building.spec("wall").unwrap().0;
        h.gather("heartoak_log", cost);
        h.gather("bog_root_timber", cost);
        let c = h.raise("wall").unwrap();
        assert_eq!(c.kind, "bog_root_timber", "the deeper stock should be spent first");
    }

    /// Only the owner may take one down — hauling stock to a teammate's anchor is the co-op
    /// verb; taking their work down is not.
    #[test]
    fn only_the_owner_may_pack_a_structure_down() {
        let (kind, cost) = stock_for("anchor");
        let mut h = BuildHarness::new();
        h.gather(kind, cost);
        h.raise("anchor").unwrap();
        let id = h.newest_id();

        // A second player in the SAME world, standing where the first one is.
        h.arena.add_avatar("p2".to_string(), 5.0);
        let here = h.arena.avatar_mut(&h.player).unwrap().position;
        h.arena.avatar_mut("p2").unwrap().position = here;
        h.inst.add_party(vec![(
            "p2".to_string(),
            "u2".to_string(),
            meld_proto::enums::CharacterClass::Explorer,
            "r2".to_string(),
        )]);
        let b = h.balance.clone();
        let run = h.inst.runs.iter_mut().find(|r| r.player_id == "p2").unwrap();
        let err = pack_down(&mut h.arena, run, &b, &id, "p2")
            .expect_err("someone else's anchor is not yours to take down");
        assert!(err.contains("not yours"), "should refuse on ownership: {err}");
    }
}

#[cfg(test)]
mod sandbox_tests {
    use super::*;

    /// **The sandbox's whole claim: you can build the moment you arrive.** A run starts you
    /// at the origin, which is ON the clear path, where every build is refused — so this is
    /// the one thing `MELD_BUILD` has to deliver, and the one a player cannot diagnose
    /// (a refused build and a broken button look identical).
    #[test]
    fn a_fresh_arrival_cannot_build_but_the_sandbox_start_can() {
        let (kind, cost) = {
            let def = meld_proto::structures::structure("wall").unwrap();
            let k = meld_proto::materials::MATERIALS
                .iter()
                .find(|m| m.class == def.material)
                .unwrap()
                .key;
            let b = Balance::load_default().unwrap();
            (k, b.building.spec("wall").unwrap().0)
        };

        // A player standing where a run puts them, carrying plenty: REFUSED, on the trail.
        let mut h = BuildHarness::new();
        h.arena.avatar_mut("p1").unwrap().position = meld_proto::common::Position::new(0.0, 0.0);
        h.gather(kind, cost * 4);
        let err = h.raise("wall").expect_err("the origin is on the clear path");
        assert!(
            !err.contains("takes"),
            "it should be refused for the GROUND, not for affordability: {err}"
        );

        // The sandbox's probe moves them somewhere legal, and then it works.
        assert!(h.stand_somewhere_legal(60.0), "the sandbox found nowhere buildable at d60");
        h.raise("wall").expect("standing on buildable ground, carrying enough");
        assert_eq!(h.arena.structures.len(), 1);
    }

    /// The sandbox hands over enough stock that gathering is not the exercise — and it must
    /// cover EVERY structure, not just the cheap one.
    #[test]
    fn the_sandbox_stock_pays_for_everything_in_the_registry() {
        let b = Balance::load_default().unwrap();
        for def in meld_proto::structures::STRUCTURES {
            let cost = b.building.spec(def.key).unwrap().0;
            assert!(
                cost <= 999,
                "`{}` costs {cost}, more than the sandbox hands out",
                def.key
            );
        }
    }
}
