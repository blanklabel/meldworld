# Adventure Depth — gear, affixes, party synergy & the competitive chase

> **Status: PROPOSED (design only).** The retention layers the Adventurer persona is
> thin on ([core-loop-and-personas.md](core-loop-and-personas.md)): the systems that
> turn "a working dive" into "one more dive." The combat *engine* (ATB, class kits, the
> barrier/regen/evasion/adrenaline/focus status layer — [combat-atb.md](../behaviors/combat-atb.md))
> and the *destination* (the boss ladder, [endgame-bosses.md](endgame-bosses.md)) are
> strong; this adds the **build-crafting, loot chase, directed goals, and competitive
> boards** in between. Builds on: gear slots + durability (`GR`), class unlocks (`CL-1`),
> party rows (`PT-1`), loot rarity banding ([world-generation.md](../behaviors/world-generation.md)),
> the bestiary (`CR-5`), named bosses (`FS-4`), crafting (`MS-1` / Terim), and the
> Vanguard board ([endgame-seasons.md](../behaviors/endgame-seasons.md)). Tracked as new
> epic **AD** in [`../ROADMAP.md`](../ROADMAP.md).

## Design principle: **the party is the build** (not a stat sheet)

A player commands **four heroes**. Per-character talent trees / skill-point allocation
would mean **four build pages to micromanage** — the wrong kind of depth. So the "build"
does **not** come from stat allocation (attributes stay **auto-gained per level**, as
today — CLAUDE.md). It comes from three composable axes:

1. **Party composition** — *which* classes you field (from your unlocked roster) and
   their **rows/roles** (`PT-1` front/back).
2. **Gear & affixes** — what each hero wears, and the **affixes** those pieces roll (Part
   A — the star of this epic).
3. **Synergies** — the mechanical interactions those choices unlock *between* heroes
   (Part C).

> **This makes MELDWORLD a *team-composition* ARPG, not a character-sheet ARPG.** The
> build-crafting puzzle is "what four heroes + what gear + what synergies = a combo,"
> which is *distinctive*, fits the 4-hero model, and avoids 4× micromanagement. It's
> also the connective tissue that ties the personas together: **adventurers chase
> affixed gear, crafters roll it (`MS-1`/Terim), merchants trade it, builders' Terim
> boss drops the recipes** — the loot economy closes the loop
> ([core-loop-and-personas.md](core-loop-and-personas.md)).

---

## Part A — Gear & affixes (the loot chase — "get really neat")

The star. Extends the gear model ([gear-item-models.md](../interfaces/data-models/gear-item-models.md),
`GR-1` slots) with a real **affix system** so drops can *change how you play*, not just
raise a number.

### A.1 The affix model
- A `GearItem` rolls **affixes** from tiered pools at drop/craft time. Rarity = the
  **count + power** of affixes (common → … → legendary), **banded by distance/tier**
  (rides the existing loot rarity banding + red-chest floor,
  [world-generation.md](../behaviors/world-generation.md); deeper = rarer pools, CR-1).
- Affixes are **server-rolled** (CANON §S); the client renders them. Red-chest gear (the
  best rolls) still burns on death unless extracted — the affix chase inherits
  **extract-or-die** stakes.

### A.2 Three affix classes (the depth ladder)
| Class | What it does | Example |
|---|---|---|
| **Stat affixes** | the bread-and-butter numbers | `+Str`, `+Max HP`, `+crit`, `+dodge` |
| **Keyword affixes** | *build-defining* — twist a class's own mechanic | "Attacks build **+50% Adrenaline**, skills cost +25%"; "**+1 Focus slot**"; "Barriers you grant **decay 50% slower**"; "on kill, **grant an ally a turn**" |
| **Synergy affixes** | the neat part — reference *allies* | "When an ally spends **Adrenaline**, deal bonus damage"; "your **Regen** also grants the target **Evasion**"; "on **Flicker**, an ally also gains Evasion" |

Keyword + synergy affixes are what make gear a *build*, and they're written against the
**existing class mechanics** (Adrenaline/Focus/Barrier/Regen/Evasion/Manifestations) so
they compose immediately.

