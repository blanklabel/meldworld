# Proposal — Crafting depth, and the non-combat "class" question

> Status: **proposed** (design of record for the rest of `MS-1`, and the answer of
> record to "should MELDWORLD have non-combat classes?"). The first slice — the
> material registry, the trophy potion line, the Forge catalyst, recipe level gates
> and the Broker — **ships**; §3–§5 are the unbuilt remainder. Companion to
> [`core-loop-and-personas.md`](core-loop-and-personas.md) (which names the four
> personas) and [`gear-identity.md`](gear-identity.md) (which owns what gear *is*).

---

## 0. What this fixes

Felling a creature banked a **combat drop** — `forest_bloom_petal`, `sun_scarab_husk`,
`ember_cinder`, `frost_shard`, `bog_ichor` — that nothing in the game could spend. No
recipe named one (every recipe took harvest reagents), the Forge treated all materials
as interchangeable filler, and no vendor bought anything. Five item kinds, dropping in
every biome, that a player could only watch accumulate.

That is not a missing feature so much as a **missing taxonomy**: materials were bare
strings, so no system could tell a monster part from a herb, and therefore no system
could ask for one.

### The shipped fix

1. **Materials are a registry with a class** (`meld_proto::materials`) —
   `reagent` (harvest), `ore` (harvest), `refined` (smelted), `trophy` (**combat
   drop**) — plus a tier equal to its biome band (forest 0 → mire 4). Class is what everything else gates
   on, and a `meld-world` unit test asserts every key the world can drop is in it.
2. **The trophy potion line** — six recipes keyed on monster parts, each one step up
   its own effect's ladder from the reagent-line potion it shadows (Verdant Draught,
   Scarab Ward, Cinderblood Philtre, Rimeglass Vial, Ichor Salve, and a Quintessence
   capstone that takes one part from all five biomes). No new combat machinery: a
   potion's `potency` multiplies a magnitude the ATB already models.
3. **Trophies are the Forge's catalyst** — the Forge now demands an *ore* for the
   body of a piece, and accepts an optional *trophy* that buys a tier past what the
   smith's own Forging level can reach and rolls the epic affix pool. **Levelling
   raises the floor; monster parts raise the ceiling.** That is the sentence the
   whole design turns on.
4. **Trophy supply tracks the fight** — a trophy is one unit per felled creature plus
   a band bonus, times the elite/gatekeeper reward spike, and drawn *without* RNG so a
   crafter can plan a hunt (`[loot] material_per_creature`, `material_qty_per_tier`).
   It was a flat one-per-encounter at any depth against any pack size, while chits in
   the same roll already scaled with both — so trophies had no supply curve at all.
5. **Recipes have a permanent level gate** — `RecipeDef::min_level` against the
   crafter's Meld level, refused with a `403` that names the missing level.
6. **The smelt line** — Forging's own craft ladder (§2.4): five recipes turning raw ore
   into **refined stock**, which is what the Forge now builds from. Forging had one
   recipe in the entire game before this.
7. **The Broker buys materials** (`/v1/vendors/broker`) — a floor price under every
   material, scaled by Mercantile level, paying Mercantile XP. Mercantile previously
   had **no XP source at all**; this is its first.

Two sinks, two different jobs. Recipes and the catalyst are *demand* — they make a
trophy worth having. The Broker is *liquidity* — it makes a trophy never worthless.
Priced deliberately low (see [economy.md](../behaviors/economy.md) S3) so selling is
never the optimal play, only the always-available one.

---

## 1. What makes crafting a game rather than a button

[`core-loop-and-personas.md`](core-loop-and-personas.md) called the Crafter "the weak
leg" and diagnosed it correctly: demand is strong (the durability sink, the affix
chase), but the crafter's *own* activity was "spend mats → get item." The four levers
that fix that, in the order they should land:

| Lever | Status | What it adds |
|---|---|---|
| **Reach vs. floor** (levelling gates the tier; a trophy buys past it) | **ships** | a reason to hunt a specific creature for a specific piece |
| **Reroll** (`/reroll` — pay for another draw on the affixes, stats untouched) | **ships** | the gambling loop; what a smith sells is a *chance* |
| **Recipe discovery** | §3 | recipes found in the world, not handed over at a level |
| **Experimentation** (spend a resource to bias the roll) | §4 | skill expressed *inside* one craft |
| **Maker's mark** (the crafter's name on the piece) | §5 | reputation — the thing a master crafter has instead of a spreadsheet |

