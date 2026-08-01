# Core Loop & Personas — one world, four ways to play

> **Status: PROPOSED (design framing, not a mechanics spec).** A design note that
> checks a single question: do the systems already specced — adventuring
> ([world-generation](../behaviors/world-generation.md), [combat](../behaviors/combat-atb.md),
> [run lifecycle](../behaviors/run-lifecycle.md)), the [living ecology](living-ecology.md),
> [building & sieges](building-and-sieges.md), [guilds](parties-and-guilds.md), the
> [economy](../behaviors/economy.md) + [Meld skills](../behaviors/meta-progression.md),
> and the persistent world (CANON §W) — actually **compose into one self-reinforcing
> loop** for players who want *different* things? And where are the holes? Companion:
> **[endgame-bosses.md](endgame-bosses.md)** — the shared climax this note points at.

## The question

The pitch: **adventure to the end of the world, helped by friends who may not want to
fight.** For that to be real, the world has to host **several kinds of player at once
and make them need each other.** This note maps four personas, tests the interlock, and
is honest about what doesn't close yet.

**Verdict up front:** the production/consumption interlock is **~80% there** — the
durability sink, material demand, the supply road, and the market already wire the
personas together. But **two loops don't close** (the Crafter's *intrinsic fun*, the
Builder's *income*), and the **shared climax** every persona's output feeds — the
end-world boss ladder — was unspecced until [endgame-bosses.md](endgame-bosses.md). Fix
those three and the design holds.

## The four personas

The player named three; a fourth (**Merchant**) falls out of the economy and deserves
first-class status.

| Persona | The fantasy | Supported by | Its **apex** (win-state) | Its **income** |
|---|---|---|---|---|
| **Adventurer** | push deep, beat the bosses with friends, bring home rare loot | groups/raids ([SOC](parties-and-guilds.md), D5 merge), the anchor push (§W4), Gatekeepers/bosses (FS-4), distance loot, Vanguard board | **The boss ladder → Ometus** ([endgame-bosses.md](endgame-bosses.md)) | loot: gear, chits, materials from the deep |
| **Builder** | carve & hold a town in a hostile world; make it great | the whole [BD epic](building-and-sieges.md) — harvest→builder-mode→verticality→anchors→garrison→siege | **Terim** (hidden boss) + a famous, held town | **the gap** — see below |
| **Gatherer / Crafter** | range for materials, master a craft, make the best loot | [MS](../behaviors/meta-progression.md) harvesting/crafting, ecology materials ([CR §G](living-ecology.md)), the durability sink + stalls/contracts | **All-Father** (gather) + **Terim** (craft) | selling gear/mats; repair fees |
| **Merchant** | run a stall empire, work the market, get rich without a sword | [economy](../behaviors/economy.md) (stalls, contracts, tax), Mercantile skill, the guild vault | a **market empire** / Mercantile mastery | arbitrage + the trade spread |

### Adventurer — strong loop, now with a climax
Everything's there *except* the thing it all pointed at: CANON §W kept referencing "a
seasonal push to a far end-world boss," but the boss was never specced. That's now
[endgame-bosses.md](endgame-bosses.md) (three known bosses → the true end, Ometus). This
is the persona that was already whole; it just needed its ending.

### Builder — strong loop, missing income
The BD epic gives Builders a rich activity and a hidden apex (**Terim**, the god of
building — [endgame-bosses.md](endgame-bosses.md)). The hole is **economic**: a town is
a *cost center* (materials, upkeep, garrison wages) unless it *generates* value. A
Builder needs a reason others' chits flow to them. Options (§"Fixing the loops").

### Gatherer / Crafter — the weak leg (and why)
The player's own instinct — "doesn't feel like a full fun loop" — is right, but the
problem is **not demand**, which is strong:
- **Gathering already inherits extract-or-die tension** — materials are Backpack items,
  lost on death. A gathering run *is* a risk dive with nodes as the objective.
- **The living ecology makes gathering a hunt** — flora that grows, mineral nodes, herds
  that move, the Shift retiling regions with new materials ([living-ecology.md](living-ecology.md)).
  A gatherer **reads a living world**, not a static node map.
- **The durability sink is a demand engine** — every adventurer death degrades gear only
  a crafter can restore, and low-level crafters cap repairs below 100%, so master
  crafters are *permanently* in demand ([economy.md](../behaviors/economy.md)). **The
  adventurers' deaths fund the crafters.**