### A.3 Uniques & sets
- **Uniques / named legendaries** — hardcoded, build-defining items with a signature
  effect + a *tradeoff* (the Bloodfang-class named gear already in code). E.g.
  *"Bloodfang, the Frenzied Cleaver — Frenzy costs no Adrenaline, but this hero cannot
  Defend."* Anchors a whole comp around it.
- **Sets** — multi-piece bonuses that reward committing slots/heroes to a theme, often
  **party-wide** (the synergy angle). E.g. *"Cradle-Warden (3 pieces): when any ally
  drops below 30% HP, grant the whole party Barrier."*

### A.4 Crafting & the crafter link (`MS-1` / Terim)
Crafters **roll, reroll, and upgrade affixes** (the `MS-1` crafting game
[core-loop-and-personas.md](core-loop-and-personas.md) calls for), stamp a **maker's
mark**, and Terim (the hidden crafting-god boss, [endgame-bosses.md](endgame-bosses.md))
drops **legendary recipes**. Gems (existing socketing) slot on top. So the adventurer's
chase *is* the crafter's demand — one system, two personas.

---

## Part B — Class unlocks = build breadth (`CL-1`)

Each **class unlock** (`CL-1`; Gatekeeper emblems + town hires) adds a possible team
member — and therefore new comps and new synergy axes. The roster *is* the build palette;
unlocking classes widens it. No new system here — AD **leans on `CL-1`** and treats the
unlocked roster as the first build axis.

---

## Part C — Party synergies (the build engine)

Where the depth lives without stats. Explicit, legible mechanical interactions the player
assembles:

- **Class-pair synergies** — e.g. Iron Hull's **wall/stance** + Psyker's **Barrier**
  stack into a fortress front; Resonant's **Regen** amplified when paired with a
  high-HP-cost kit; Shifter's **Evasion** blink covering a fragile back line.
- **Row/position synergies** (`PT-1`) — front/back placement changes reach, target
  priority, and which synergies are live.
- **Affix-driven synergies** (Part A.2) — the reconfigurable layer that lets *any* comp
  find a combo.

**Sequenced combos (shipped addition to this part).** Beyond passive pairings, one hero's
ability *primes* a target and a specific follow-up cashes it in for amplified damage inside
a short window — Snare then Backstab, Gravity Well then Kinetic Shock. Three of the four
combos require two different classes, which is what turns a turn order into a party
decision rather than four independent menus. Primers are consumed on payoff and expire, so
a single setup turn cannot be banked.

**Surface them.** The party screen (inventory overlay) shows **active synergies** so a
player can *see* what their comp+gear enables and *chase* new ones — the build feedback
loop. This is the "aha, these four + this affix = a combo" moment that replaces a talent
tree.

---

## Part D — Elemental affinities & resistances (make damage types matter)

The lore is full of damage types (Force, Psychic, Radiant, Bludgeoning, Lightning,
Shadow) and biome hazards — but combat has **no** weakness/resist mechanic today. Make
them real:

- Creatures/bosses/biomes carry **affinities** (weak/resistant/immune to types).
- Gear/affixes grant **resistances** or **convert/add** a damage type — so adapting your
  comp + gear to the biome is a *decision* ("bring Lightning resist to the Brass Corpse";
  "Nestiph's spores are Shadow — pack Radiant").
- Makes the **biomes mechanically distinct** for combat, not just flavor, and gives
  gear/affix choice a tactical reason beyond raw stats.
- **Legibility (accessibility, `UX-2`):** weaknesses are **telegraphed** (nameplate/icon,
  never color-only) so this reads as strategy, not guesswork.

---

## Part E — The Hunt Board (directed goals — the mid-game spine)