The randomization the player asked for is already the spine of this: a forged piece
rolls its stat inside a variance band that *narrows* with Forging level, and its
affixes are a tiered draw that can be bought again. Path of Exile is the reference
point — its entire crafting economy is the tension between a deterministic outcome and
a cheaper random one — and the reroll endpoint is that tension in miniature.

### 1.1 Tiers, concretely

Three ladders, deliberately distinct, so "tier" always means one thing in context:

- **Material tier** = biome band (0–4). Drives Broker price and which recipes want it.
- **Gear tier** = `floor(forging_level × gear_tier_per_forging_level)`, `+
  catalyst_tier_bonus` when catalyzed. Drives the stat and which affix pools open.
- **Consumable tier** = shop/recipe rung; `potency` is the separate dose ladder.

---

## 2. Should MELDWORLD have non-combat classes?

**Recommendation: no — and the reason is structural, not a matter of taste.** What the
player is reaching for is real and worth building; "class" is the wrong container for
it in *this* game. Three reasons, strongest first:

1. **Hero levels are ephemeral; professions must not be.** Levels live inside a dive
   (`PlayerRun::run_level`) and reset every run — the *world* is the progression
   ([`progression-and-unlocks.md`](progression-and-unlocks.md)). Meld skills are the
   only thing that never wipes, "not even at season end." A Blacksmith **hero** would
   have its whole identity reset every time you walked out the gate. A Blacksmith
   **skill ladder** is exactly what a profession wants to be, and it already exists.
2. **A party slot is a combat slot.** A player fields up to four heroes on a 100 ms
   ATB; every one of them takes turns and is a target. A non-combat hero is either
   dead weight in a system where a wipe costs your backpack, or it is a combat class
   with a craft-flavoured name. Party slots are also *earned* (level 10/20/30) — the
   most expensive thing a new player unlocks. Spending one on a hero that cannot
   fight is a trap, and players will correctly refuse it.
3. **The persona already has a home.** Four personas are named
   ([`core-loop-and-personas.md`](core-loop-and-personas.md)); three of them are
   non-combat, and each has an activity (Meld skills, the BD epic, the market), an
   income, and an **apex boss** (All-Father for the Gatherer, Terim for the
   Crafter/Builder, a market empire for the Merchant). The gap was never a class
   slot — it was crafting *depth*, which is §1.

### 2.1 What to build instead: professions as a Meld ladder with an identity

Keep the mechanism (Meld skills) and add the two things a "class" was really being
asked for — a **name to be** and a **way to matter in the field**:

- **Profession titles on the Meld ladder.** The combat classes read as promotion
  because their unlocks follow their order's rank ladder (1/2/5/9/13/17 —
  [`factions.md`](../lore/factions.md)). Two of the three professions can borrow that
  directly: **Forging** takes The Foundry's ladder (Indentured Extractor → Master of the
  Foundry) and **Alchemy** takes The Open Flower's (Sprout → Terra). Named ranks, earned
  at levels, shown on the party/profile screen — nearly free, and it buys most of the
  felt identity of a class.

  **Mercantile is deliberately different: there is no merchant order, because merchants
  are just merchants.** No guild, no charter, no rank ladder to promote through — which
  is the right shape for a market the *players* run (GDD §7). Its ladder is therefore not
  a title but a **standing**: the Broker's haggle multiplier, stall slots and placement
  gates ([`economy.md`](../behaviors/economy.md)), and eventually reputation. A merchant's
  rank is what the market will give them, not what an order confers. If a display title is
  wanted, it should read as self-made (a turnover or holdings bracket), never as a
  promotion.
- **Gathering lenses on the existing `[perks]` system** — see §2.2. This is the
  "non-combat play that goes into the field," and it needs no new machinery at all.

Scoped out of MS-1; recorded here so the shape is settled before someone builds a
Blacksmith hero.

### 2.2 The field half is `[perks]`, which already exists

Every class already carries an **overworld perk** that ramps with run level: the
Explorer's minimap (including harvest-node dots at `explorer_map_harvest_at`), the
Hunter's creature intel, the Shifter's dungeon- and trap-sense, the Psyker's threat
marks, the Phoenix Guard's aggro reduction, the Resonant's walking regen. That *is* the
"field artisan" system. A gathering specialization is one more entry in `[perks]`, not
a new concept.

**Three material sources, each with a `find` lens and a `yield` lens.** The find lenses
all ship; none of the yield lenses do:

| Source | **Finds it** | **Yields more of it** |
|---|---|---|
| Harvest nodes — reagents (`alchemy`) | Explorer — node dots on the minimap ✅ | **The Open Flower** (agriculture and balance) |
| Harvest nodes — ore/wood (`forging`) | Explorer — same dots ✅ | **The Foundry** — its Extractors (§2.4) |
| Creatures — trophies | Hunter — level/HP/ATB intel ✅ · Psyker — marks aggressives ✅ | **Hunters** (their trade *is* disposing of creatures) |
| Chests & dungeon loot | Shifter — dungeon sense + insured-vs-ephemeral item sense ✅ | **Shifters** (the salvage order) |

Splitting *find* from *yield* is what keeps two gatherers from being a dominance
ordering: the Explorer tells you where the vein is, the miner gets more out of it.
Both are worth bringing, neither is a worse copy of the other.

### 2.3 Soft gates, hard byproducts

**No material is ever class-locked.** A hard gate reads as interdependence only in a
game with one character per player (SWG). Here a player fields four heroes and
eventually owns every class, so a hard gate degrades into a checklist — *and* it walls
off the one-hero era, when a new player has a single Explorer and no way to unlock
anything else (`CL-1`). Three layers instead:

1. **Base material — ungated.** Anyone who finds a node or fells a creature gets it.
2. **Yield — a soft multiplier** from the specialist's perk: quantity, harvest-channel
   speed (once `MS-2` lands), Meld XP rate.
3. **A rare byproduct — hard-gated, on a *new* item.** The seed, the living cutting,
   the intact organ, the pristine salvage: only the specialist produces it. Nobody is
   walled out of the economy or the recipe book, but the specialist owns the sole
   supply of a top-end input — which is exactly what makes them *a person other
   players need*.

**The tuning rule: worth bringing, never required.** If a specialist doubles all
yields, every party must slot one and the choice evaporates. Keep the base-material
bump modest and put the exclusivity only in the byproduct, which is optional content by
construction.

**Gate the byproduct on a rank, not on presence.** Put the byproduct behind a level
threshold the way `hunter_intel_atb_at = 6` does, so the monopoly belongs to a
*developed* specialist rather than a slotted one.

This has to be **stated as a rule, because nothing enforces it for free.**
[`compute_perks`](../../server/crates/meld-server/src/game.rs) is a pure *presence*
check — `classes.contains(&c)` — scaled by the player's **shared** `run_level`, which
follows their best hero, not the specialist's own. So a specialist you bring and never
play carries exactly the same perk as one you built; there is no self-balancing to lean
on, and any "a developed specialist earns more" rule is a rule someone must write.

**Granularity is the party, not the hero.** The overworld avatar is the whole party
(one avatar per player; heroes only materialize as combatants in battle), so the check
is "does this party contain class X" — which the run already knows from the persisted
hero classes. There is deliberately no notion of *which* hero harvested.

### 2.3a Stacking a specialist buys **tempo**, not yield

A pure presence check makes four of one class worth exactly one of it, so a mono-party
pays four slots for a single lens *and* eats the party-size penalty — creature HP scales
`[1.0, 1.9, 3.0, 4.4]` with party size and XP splits across it. Both of these orders are
support/control archetypes, so **a mono-gatherer party cannot fight**: four low-damage
heroes against 4.4× HP is a fight that does not finish. Strictly dominated by "one
specialist plus three fighters," which makes the 4-stack a mistake rather than a build.

**The rule that fixes it: the first specialist sets the yield lens; every additional one
makes that trade's timed actions faster.**

| Class | Order | Stacking speeds up |
|---|---|---|
| **Keeper** | The Open Flower | harvesting, farming, planting — and tending a town's groves |
| **Smithwright** | The Foundry | building (walls included), repairing, smelting, forging |

It works because it never inflates *supply* — a node still gives what it gives, so
there is no economy pressure — while paying the mono-party in the one currency a
vulnerable channel makes precious: **time exposed**. Four pairs of hands dig faster; the
vein does not get bigger. That yields a real spectrum instead of a trap:

- **1 specialist** — the balanced party's yield bump.
- **4 specialists** — a fast, fragile raid on a resource field you must not be caught
  in: maximum tempo, no combat capability, and the whole backpack lost if something
  reaches you.

**This makes `MS-2` load-bearing rather than optional.** If the stack buys speed, then
every profession action must *have a duration to speed up*: the harvest channel (`MS-2`),
smelting, building, repairing, planting. Instant actions cannot be accelerated, so a
profession built on tempo cannot exist until its verbs take time. Harvesting stops being
a free tap and becomes the commitment the whole design rests on.

