# Meta-Progression

Persistent account progression that survives runs: Outer Hub unlocking, base run levels, character class unlocks, pre-run build allocation at the Training Ground, world-wide resource stratification, and the three permanent Meld Skills (Forging, Mercantile, Alchemy). Everything in this file is **persistent state** owned by the server and mutated through the HTTP API (or by the server itself at run end); nothing here is wiped by death, extraction, or season rollover.

**Source:** GDD.md §3, §4, §4.1; CANON.md §B (Hubs & run levels, Death & durability, Economy), §D (D4, D6, D7, D9), §G

Related specs: [economy.md](economy.md) (stalls, contracts, tax — the Mercantile sinks/sources), [endgame-seasons.md](endgame-seasons.md) (what is and is not wiped at season end), [../edge-cases/limits.md](../edge-cases/limits.md) (consolidated limits).

---

## Hub Unlock Flow

Outer Hubs are unlocked **per player** (per `Player` account, not per instance or per party). The shipped registry is `meld_proto::hubs::HUBS` — **7** hubs, listed below. The Center Hub (`d = 0`) is always unlocked.

**As shipped (PG-2), a hub is a lookup, not an entity.** The gate is the account's own all-time deepest distance, taken from the `vanguard` table (written off *validated movement*, read across **all** seasons — a season rollover must not revoke ground you stood on). A run reads one integer: `hubs_reached(deepest)`. There is no ruined camp to rebuild and no Gatekeeper kill required to open one, and a requested hub is **clamped to the deepest reached, never rejected** (exactly as `party` is clamped to owned classes). The ruined-camp/rebuild flow below the table is the original design and is **not built**; the `Structure` primitive it needs is roadmap epic `BD`.

**Source:** GDD.md §3 (Gatekeeper Bosses, Persistent Milestones), §4; CANON.md §B (Hubs & run levels), §G (Hub, Gatekeeper)

### Flow

1. A party reaches the Gatekeeper arena at `d = 500k − 1`. The arena is a full-width chokepoint: no path past it exists until the instance's per-instance clear flag is set.
2. The party defeats the `GatekeeperBoss`. The server sets the instance's clear flag (per `MazeInstance`, so the instance can now path past the arena) and rolls Gatekeeper drops (see Class Unlocks below).
3. Every `Player` who was a combatant in the killing `Battle` at the moment of the kill is credited with the defeat: their per-player hub state for the hub at `d = 500k` transitions `locked → camp_accessible`. Players in the merged battle from other instances are credited equally.
4. The ruined camp at `d = 500k` becomes enterable for credited players during any run. Inside, the player may **rebuild** the camp by paying the rebuild cost (chits and/or materials from Vault, defined by content tables **[TUNABLE]** — not specified in GDD/CANON; see Notes).
5. On rebuild, that player's hub state transitions `camp_accessible → active`. The hub is now a full persistent safe zone for that player: they may start runs from it, deploy stalls in it (subject to Mercantile gates, see [economy.md](economy.md)), and use its Training Ground.

### States / Transitions (per player, per outer hub)

| From state | Event | To state | Side effects |
|------------|-------|----------|--------------|
| `locked` | Player is combatant in the Battle that kills the guarding Gatekeeper | `camp_accessible` | Credit recorded with timestamp; ruined camp enterable |
| `camp_accessible` | Player pays rebuild cost at the camp | `active` | Rebuild cost debited (HTTP, atomic); hub selectable as run departure point |
| `active` | — | — | Terminal. Hub unlock state is never revoked; NOT wiped at season end ([endgame-seasons.md](endgame-seasons.md)) |

### Error cases

| Condition | Error code | HTTP status |
|-----------|------------|-------------|
| Rebuild attempted while state is `locked` | `forbidden` | 403 |
| Rebuild with insufficient chits/materials | `insufficient_funds` | 409 |
| Rebuild attempted when already `active` | `conflict` | 409 |
| Starting a run from a hub not `active` for that player | `forbidden` | 403 |

---

## Base Run Level

Entering the Maze resets the party's ephemeral `run_level` to a value determined solely by the departure hub. All party members receive the same base run level (the departure hub is a property of the run, and matchmaking filters by departure hub per CANON §D13).

**Source:** GDD.md §2.2, §4 (Base Level Scaling); CANON.md §B (Hubs & run levels), §D (D4, D13)

### Formula

```
base_run_level(hub) = round(1 + hub.distance × 0.078)
```

Rounded to nearest integer. **[TUNABLE]**

### Hub table (`meld_proto::hubs::HUBS`)

| Hub | `key` | Distance | base_run_level |
|---|---|---:|---:|
| The Center Hub | `center` | 0 | 1 |
| First Reach | `first_reach` | 500 | 40 |
| The Span | `the_span` | 1000 | 79 |
| Cinderwatch | `cinderwatch` | 1500 | 118 |
| The Lastward | `the_lastward` | 2000 | 157 |
| Hollow March | `hollow_march` | 2500 | 196 |
| The Threshold Deep | `the_threshold_deep` | 3250 | **255** |