Today the only goals are "go deeper" (Vanguard) and "beat the end bosses" (EW). The
mid-game needs **directed combat objectives** — a **Hunt Board** (combat-facing, distinct
from the economy's *gathering* bounties):

- **Hunts:** track & kill a **named creature** (`FS-4`), clear a specific **dungeon**
  (`DG`), "fell X at depth ≥ Y," survive a keystone (Part F), win a swarm/turf event (CR).
- **Ties the bestiary (`CR-5`):** hunts drive codex completion; the codex surfaces hunt
  targets.
- **Co-op/guild hunts (`SOC`):** shared objectives a group/guild pursues together.
- **Rewards:** chase gear + affixes (Part A), currency, **reputation**, and
  **leaderboard** points (Part G).

This gives the Adventurer a *reason to dive* between the tutorial and Ometus — purpose,
not just depth-for-its-own-sake.

---

## Part F — Keystone modifiers (optional challenge → better loot)

A lever to **dial difficulty up for better rewards without going further out** — replay
value + a reward knob + a competitive axis. Seeds directly from **`FS-4` champion
affixes** (Swift/Brutal/Armored/Giant/Vicious already exist):

- A **keystone** applies stacking **modifiers** to a dive/dungeon (e.g. "creatures
  reanimate," "no extraction for 5 min," "double swarm density," "Shifts twice as often")
  in exchange for **higher affix tiers / rarity**.
- Think PoE maps / D3 rifts / mythic+ keys, in the MELDWORLD idiom (a **cursed dive** or
  **keystone dungeon**).
- **Competitive:** highest modifier cleared / fastest clear feeds a leaderboard (Part G).

---

## Part G — Leaderboards (the competitive chase)

The Adventurer's **persistent, seasonal, competitive endgame** — expanding the single
existing board ([endgame-seasons.md](../behaviors/endgame-seasons.md), Vanguard = deepest
distance) into a **board suite**. All follow the Vanguard rules (server-authoritative,
anti-forgery, per-`Season` D8, archived read-only at season end, top ranks grant
titles/cosmetics):

| Board | Ranks by | Ties |
|---|---|---|
| **Vanguard** (exists) | deepest `distance` reached per instance | world-generation |
| **Boss Ladder** | first / fastest end-world boss clears; Ometus first-kill | `EW` |
| **Keystone** | highest modifier cleared / fastest keystone clear | Part F |
| **Hunt / Codex** | hunt completion / bestiary completion | Part E, `CR-5` |
| **Guild** | best/aggregate member result | `SOC` (already flagged B.9) |

Boards are the horizontal, never-ending chase (climb the ranks) *and* the season's
prestige story (first-clears, titles). Rewards persist across seasons; ranks reset (the
endgame-seasons "not wiped" rule).

---

## Data models / wire (additive)

| Model / field | Summary |
|---|---|
| `GearItem` (extend) | `+ affixes: [Affix]`, `+ item_class` (normal/unique/set), `+ set_id?`, `+ maker_mark?` |
| `Affix` (new) | `key` (stat/keyword/synergy), `tier`, `values`, `pool_id` — server-rolled |
| `AffixPool` / `UniqueDef` / `SetDef` (content) | tiered affix pools, unique defs, set-bonus defs (distance/rarity-banded) |
| `Synergy` (content) | a named interaction (class-pair / affix-driven) surfaced on the party screen |
| `Affinity` (content) | per-creature/biome damage-type weak/resist/immune; gear resist affixes reference it |
| `Hunt` (new) | a combat objective: target, condition, reward, expiry — the Hunt Board |
| `Keystone` (new) | a modifier set applied to a dive/dungeon → reward multiplier |
| `LeaderboardEntry` (extend) | generalizes `VanguardBoardEntry` to a `board_kind` (vanguard/boss/keystone/hunt/guild) |

Wire is additive: affixes/synergies/affinities ride the existing combat + gear surfaces
(the `statuses`/`key:value` token convention where per-combatant); Hunt Board + boards
read over HTTP (persistent); keystones are a dive-config the server applies. No renames.

## Balance tunables (new `[affix]` / `[adventure]` blocks)

Per-tier affix roll weights + value ranges; unique/set drop rates; affinity multipliers
(weak/resist/immune); keystone modifier → reward-multiplier curve; hunt reward tables;
per-board title thresholds. All **[TUNABLE]** behind `meld-balance`.

## Build plan — Epic AD (phased)

- **AD-1 — Gear affixes & the loot chase (the star).** The affix model, three affix
  classes (stat/keyword/synergy), rarity/distance banding, uniques + sets. Extends `GR-1`
  + gear-item-models. *The single highest-leverage item — it's what "crazy grinding"
  runs on.*
- **AD-2 — Party synergies + surfacing.** Class-pair + affix-driven synergies; the party
  screen shows **active synergies**. Depends on AD-1 (synergy affixes) + `PT-1` (rows).
- **AD-3 — Elemental affinities & resistances.** Damage-type weak/resist/immune on
  creatures/biomes; resist/convert affixes; telegraphed (`UX-2`). Extends
  [combat-atb.md](../behaviors/combat-atb.md).
- **AD-4 — The Hunt Board.** Directed combat objectives (named creatures/dungeons/depth);
  ties `CR-5` bestiary, `FS-4`, `DG`; co-op/guild hunts (`SOC`); rewards feed AD-1 + G.
- **AD-5 — Keystone modifiers.** Optional challenge scaling for better loot; seeds from
  `FS-4` affixes; feeds the keystone leaderboard.
- **AD-6 — Leaderboard suite.** Generalize the Vanguard board into boss/keystone/hunt/
  guild boards; seasonal, titles/cosmetics. Extends [endgame-seasons.md](../behaviors/endgame-seasons.md).

**Ordering:** `AD-1` first (the chase is the engine), then `AD-2` (synergies make the
chase *mean* something) → `AD-3`/`AD-4` (tactical + directed depth) → `AD-5`/`AD-6`
(replay + competition). Class unlocks (`CL-1`) is a parallel prerequisite for build
breadth. AD is **not** gated on `SC-3` — it's account/run-level, so it can proceed on the
current build alongside the world-sim epics.

## CANON deltas to fold in

- **New D — Builds are gear + roster + synergy, not stat allocation.** Attributes stay
  auto-gained; character customization comes from party composition, gear affixes, and
  synergies. (Refines the GDD "Training Ground / Build Templates" gesture into *loadout*
  templates, not skill-point trees.)
- **New D — Affix system.** `GearItem` carries server-rolled affixes (stat/keyword/
  synergy) from distance-banded tiered pools; uniques + sets; rolled/rerolled by crafting
  (`MS-1`).
- **New D — Damage-type affinities.** Creatures/biomes have weak/resist/immune affinities;
  gear grants resistances; telegraphed (`UX-2`).
- **New §/refine — Leaderboard suite.** Generalize `VanguardBoard` to multiple seasonal
  `board_kind`s (vanguard/boss/keystone/hunt/guild).
- **Glossary (§G):** `Affix`, `UniqueDef`, `SetDef`, `Synergy`, `Affinity`, `Hunt`,
  `Keystone`, `board_kind`.

## Open decisions (yours to call)

1. **How build-defining should affixes get?** Conservative (mostly stat + light keyword)
   vs. aggressive (synergy affixes that hard-enable comps, PoE-style). Recommendation:
   **aggressive on keyword/synergy** — it's the whole "get really neat" ask — but gate the
   wildest ones behind uniques/sets so they're a *chase*, not baseline.
2. **Affix acquisition: drops vs. crafting.** Do the best affixes drop, or must they be
   crafted/rerolled (forcing the crafter link)? Recommendation: **both** — drops seed the
   pool, crafters reroll/perfect them (keeps the crafter loop load-bearing).
3. **Do synergies need explicit "synergy" objects, or just emergent from affixes?**
   Recommendation: **a few authored class-pair synergies** (legible, teachable) **+**
   open-ended affix-driven ones (the deep end).
4. **Leaderboard scope** — per-world, per-region, or global per season? Recommendation:
   **global per board per season** (like Vanguard today), with a **guild** cut.
5. **Keystone stakes** — do keystone modifiers raise the *death* stakes too (bigger loss),
   or only the reward? Recommendation: **both, opt-in** — the extract-or-die tension is
   the point.