*Later, better, for the Foundry specifically:* the fiction says a Foundry crew is
**mixed-caste** — Extractor, Smelter, Smithwright — not four identical smiths, so "a
party of Smithwrights" ideally means a *work crew* with different jobs and different
lenses. That wants a sub-role on a hero (a caste choice, a wire field, UI), so it is the
expensive version of this idea and should not be designed away before it can be built.

### 2.4 Who mines? — **The Foundry**

Forging's home order is **The Foundry**: the subsidized, quota-driven, strictly audited
branch of the city government that supplies the structural iron and magitech metals the
Stabilizing Towers, the Great Ivory Wall and the Power Grid are made of. Not an
adventuring guild — an *industry*, which is why it reads differently from every other
order and why its labour is drawn from citizens in debt.

Its three castes are already, exactly, the Forging material pipeline:

| Caste | Fiction | Mechanically |
|---|---|---|
| **Extractors** (Rank 1) | rip resources out of the Shifting Lands and the Slag-Fields; indentured, high mortality | **ore/wood harvest yield** — the `forging` nodes |
| **Smelters** (Rank 2) | boil the corruption and magical volatility out of raw ore to stabilize it | **the smelt line** — raw → `refined`, *ships* |
| **Smithwrights** (Rank 3–6) | magitech components, riveted plating, structural armor | **the Forge** — gear crafting from refined stock, `forgeable_tier` |

And its rank ladder already lands on MELDWORLD's: Rank 3 at character level 5, Rank 4 at
9, Rank 5 at 13, Rank 6 at 17 — the same 1/2/5/9/13/17 rungs every order's ability
unlocks use ([`skills.rs`](../../shared/meld-proto/src/skills.rs)). Nothing needs
inventing; the ladder was already built to fit.

**The Smelters named a mechanic we didn't have — it now ships.** "Boiling away the
corruption or magical volatility from raw ores" is a **raw → refined** step, and it
filled a real hole: Forging had exactly *one* recipe in the whole game (the Town Portal)
against Alchemy's thirteen. A profession with a single craft is not a profession.

Shipped as the **smelt line**: a fourth [`MaterialClass::Refined`](../../shared/meld-proto/src/materials.rs)
with one refined form per ore (Heartoak Stave, Dune/Cinder/Rime/Peat Ingot, each in its
ore's own band so smelting cannot launder shallow material into deep gear), five
`forging` recipes at **two raw for one refined** — the loss *is* the cost of stabilising
it — and **the Forge now builds from refined stock**, refusing raw ore with a message
that names the smelt to run. So a Smithwright's pipeline is `harvest ore → smelt → forge`
rather than a single tap, and the Smelter caste has a job.

The `min_level` ladder rises by band (1 / 2 / 4 / 6 / 8), which is where the decision
lives: **ore you cannot yet work is ore worth banking** until your Forging catches up.
Refined stock also out-prices its ore at the Broker, because a Smelter's labour is in it.

*Still open (§4):* the **volatility gamble** — a level-scaled recovery rate, so smelting
early costs you material rather than merely being unavailable. The shipped version gates
by level instead of rolling, which is simpler and has no RNG in it; the gamble is the
richer version and belongs with Experimentation.

**Ore is deliberately still ungated by CLASS-yield, for now.** It is the one input every gear craft needs, so
until the Foundry exists as a playable class, ore stays the democratic material and the
Extractor yield-lens is a `[perks]` entry waiting for its class. §2.3's rules apply to it
unchanged when it lands.

> **Canon conflict to resolve — "One" vs Terim.** The Foundry worships **"One" (the Forge
> God)** as a state-mandated pseudo-religion of industrial efficiency, and puts **Terim**
> on the Stabilizers and the Healery instead. But
> [`endgame-bosses.md`](endgame-bosses.md) and roadmap **`EW-6`** make Terim *the God of
> Crafting & Building* and the Crafter/Builder apex boss. Both cannot hold. Three ways
> out: (a) **One** takes the craft domain and `EW-6`'s hidden boss becomes One, with
> Terim keeping stability/healing; (b) Terim keeps craft and "One" is the Foundry's
> *heresy* — a state cult that industrialized a god of making into a god of throughput,
> which is thematically excellent and costs the roadmap nothing; (c) they are the same
> entity under two names, the city's official one and the older one. **(b) is
> recommended** — it preserves `EW-6`, and "the state renamed your god as a productivity
> metric" is exactly the Foundry's voice.

### 2.5 Who does this well — and what each one teaches

| Game | What it did | The transferable lesson |
|---|---|---|
| **Star Wars Galaxies** (2003, pre-CU) | Artisan → Armorsmith/Weaponsmith/Chef/Merchant/Bio-Engineer as **full professions** with their own skill trees; galaxy-wide resource quality that *shifted weekly*, an experimentation step, factories, player malls, signed items | The canonical answer, and it worked because **crafters were the only source of good gear** and because the fun lived in *prospecting + experimentation + your name on the item* — not in the recipe button. It also needed a real population and a real market. |
| **FFXIV** | 11 Disciples of the Hand/Land, first-class jobs with their own levels, gear and an **active crafting minigame** (progress/quality/CP rotations) | Put professions on the **same character, in a different mode** — never in a combat party slot. Also: crafting can carry a genuine skill-expression layer. |
| **EVE Online** | Industry and trading as complete careers; account-level skill training, no classes | Non-combat mastery does not need a class *or* a body — it needs a market deep enough that logistics is strategy. |
| **Black Desert** | Life skills (gathering/alchemy/processing/trade) with mastery ladders, plus **workers and nodes** running a semi-idle production empire | Give the non-combat player something that **compounds while they are away** — a good fit for an asynchronous MMO. |
| **Ultima Online** | GM Blacksmith as a *build*, under a shared skill cap | The tradeoff *is* the identity: choosing crafting meant giving up combat. If nothing is given up, nothing is chosen. |
| **Mabinogi** | Non-combat "talents" (Blacksmith, Cook, Tailor, Musician) with titles | Titles and talents deliver most of the identity of a class at a fraction of the cost. This is §2.1. |
| **Path of Exile** | Crafting as currency gambling — essences, fossils, reroll spam | Depth can come *entirely* from deterministic-vs-random tension. The reroll endpoint is this. |
| **WoW / GW2** *(counter-example)* | Crafting as a side bar on a combat character; materials are vendor trash | The failure mode to avoid: professions that are a **second progress bar** rather than a way to play. Named here so we can tell when we are drifting into it. |

The synthesis MELDWORLD should land on is **FFXIV's containment** (professions live off
the combat party), **SWG's texture** (world-dependent materials, experimentation, your
name on the blade), **BDO's asynchrony** (it compounds while you are offline), and
**PoE's gambling** (the reroll is the loop). We already have the last one.