What's missing is the **crafter's own fantasy** — Adventurers have the boss, Builders
have the town, the crafter has a spreadsheet. The fix is four things (§"Fixing the
loops"): make crafting a *game*, give crafters *reputation*, close the *sink*, and give
them an *apex* (Terim + All-Father already do).

### Merchant — surface it as first-class
Stalls + contracts + tax + the Mercantile skill already *are* a trading meta-game —
buy low, arbitrage, run a stall network, never swing a sword or craft a thing. The specs
treat the market as plumbing; naming the **Merchant** as a real persona (with Mercantile
mastery as its ladder and a market empire as its win-state) costs almost nothing and
gives the least combat-y player a home.

## The interlock — one economy

The personas are healthy only if they **produce what the others consume.** They mostly
do:

```
  GATHERER ──raw materials──▶ CRAFTER ──gear + repairs──▶ ADVENTURER
     ▲                          │                           │
     │                    structures'                  deaths degrade gear
   protected              stone/wood                   (the durability SINK,
   deep access                 │                        the demand engine)
     │                         ▼                           │
  BUILDER ◀──materials── the town/road ──supply line──▶ ADVENTURER's deep push
     ▲                         │                           │
     └──── MERCHANT moves it all, takes the spread; the market clears everyone's output
```

- **Adventurers** consume gear (crafters), the supply road (builders), and mats
  (gatherers); they produce the **deaths** (durability sink) and **deep loot** that fund
  everyone.
- **Builders** consume stone/wood (gatherers) and chits (income gap); they produce the
  **held ground + supply road** that let gatherers reach deep nodes and adventurers push
  further.
- **Gatherers/Crafters** consume protected deep access (builders' roads, adventurers'
  clearing); they produce the **gear + repairs + structure materials** everyone needs.
- **Merchants** consume nothing and produce **liquidity** — the market that clears every
  other persona's surplus and sets prices.

The web is real. It's just **never articulated as one economy anywhere**, and two nodes
leak.

## Fixing the two open loops

### 1. The Crafter's fun & identity (fold into MS / economy)
- **Make crafting a game, not a button.** MS-1 needs *quality rolls*, *recipe
  discovery*, and *signature outputs* — not "spend mats → get item."
- **Give crafters reputation.** A **maker's mark** stamped on crafted gear (an adventurer
  wears *your* blade, with your name on it); Forging/Mercantile leaderboards; guild
  demand. Today a master crafter is invisible — fix that.
- **Close the sink.** Chits must buy things a *crafter* wants: better tools, recipe
  unlocks, stall slots, a workshop plot in a town, cosmetic marks.
- **Give them an apex.** Already solved by the hidden bosses — **Terim** (craft/build
  god) and **All-Father** (the ecology origin) are the Gatherer-Crafter's endgame
  ([endgame-bosses.md](endgame-bosses.md)).

### 2. The Builder's income (fold into BD / economy)
A town must *earn*. Options, best-first:
- **Market hub / rent** — other players deploy stalls in your town (you take a cut / a
  plot fee); a well-sited, well-defended town becomes a *trade post* with real footfall.
- **Defense-for-hire / tolls** — a safe corridor through dangerous distance is worth
  paying for (escort contracts, a toll at your gate for the extraction portal).
- **Resource claims** — a town near rich `MineralNode`s / groves grants its owner a
  share of what's harvested there.
Recommendation: **market-hub rent first** — it reuses the stall system, ties Builders to
Merchants, and makes town *siting* (distance-as-difficulty, BD §E.3) an economic bet.

## Help without fighting — the co-op-without-combat answer

The pitch's load-bearing claim, and it *is* supported:
- **Build & maintain the supply road** — forward towns, anchors, extraction portals,
  respawn hearths (BD / §W4).
- **Garrison it** — hired NPC defenders hold it while the group sleeps (BD-11).
- **Craft & repair the party's gear** — the durability sink (economy).
- **Mid-battle support drops** — heal/revive dropped onto a *battling* ally from the
  overworld, injected into their fight ([async-interaction.md](../behaviors/async-interaction.md)).
- **Escorted gathering** — harvest in the held corridor as the fighters clear.
- **Bankroll the push** — the guild vault + guild bounties fund the mats and gear (SOC).

**The honest constraint:** a non-combat player can't *solo* to the end of the world —
deep distance is lethal. They thrive **in support of, and in the safety of, the combat
push.** That's intended for a co-op MMO, not a bug — but it means the *support roles must
be first-class and rewarding*, which is exactly what the two loop-fixes above and the
hidden bosses deliver.

## Every persona gets an apex (the part that clicks)

The reason the boss roster matters beyond the Adventurers:

| Persona | Apex | Why it fits |
|---|---|---|
| Adventurer | **the 3 known bosses → Ometus** | the deep-raid climax |
| Gatherer | **All-Father** (mountain-slime origin) | found by reading the *ecology* — the gatherer's world |
| Crafter / Builder | **Terim** (god of crafting & building) | reached by *mastery of the craft*, not combat depth |
| Merchant | **a market empire** (+ selling raid consumables into the boss economy) | the boss push is the biggest demand spike of the season |

The hidden bosses (Terim, All-Father) are the elegant close: **the non-combat personas
get their own endgame**, aligned to *their* fun, not a combat gate. See
[endgame-bosses.md](endgame-bosses.md).

## Open questions / recommendations

1. **Is Merchant a real supported persona or just crafter-adjacent?** Recommendation:
   **name it** — it costs little (the market exists) and homes the least combat-y player.
2. **Builder income model** — market-hub rent (recommended) vs. tolls vs. resource
   claims. Pick one to prototype.
3. **How much crafting depth (MS-1)?** The single biggest lever on whether the
   Gatherer-Crafter is fun. Recommend treating MS-1 as a *headline* feature, not plumbing.
4. **Non-combat solo ceiling** — confirm the intended constraint (non-combat players
   support/shelter, they don't solo the frontier). It shapes how safe "held territory"
   must feel.
