# Proposal — Progression & unlocks: a new player earns their party

> Status: **proposed** (design of record for `CL-1`, `PT-3` party-slot unlocks, and the
> `P1-3` onboarding work that depends on them). Folds into [`CANON.md`](../CANON.md) §G +
> a D-number when the first slice lands.

Today a brand-new account starts with **four party slots and all five classes**, levels
live only inside one dive, and nothing is ever permanently earned. That makes the first
hour both overwhelming and weightless: everything is available and none of it is a
reward.

This proposal makes the party something a player **assembles over time**, and it names
the two structural changes that have to happen first.

---

## 0. Two blockers this design has to fix first

**Level 255 was impossible on the old curve.** `xp_to_next(L) = xp_base ×
growth_factor^(L-1)` doubled every level: level 255 needed `80 × 2^254` XP. The curve is
now the **design statement itself** — *level L takes `fights_per_level_base` fights
against a same-level encounter, plus one more every `fights_per_level_ramp` levels*. The XP
number is **derived** from the encounter tables (a same-level encounter sits at
`d = 12.5 × L`, since `mlevel(d) = round(d / 12.5)`), so retuning creature XP retunes the
ladder with it instead of letting the two drift apart. Punch above your level and you
climb faster; that falls out of the same maths rather than needing a rule.

The ramp is deliberately gentle through the first act. The gate on the **second party
slot** is a hero at level 10, so everything before it is played solo — and a straight
`L + 1` shape charged **54** at-level fights for it, which is most of a first session
spent in the game's least interesting configuration. At the shipped values it costs
**22**, while level 20 costs 65 and level 30 costs 128 (the old shape wanted 209 and
464). The goal the curve is tuned against: *most players reach doubles before the run
that kills them.*

**What is reachable today, and what waits on hubs.** `departure_hub_distance` is
hard-coded to the Center Hub, so `base_run_level` is always 1: a dive starts at level
1 and climbs only as far as that dive's fighting takes it (~level 5–10 in a good run).
Abilities above roughly level 16 — and every rank past the first — are therefore
**authored ahead of the system that reaches them**. Deeper departure hubs are that
system, and they are future work. The record they will read from already persists
(`class_bests`, the Vanguard board). This is a deliberate ordering, not a tuning bug.

**XP is not persistent.** Levels live inside a dive (`PlayerRun::run_level`), and a deeper
departure hub starts a run at a higher `base_run_level` — the **world** is the progression,
not a character sheet. Level 255 is therefore something a deep run reaches, not something
a grind banks.

**Levels are per hero, within the run.** Each hero climbs its own ladder from the run's
`base_run_level`, so the hero doing the killing is the hero that gets stronger — and a hero
that fell earns nothing from the fight it did not finish. The player's headline `run_level`
follows their best hero, so every one-number message stays true.

**What persists is the record, not the XP:** the **best level ever reached per class**
(`class_bests`), monotonic, so a shallow dive can never lower what was earned deep. That is
the roster's memory and the thing a player can point at.

---

## 1. Party slots are earned

| Slot | Unlocked by |
|------|-------------|
| 1 | from the first login (Explorer only) |
| 2 | any hero reaches **level 10** |
| 3 | **two** heroes reach **level 20** |
| 4 | **three** heroes reach **level 30** |

A locked slot renders as a locked slot in the party builder, not as an absence — a player
should see what is coming.

## 2. Classes are earned, and each one teaches something

Every unlock is **permanent and account-level**. Order matters: each trigger is a thing
the player was going to do anyway, so the unlock reads as recognition rather than a chore.

| Class | Unlocked by | Why that trigger |
|-------|-------------|------------------|
| **Explorer** | start | the martial baseline; the whole first hour is Explorer-only |
| **Resonant** | defeat an **elite** (once slot 2 exists) | the first fight that genuinely wants a healer |
| **Shifter** | **enter a dungeon** for the first time | the class whose senses are *about* dungeons |
| **Phoenix Guard** | survive an **undead boss + minions** encounter | the fight that wants a wall; surviving it proves you needed one |
| **Psyker** | suffer a **full party wipe** with 3 heroes | the consolation prize for the worst night: the class that changes how you fight |