`base_run_level(d) = round(1 + 0.078 × d)` (`meld_proto::hubs::start_level`, held against the
server's copy by a test so the two cannot drift). `d = 3250` is the deepest hub because the
formula reaches `max_hero_level` (255) at `d ≈ 3256`: past that a hub buys nothing while
creatures keep scaling, which makes that distance the **structural end of the game**. The old
`0…5000` / 11-hub ladder (base_run_level up to 391) predates the 255 level cap and is retired.

Hero level is capped at `[runs] max_hero_level` = 255 **[TUNABLE]**; it grows during the run with XP, and the curve is stated in **fights**, not points — see [CANON §B](../CANON.md) *Hubs & run levels* (`meld_run::xp_to_next`). On death, accumulated run levels are deleted (see [economy.md](economy.md) for the durability consequence).

---

## Class Unlocks (ClassEmblem Drops)

Character classes beyond the default `explorer` are permanent account unlocks obtained as `ClassEmblem` drops from Gatekeeper Bosses (e.g. defeating the Distance 500 Desert Gatekeeper drops the "Emblem of the Dragoon").

**Source:** GDD.md §4 (Gatekeeper Drops); CANON.md §D (D9), §G (CharacterClass, Emblem)

- Launch class set (`CharacterClass` enum): `explorer` (default, always unlocked), `dragoon`, `sage`, `ranger`, `alchemist_knight`, `bard`. Content team may extend.
- A `ClassEmblem` is an account-level unlock item. Which Gatekeeper drops which emblem is a content-table mapping (GDD gives Dragoon ← D500 Gatekeeper as the example).
- Drop crediting: every `Player` who is a combatant in the killing Battle rolls for the emblem drop independently. Drop rate **[TUNABLE]** (not specified in GDD/CANON — see Notes).
- An emblem drops into the player's Backpack like other loot; it must be **extracted** to take effect. On extraction the class is permanently unlocked on the account. If the player dies with an unextracted emblem in the Backpack, it is deleted with the rest of the Backpack.
- Unlocking an already-unlocked class is a no-op (duplicate emblems have no additional effect; they remain tradeable items — see Notes).
- Class unlocks are stored on the `Player` and are NOT wiped at season end.

---

## Training Ground (Build Templates)

A UI subscreen available in Outer Hubs that lets players pre-allocate the large block of combat skill points granted by a high base run level, using saved "Build Templates", instead of clicking through hundreds of level-ups at maze entry.

**Source:** GDD.md §4 (The Training Ground); CANON.md §B (Hubs & run levels), §S (Bevy client), §D (D16)

### Flow

1. In an `active` Outer Hub, the player opens the Training Ground and creates or edits a **Build Template**: a named allocation plan mapping the skill points earned by levels 1…`base_run_level(hub)` onto combat skills/stats.
2. Templates are persistent per-player data (survive logout; HTTP API). A player may save up to 8 templates **[TUNABLE]**; template names follow the text limits in [../edge-cases/limits.md](../edge-cases/limits.md).
3. When the player starts a run from that hub, they select a template (or "manual"). The server sets `run_level = base_run_level(hub)` and applies the template's point allocation instantly, before the party enters the maze.
4. Skill points earned from run levels gained *during* the run are allocated manually as normal.

### Rules

| Rule | Behavior |
|------|----------|
| Availability | Outer Hubs only (the Center Hub grants RL 1 — no block to allocate). |
| Validation | A template is validated against the departure hub's point budget at run start; a template built for a deeper hub than the departure hub fails with 400 `validation_error`. A template built for a shallower hub applies, and the surplus points are left unallocated for manual spending. |
| Class binding | A template is bound to a `CharacterClass`; selecting it with a different class active fails with 400 `validation_error`. |
| Wipe behavior | Templates are persistent meta-state; combat allocations they produce are ephemeral and die with the run. |

---

## Resource Stratification

Material spawn tiers are banded by distance so that the whole world stays economically relevant: high-tier hubs yield rare materials, but the low-tier materials needed for base crafting components only spawn near the Center Hub.

**Source:** GDD.md §4 (Resource Stratification); CANON.md §B (Distance → difficulty: `tier(d) = floor(d / 100)`)

### Rules

- Material tier bands **mirror the biome tiers**: the loot tables of a band spawn materials of that band's tier.
- **Tier-1 materials spawn only at `d < 300`** **[TUNABLE]**. In general, each material tier has a bounded spawn band; materials of tier *t* do not spawn in bands far above their home band, forcing deep-zone crafters to buy low-tier materials from players who farm near the center (via stalls and contracts — see [economy.md](economy.md)).
- Loot rarity weights shift one band per `tier(d)`. Red-chest gear hits its **full** rate at `d = 300` and **ramps in** from `[loot] gear_ramp_start_distance` (40) below it, at `gear_ramp_start_mult` of the rate rising to all of it at 300 — see CANON §B. Nothing drops shallower than the ramp start.
- A felled encounter may also drop a **potion** (`[loot] potion_drop_chance` **[TUNABLE]**), from the consumables whose own tier is at or below `tier(d)`, so the deep trophy line stays behind the depth it was authored for. The Apothecary and Alchemy remain the reliable supply; a drop is a top-up. The Revive and Experience consumables are excluded — they have their own world-drop rates.
- Enforcement is in loot-table generation (server-authoritative loot rolls, CANON §S); there is no client-visible error — out-of-band materials simply never appear in drops.

---

## Meld Skills

Three permanent non-combat skills per player, levels **1–99**: `forging`, `mercantile`, `alchemy` (`MeldSkill.skill_kind`). Combat stats wipe per run; Meld Skills never wipe (not on death, not at season end). They level up **exclusively in Hubs or through extraction success**.

**Source:** GDD.md §4.1; CANON.md §B (Death & durability — Repair; Economy), §G (MeldSkill, Gem)

### XP sources (exactly as GDD §4.1)

| Skill | XP source |
|-------|-----------|
| `forging` | Successfully combining extracted raw materials (crafting). |
| `mercantile` | Successfully completing player contracts, or selling items at stalls. **Shipped source:** selling a material stack to the Broker (`POST /v1/vendors/broker/sell`) — stalls and contracts are unbuilt, so until `EC-1` this is Mercantile's only XP anywhere. |
| `alchemy` | Extracting rare plants and monster parts. Also every potion craft (`POST /v1/crafting/craft` — a potion credits Alchemy, only metalwork credits Forging). |

XP amounts per action and the level curve are content-table values **[TUNABLE]** (not specified in GDD/CANON). No XP is granted for failed crafts, cancelled contracts, or unsold stall listings ("successfully" is load-bearing in GDD §4.1).

### Level effects

| Skill | Effect | Formula / rule |
|-------|--------|----------------|
| `forging` (level L) | Max-durability repair cap on blue-chest gear | Can restore max durability up to `base_max × (0.5 + L/198)` — L99 → 100% of `base_max`. **[TUNABLE]** See [economy.md](economy.md) (Durability Sink). |
| `forging` | Better stat variance on crafted items | Directionally per GDD §4.1; magnitude is content-table **[TUNABLE]**, no CANON formula. Shipped as `[forge] gear_variance` narrowing by `gear_variance_per_level` to `gear_variance_floor`. |
| `forging` (level L) | Gear tier a smith can reach | `floor(L × gear_tier_per_forging_level)`; a **trophy catalyst** adds `catalyst_tier_bonus` on top (levelling raises the floor, monster parts raise the ceiling). Rerolling affixes needs `reroll_min_forging_level`. **[TUNABLE]** |
| `forging` / `alchemy` (level L) | Which recipes are craftable | Each recipe declares `min_level` in its own skill (`meld_proto::consumables::RecipeDef`); below it the craft is refused `403 forbidden`. The **trophy line** (potions made from combat drops) sits above the reagent line on this ladder. |
| `mercantile` (level L) | Broker sale price | `+sale_haggle_pct_per_level` per level over 1, capped at `sale_haggle_max_pct` — a better haggler is quoted better. **[TUNABLE]** |
| `mercantile` (level L) | Hub tax reduction | Tax `= 10% − L × 0.05%`, floor 5%. **[TUNABLE]** Applies to stall sales and contract payouts (paid by seller/poster). |
| `mercantile` (level L) | Stall slot count | `4 + floor(L / 10) × 2`, max 24. **[TUNABLE]** |
| `mercantile` (level L) | Stall placement gates | Stalls in hubs `d ≥ 1000` require Mercantile ≥ 30; `d ≥ 3000` require ≥ 60. **[TUNABLE]** |
| `alchemy` (level L) | Gem crafting | High-level Alchemists craft permanent `Gem` items that socket into blue-chest gear. Each gem recipe carries a minimum Alchemy level (content-table **[TUNABLE]**; no CANON formula). Socket counts per gear item: see [../edge-cases/limits.md](../edge-cases/limits.md). |

### Invariants

- Meld Skill levels only increase; there is no decay or respec.
- Meld Skill XP is never granted inside the Maze except via the extraction events listed above (banking a Backpack).
- All Meld Skill mutations are server-side, via the HTTP API or the server's own run-end processing (CANON §S boundary rule).

---

## Notes

- **Gaps (not resolved by GDD/CANON, flagged for design):** rebuild cost for ruined camps, ClassEmblem drop rates, Meld XP amounts and level curve, gem recipe level gates, template count cap. All marked **[TUNABLE]** above and must live in server config / content tables, not code.
- **Stall slot formula vs. cap:** `4 + floor(L/10) × 2` reaches only **22** at the level cap L99; the stated max of 24 is unreachable. Implement the formula with the 24 clamp as written; flagged as a design inconsistency.
- **Tax floor:** at L99 the tax is `10% − 4.95% = 5.05%`; the 5% floor never binds within levels 1–99. Implement the clamp anyway.
- **Duplicate emblems** are ordinary items after the first unlock — they can be sold at stalls, making Gatekeeper farming a valid economy source (consistent with the chits/item conservation rules in [economy.md](economy.md)).
- **Tier-1 band `d < 300`** mirrors the *biome* band (Forest + Desert end at 300), not the `tier(d) = floor(d/100)` loot-tier band (which would end tier 1 at `d = 200`). Material stratification follows biome bands.