---

## 3. Recipe discovery *(unbuilt)*

Level gates alone make the recipe book a wall-chart. Better: a recipe is **found** —
dropped by a creature whose parts it uses, in a dungeon chest, or from a Broker who
sells the sheet but not the skill. `min_level` stays as the *floor*; discovery is the
*key*. Needs a `known_recipes` table and a drop hook; the level gate that ships is the
half of this that works without one.

## 4. Experimentation *(unbuilt)*

One craft, one decision: spend extra material (or a second trophy) to bias the roll —
narrow the variance band, guarantee one affix class, or push one tier. SWG's
experimentation and PoE's essences are the same idea. This is where a master crafter's
*play* lives, as opposed to their level.

## 5. Maker's mark & crafter reputation *(unbuilt)*

Stamp the crafter's name on a forged piece (`gear.maker`), show it in the tooltip, and
rank Forging/Alchemy on their own leaderboards. This is the cheapest identity fix in
the whole document and the one `core-loop-and-personas.md` calls for by name: today a
master crafter is invisible.

---

## Open questions

1. **Does the Broker need a faucet cap?** It mints chits (source S3) with no ceiling.
   Recommendation: ship uncapped, watch `Σ chits`, and add a per-day cap only if it
   actually inflates — a cap needs persistence a floor price may not deserve.
2. **Do trophy potions belong on the Apothecary shelf?** No, currently — they are the
   reward for hunting, and a shelf copy would undercut the sink. Revisit if new
   players cannot reach the trophy line.
3. **Should the catalyst also be *required* past some tier?** It is optional today
   (a bonus, not a tax). If deep gear should demand hunting, a tier floor above which
   an uncatalyzed forge is refused is the lever.
4. ~~Where do profession ranks live in the fiction?~~ **Settled.** **Forging → The
   Foundry** (§2.4); **Alchemy-side gathering → The Open Flower**; **Mercantile → no
   order, deliberately** — *merchants are just merchants* (see §2.1).
5. **Does trophy yield need a cap?** Trophy quantity now scales with pack size × depth
   × the elite spike (`[loot] material_per_creature`, `material_qty_per_tier`). A deep
   gatekeeper pack is a large single haul; if that outruns recipe demand, the lever is
   `material_qty_per_tier` before anything structural.