Renames: **Iron Hull → Phoenix Guard** ✅ (mechanically identical; the name earns its
fire-and-return fiction from the unlock that grants it). The wire/balance/asset key is
`phoenix_guard`; `iron_hull` survives as a deserialization alias so heroes already
persisted under the old key keep their class.

## 3. The Shifter becomes the dungeon class

Its current overworld perks (finding *world* items and locations) move to the **Explorer**,
which is the class whose guild premise is exploration. The Shifter instead:

- **knows where dungeon entrances are** on the overworld (the dungeon-finder),
- carries the **dungeon-level abilities** (its kit stays, gaining depth inside a dungeon),
- **senses permanent and ephemeral items** in the overworld *and* inside dungeons.

So the Explorer is *the map* and the Shifter is *the door and what's behind it*.

## 4. Abilities keep arriving all the way to 255

A ladder that stops at level 5 is a ladder that stops mattering at level 5. Two mechanisms
together, so 255 levels of growth doesn't mean 250 new buttons:

- **New abilities** at authored levels, spread across the whole range rather than bunched
  under 10.
- **Ability ranks**: an ability a hero already owns gets stronger at authored levels
  (`Power Strike II`, `III`, …). Ranks are the bulk of the late ladder — a rank is a
  number change on an existing button, which is content a designer can add in a line.

**Every ability says what it does**, in two places: the battle command menu (the row a
player is about to press) and the abilities view in the menu (browsing between fights).
The description lives with the ability definition, so the two can never disagree.

## 5. Every unlock announces itself

One banner, the same style the game already uses for a level-up or a loot report:
what was unlocked, and one line on what it means. Menus get the frosted-glass treatment
and stay dismissable — a banner that traps a player is worse than no banner.

---

## Build order

1. **Foundation** ✅ — the fights-per-level XP curve to 255; per-hero levels inside the
   run; the unlock registry (`meld_proto::unlocks`) + the `unlocks` table; slot/class
   gating enforced server-side (`run.enter_maze` clamps rather than rejects); unlock
   banners.
2. **Abilities** — the ladder to 255 (new abilities + ranks) and the descriptions, in both
   surfaces.
3. **Class roles** — the Explorer/Shifter perk swap and the Shifter's dungeon senses.
4. **The undead boss** ✅ — the encounter that unlocks the Phoenix Guard: the **undead
   rite**, a boss with four undead minions, built on `CR-7`'s pack machinery. Bosses
   carry their own faction (so an undead boss is undead wherever it is met), gain a
   deep-gated ability past level 45, and wear a palette band that darkens with depth.
5. **Presentation** ✅ (menus) — `meld_client::glass` is the one definition of the
   frosted-glass menu surface, imported by every menu; banners dismiss on
   [Space]/[Enter]/[Esc] and time out on their own.

Onboarding (`P1-3`) sits on top of all five and is deliberately **not** part of this
proposal: it can only teach a first hour that exists.

## Decided while building

- **A dead hero earns nothing.** XP goes only to heroes still standing when the fight ends,
  on each hero's own ladder.
- **A level-up raises nobody.** It tops up the living; the fallen come back on a **Waking
  Salt**, which the world sprinkles alongside **Insight Motes** (bankable XP you choose who
  to spend on). Progression stops being a free heal.
- **No persistent XP.** Progression is dive-scoped; depth is the meta-progression. What
  persists is the *record* of a milestone (used by the unlocks), never the XP itself.

## Open decisions

- *(Settled)* The slot rules count heroes **simultaneously** at the level during a dive
  (`heroes_at_level`), and the unlock they grant is permanent. Reaching the bar gets easier
  as gear improves, which is the intended pressure toward the loot chase.
- Should a locked class still be *visible* in the party builder? Yes — a player should see
  the roster they're working toward, greyed with its trigger named.
