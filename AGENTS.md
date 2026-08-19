# AGENTS.md — AI Agent Context for MELDWORLD

> Symlinked as `CLAUDE.md` so Claude Code and other tools read the same file.
> This is the convention-of-record for any AI agent working in this repo. Keep it
> accurate; prune what isn't followed. For deep dives, read the spec docs linked
> in [Deep Dives](#deep-dives) — don't duplicate them here.

## What this is

**MELDWORLD** is an instanced, asynchronous MMO roguelite with turn-based **ATB**
combat, built **all-in-Rust** (server + Bevy client + shared wire types). The core
loop is **extract-or-die**: dive into a procedurally-generated maze from the Center
Hub, march outward through biome areas fighting creatures, and either extract your
loot at a portal (banked to a persistent Vault) or die and lose your backpack.
Difficulty, monster level, and loot scale purely with **distance** from the origin.

This started as an **architecture spike** and has grown into a working vertical
slice on the real stack — the core loop plus Last City, verticality, per-character
gear, and extraction all ship. Trust the **code** for what's live and
[`ROADMAP.md`](docs/ROADMAP.md) for what's next; larger systems (full economy/meta,
Gatekeepers, chunk streaming, seasons) are scoped there.

## Docs live in `docs/`

All design, spec, and planning docs live under [`docs/`](docs/) — start at its
index, [`docs/README.md`](docs/README.md). Only `AGENTS.md`/`CLAUDE.md` and the
repo `README.md` stay at the root. If you cite a doc in a code comment, use its
`docs/…` path.

## The roadmap is the worklist — check items off

[`docs/ROADMAP.md`](docs/ROADMAP.md) is the **live list of what we're building
next**, as checkboxes with stable IDs (`LC-2`, `GR-3`, …). It sits above
`BUILD-PLAN.md` and below the spec. **When you pick up work:**

- Find (or add) its roadmap item; cite the **ID** in your branch name, commits,
  and PR title.
- **Tick its checkbox** (`- [ ]` → `- [x]`) in `docs/ROADMAP.md` in the *same PR*
  that lands it — and update its `behaviors/`/`interfaces/` spec if observable
  behavior changed. A merged item with an unchecked box is a bug. Partial work
  stays unchecked; record progress in the item's sub-bullets.
- This file is a merge hotspot — edit only *your* item's line.

## Spec hierarchy — read this before changing behavior

Behavior is specified top-down; **on conflict, the higher doc wins**:

1. [`GDD.md`](docs/GDD.md) — the game design vision (source of truth for *intent*).
2. [`CANON.md`](docs/CANON.md) — authoritative resolutions of every gap/ambiguity/name in the GDD. **CANON wins over GDD.** Names, enums, formulas, and `[TUNABLE]` constants live here. If you're implementing a rule, find its CANON §/D-number.
3. [`behaviors/`](docs/behaviors/) + [`interfaces/`](docs/interfaces/) — the spec: observable behavior (behaviors) and wire/data contracts (interfaces). Each references its CANON source.
4. [`BUILD-PLAN.md`](docs/BUILD-PLAN.md) — milestones (M0–M…) and task IDs (T1–T6) the code is executed against.

*What to build next* comes from [`docs/ROADMAP.md`](docs/ROADMAP.md) (above); the
four docs here specify *how it must behave*. When you add or change a gameplay
rule, cite its spec (`combat-atb.md`, `CANON §B`, etc.) in the code comment, as the
existing code does.

## Workspace layout

Cargo workspace; the Bevy client is a **separate workspace** under `client/`
(sharing only `meld-proto`) so its heavy wasm/Bevy deps don't burden the server.

```
shared/meld-proto/          wire types: envelope {type,seq,ts,payload}, C2S/S2C messages,
                            HTTP DTOs, enums, validators, golden round-trip tests
balance/balance.toml        EVERY [TUNABLE] constant — no gameplay literal lives in code
server/crates/
  meld-balance/             typed balance.toml loader (Balance struct)
  meld-db/                  Postgres persistence (accounts + bcrypt, Vault, gear, meld-skills)
  meld-api/                 HTTP API (axum): auth, players/me, realtime-ticket mint, vault/crafting
  meld-battle/              server-authoritative ATB engine (100 ms tick) — pure, deterministic, no I/O
  meld-world/               overworld: seeded procedural areas, monster placement, movement, touch
  meld-run/                 run/instance lifecycle + battle assembly (party → Fighters)
  meld-server/              WS gateway + session handshake + the authoritative game loop + HTTP mount
client/crates/meld-client/  Bevy client (native + wasm); screens: Join → Overworld → Battle → Ended
qa/                         headless bot framework + Postgres-backed conformance/integration tests (T6)
```

The authoritative game loop is [`meld-server/src/game.rs`](server/crates/meld-server/src/game.rs):
one Tokio task owns all ephemeral state (sessions + the active `MazeInstance`), is fed
`ServerEvent`s over an mpsc channel, advances the ATB on the 100 ms tick, and fans
authoritative `*.*` messages back per session. **Exactly one task touches the state, so
there are no locks** (CANON §S).

## How to run

A `Makefile` wraps the Postgres + server + client wiring:

```sh
make play         # boot throwaway Postgres + server + the native window (assets PACKED, release)
make play-dev     # same, but a debug build that hot-reloads loose assets from disk (dev loop)
make play-solo    # self-contained native window: server baked in, no Postgres, no setup
make dist         # build the shippable single-file QA binary (server + assets embedded)
make release VERSION=v0.1.0   # tag latest main + push → CI builds win/mac/linux + cuts a Release
make smoke        # headless: drive the whole loop through the real client netcode (exits 0 on victory)
make server       # server only
make test         # the Postgres-backed QA suite
make stop         # stop the local server (Postgres left running, reused across runs)
make help         # list every task
```

### Self-contained QA / demo binary (`make dist` / `make play-solo`)

For handing the game to someone who just wants to *play it* — remote QA, a
demo — there is a single-file native build that needs **no Postgres, no server
process, no Rust toolchain, and no files beside it**. `make dist` produces one
executable (`dist/meldworld-<os>-<arch>`); the tester runs it and the game window
opens. `make play-solo` builds and runs it in place for a quick local try.

It's the `meld-client` binary built with the `embedded-server` feature (native
only): `main()` boots the whole authoritative server on a background thread with
an **in-memory** DB ([`meld-db`](server/crates/meld-db/src/lib.rs) `Backend::Mem`,
selected by a `memory://` URL) and the **embedded** balance
([`meld-balance`](server/crates/meld-balance/src/lib.rs) `EMBEDDED_DEFAULT`), on
an ephemeral localhost port; [`bevy_embedded_assets`](client/crates/meld-client/src/main.rs)
bakes all 84 MB of assets into the file. Everything is **ephemeral** — accounts,
Vault, progression live in RAM and reset on exit (a clean slate every launch),
which is what you want for QA. The party/flag env vars (`MELD_PARTY`,
`MELD_CLASS`, `MELD_AUTOPLAY`) still apply. This does **not** touch the normal
server/Postgres path — default builds and the wasm client are unchanged.

`make dist` builds for the host OS/arch only. For **cross-platform** binaries
(Windows `.exe`, macOS, Linux) there's a `dist` GitHub Actions workflow
([`.github/workflows/dist.yml`](.github/workflows/dist.yml)) that runs the same
`embedded-server` release build on each native runner — no flaky cross-compiling.
It runs on every merge to `main` (binaries are downloadable run artifacts — the
"latest main" build), on demand from the Actions tab ("dist" → "Run workflow"),
and on a `v*` tag (which also attaches the per-OS binaries to a GitHub Release).
`make release VERSION=v0.1.0` is the one-liner for the tag path: it tags the
latest `origin/main` and pushes, and CI does the rest.

`make play` builds the wasm client and has the **server itself serve it**, so the
whole game lives at one URL (`$MELD_ADDR`, default `http://127.0.0.1:18090`) — no
proxy, no second port. It needs `trunk` (`cargo install trunk`) and the wasm target
(`rustup target add wasm32-unknown-unknown`); everything needs a local Postgres
(`initdb`/`pg_ctl`/`createdb` on PATH).

**[E] is the one interact key** on the overworld — it does whatever is in reach
(gather a node, open a chest, descend an entrance, extract at the deep portal, join a
nearby fight) and stops a channel if one is running. Priority is urgency then
proximity: a teammate's fight outranks scenery because it closes. The HUD shows a
prompt *only* when something is in reach, plus a **progress bar** that fills once per
channel payout (per unit while gathering, once while extracting — `fill_ms` on
`run.channel_started`). Touch gets the same thing as one contextual **Interact** button
that hides when nothing is in reach.

**A HARVEST BANKS ONE UNIT PER TICK AS ITS OWN STACK.** `advance_harvests` pushes rather
than merging, so ore you just dug up is six stacks of one and never one stack of six.
Anything that spends materials must therefore sum **across stacks** (`spend_material`) —
looking for a single stack holding the whole cost is what made a 6-ore structure and a
3-ore field forge both unbuildable from freshly-gathered ore, while refusing a player who
was carrying exactly enough. One KIND though, never a mix: a structure records what it was
built from so packing it down hands back the same stock.

**Field stations (MS-1).** Raising one **takes time**: it is a channel like harvesting
(`[forge] station_setup_ms`), the stock is spent up front, and stepping away loses the
work — so where and when you build is a real decision. Packing one up is its own channel
and hands back part of the stock, and only its owner may do it. A crafter who carries the
stock can raise a bench in the maze from the menu's **Map** column — a smith's **forge** (ore, gated on Forging) or a Keeper's
**alembic** (reagents, gated on Alchemy). It then stands in the world for everyone
(`station:<kind>:<jobs>`), and **anyone** standing at it can ask for work with `[E]`: the
**station owner's** skill is what the job is done at and takes the XP, while the piece and
the stock are always the requester's — ownership never moves. Working metal is a **heat**:
a marker sweeps a red bar, each blow has one yellow band, and the blows that land decide
what the work is worth (the affix pool a re-draw rolls, the durability a repair gives back,
the size of a temporary edge, the doses a brew yields). Deeper work is harder; the
crafter's own level and every other **Smithwright** (at a forge) or **Keeper** (at a
still) in the party make it easier again. The bench's temporary boon is its own prompt and
its own button — **[N]** asks for a smith's **edge** on a worn piece or a Keeper's
**tonic** for the whole party, both lasting the dive and no longer. A set-up alembic also
radiates a **regen field** over anyone standing near it.

**An AFFLICTION does not wear off; a BOON does.** Poison, a web, a mark, a dread — these hold
until something CURES them, because outlasting a debuff by standing still is not a decision.
Haste, Barrier, Regen and Evasion still expire and decay, or the opening turns of a fight would
be the whole fight. The classification is `meld_proto::statuses` (`AFFLICTIONS` / `BOONS`), read
by both sides, and an unknown condition is treated as a BOON on purpose: a new boon mistaken for
an affliction becomes permanent and breaks every fight, while the reverse merely keeps the old
timer.

**A cure answers a CONDITION, not a checklist.** `statuses::Family` — Venom (poison, burn),
Bindings (slowed/web/chill/bind), Senses (marked/distracted/blinded), Mind (dread/frenzied) —
and each mender row lifts ONE: the Keeper's **Poultice** (5) draws venom, the Resonant's
**Sanctuary** (35) calms a mind. Only a **Panacea** answers all four, and it is priced like it;
a cheap bottle that cured everything would make every affliction a non-event. Every affliction
must belong to a family (`every_affliction_belongs_to_a_family`) or nothing can lift it.

**A revive has to be REACHABLE.** The only one in the game was the Resonant's level-255
capstone, so a party that lost a hero at level 20 had nothing but a rare Waking Salt drop.
The Resonant's **Revitalize** (50) raises one fallen ally and the Keeper's **Terra's Gift** (50)
raises them all — both through `raise_fallen`, which resets the gauge so you come back at the
END of the queue rather than with a free turn. A party row that can raise is the only kind whose
targets include the dead; every other one skips them, because healing a corpse does nothing.

**A blow brings you round; care takes the wheel back.** A physical hit clears `dread` and
`confused` from whoever takes it — creature or hero — and any healing ends `frenzied`. Both are
how a party answers a condition without carrying the right bottle, and both cut in the enemy's
favour too. `dread` forbids acting on ENEMIES and nothing else (defend, drink, mend yourself or
an ally, even swing at one, all still yours); `confused` swings at the wrong person; `frenzied`
takes the choice away and attacks on its own; `paralyzed` skips the turn, and **a wholly
paralysed party is a DEFEAT** — otherwise nobody can act while the creatures work through them,
which is the unbounded soft-lock a gauge CAP used to cause.

**BLINDNESS IS ENFORCED SERVER-SIDE, because people will hack the client.** A blinded party is
not SENT the creatures — `snapshot_msgs` drops them into the same `hidden` set that hides
another player's bounty mark. The client's blackout is presentation on top of that, and a
patched client that skips the mask still learns nothing. `check_touch` runs off server positions
regardless, so **you walk into what you cannot see and the fight starts anyway**. Any future
"you cannot see X" condition belongs in that cull, never in the renderer alone.

⚠️ **Afflictions were BATTLE-scoped and now are not.** `timed_statuses` still lives on the
`Fighter`, which is rebuilt per fight — so the RUN remembers instead (`hero_afflictions`, beside
`hero_hp`), harvested when a battle ends and re-applied when the next starts. That is what makes
"until cured" mean something on the road, where venom bites per step and bindings drag a march.

**A condition repaints the readout.** Statuses are not just icons: a hero's cell and a
creature's HP bar take the condition's colour, from a palette named after things rather than
built from primaries — poison purple, marked mustard, slow blue sage, rage red for
**afflictions**; barrier steel blue, regen rosemary green for **boons**. Warm-to-sour means
something is being done TO you, cool herb/metal means something is helping, so a glance sorts
the party before you read a word. An affliction outranks a boon, and being hit or being the
active hero outranks both. `condition_tint` owns it; anything the engine slows (`web`/`chill`/
`bind`) also wears a **snail**, because a crawling gauge with no icon was indistinguishable
from a slow one. The **fighter itself** wears the colour too, as a rim around its own sprite
(`update_condition_rims`) — the tint on the party strip alone went unnoticed in play, because
in a fight the eye is on the arena.

**REACH AND SWEEP ARE TWO AXES, NOT ONE "RANGED" FAMILY.** Reach is *can it get there*
(`ItemFamily::reaches_past_the_front` — bow, sling, thrown spear), sweep is *how many does
it hit*. Bundling them would make every ranged weapon a crowd-clearer and every
crowd-clearer ranged. Reach is the MARTIAL answer to a back rank, which until now was a
caster's problem and a swordsman's wall — and it is the ATTACKER's property, resolved inside
the one reach rule, so one archer never exposes the rear to the rest of the party.
`Spear` deliberately does NOT reach despite being the two-handed reach weapon: granting it
would silently buff every Explorer holding one, which is a balance change wearing a
refactor's clothes. **All-enemy is not a weapon property** — a basic attack that hits
everything scales with pack size, beats a single-target weapon at about four enemies, and
makes the back rank, the group tier and the entire level-20-and-up AoE ability ladder
pointless. It belongs on limited thrown consumables.

⚠️ **`GearBonus` is defined TWICE** — `meld_db::GearBonus` and `meld_run::GearBonus`, bridged
by `effective_gear_bonus`. A property added to one and not the other is a bow that reaches in
the Vault and not in the fight.

**AN ENCOUNTER IS PACKS OF GROUPS, AND A RANK IS RELATIVE.** A **pack** is how the fight got
assembled (what `group_around` pulled in) and is only provenance; a **group** — enemies of
the same type and their minions — is the addressable unit, and it is derived at BATTLE
ASSEMBLY rather than carried through the world. That is deliberate: a group is a property of
the encounter (the same creature belongs to a different one depending on who it ended up
standing with), and there are sixteen places a spawn is created against exactly one place a
battle is built.

**Reach is resolved in ONE function** (`softened_by_rank`), inside the one funnel every hit
already passes through (`apply_damage_reaching`). Rank, damage type and flank state decide
it together. A second copy of that rule at any other call site is the exact drift that has
bitten this repo twice — the wall-collision line that went into one mover and not the other,
and the two copies of "what blocks".

**FLANKING is the co-op answer to "a rank is relative".** One group's back row is another
party's front, and once **two distinct parties** have laid into the same group its rear stops
being covered — for everyone, for the rest of the fight (`group_strikers`). It is derived
from ENGAGEMENT rather than from coordinates, so the fight stays an ATB fight instead of
becoming a tactics grid, and it is EARNED: two heroes of one party never flank however hard
they swing, or a solo player would flank everything by attacking twice. It rides the wire as
`flanked`, because a rank that stopped protecting has to say so.

**CREATURES STAND IN RANKS TOO.** A pack of three or more forms two: the leader and the
front half hold the line, the rest stand behind (`MonsterSpawn::back_row`, carried into the
fight as `Fighter::back_row`). It is the SAME trade a hero's back row makes, in the same
engine code — half the physical damage taken, half the physical damage dealt — so a pack's
rear shrugs off a sword and is answered by a spell, an elemental brand or reach, with no new
combat code at all. A creature fighting **alone is always in front**, or a lone spawn would
be half-immune to every sword in the game for free. A formation is also what makes a big
pack legible: five abreast already spans the battle screen, so the client draws the rear as
its own inset rank rather than one long line.

**The back row is a TRADE.** Standing in the back rank halves the **physical** damage you
take and halves the **physical** damage you deal (`back_row_damage_mult` /
`back_row_attack_mult`). It used to only do the first, so the optimal formation was the
whole party standing back for a flat 2x effective HP — nothing reached past the row and
nothing was given up for it. A spell, a Focus or an elemental breath reaches the back rank
at **full force** (`DamageType::is_physical`), so a caster gives up nothing there — which
is why the back row is a caster's home and the front line is a martial's — and a weapon
carrying an elemental `brand` keeps full damage from the back. Every physical swing reads
its power through `phys_atk`; a call site that touches `.atk` directly is a swing nobody
charged for. **A class missing from `hero_attack_type` deals TRUE damage** (`None` bypasses
the modifier map entirely) — the Hunter, Smithwright and Keeper all were, for a long time.

**What you WEAR decides what hurts you, and it is the same rule for a creature's hide.**
Armour has a weight (`ArmorWeight`: heavy / medium / light / robe) and each weight answers
the three physical types differently — plate turns an edge and fears a hammer, mail defeats a
cut and lets a spike through, padded leather soaks impact and opens to a blade, and a robe is
not armour at all (worst against every physical type, best against fire, ice, lightning and
mind). The SHAPE lives in `meld_proto::equipment::weight_profile` as signed steps so a client
can render it; the size of a step is `[armor_resist] step`, because a coefficient is balance's.
Steps are **per piece** and fold through `fold_damage_modifiers` (1 + Σ(m−1), clamped 0..2),
so a full plate set is an identity and one piece is a nudge — and a class allowed two weights
has a real mixed-loadout decision. Keep a full set well inside the clamp: at 0.0 or 2.0 a
resistance stops being a trade and becomes a switch.

Creatures answer the same question through `abilities::Body` (Rigid / Plated / Hide / Soft /
Amorphous), in ABSOLUTE multipliers rather than steps, because a creature is one body and
nothing stacks. **The physical half of a creature's profile is no longer authored per kind**
— it was, and it was wrong in the same direction nearly everywhere: every colossus, golem and
ice maw RESISTED blunt, because the table had been written as "a tough thing resists physical"
instead of by material. A hammer is what breaks stone. A per-kind *elemental* entry still
overrides the body; a per-kind *physical* one is rejected by
`the_per_kind_table_no_longer_authors_physical_types`.

Every weight and every body must resist something AND fear something — that is the rule, not a
property of the current numbers (`every_armor_weight_is_a_trade`, `a_body_is_a_trade`). A robe
must also be worse against every physical type than real armour is against its own weakest,
or cloth is just a cuirass with better rolls, which is what the first draft accidentally was.

**Resistance is the defensive stat that actually works**, which is why it is the answer to the
gear-gate problem below: ability damage skips `def` entirely but always passes
`modifier_for`. Measured on the end fight, though, plate is the WRONG armour there — those
bosses lead with fire and hammers, and plate is weak to blunt, so a dressed party gains far
less than its armour value suggests. That is the system working: it made "what do I wear
against this" a question with an answer.

**`def` answers a blade; `ward` answers a spell.** Two defensive stats, each subtracted from
its own half of the damage types: `def` (from **Wll**) from physical, `ward` (from **Mnd**)
from everything elemental or psychic, `DamageType::None` from neither. Both are bounded by
`damage_floor_fraction`, so stacking either is a discount and never an immunity — immunity is
what `damage_modifiers` is for. This is why **Mnd has a defensive half**, the way Dex buys
dodge as well as speed: a caster is no longer simply squishy.

The mitigation lives in ONE place, `apply_ability_damage`. Two paths deliberately do not use
it: a basic attack subtracts `def` itself (along with the defend reduction and its crit), and
a burn/poison DoT is a fraction of the victim's OWN max HP, already scaled to the target.
Everything else — every hero skill, every creature ability — goes through it. **Before this,
non-physical ability damage was subtracted by nothing at all**, because `def` was only ever
consulted for a basic attack; a boss that fought with fire and breath ignored every point of
armour a party had earned. Creatures get a `ward` too, at `[armor_resist]
creature_ward_fraction` of their `def` — under 1.0 on purpose, so casting stays the answer to
something armoured rather than being deleted by giving every creature a magic shield.

**Gear answers elements through two affixes**: "of the Aegis" (flat `ward`) and "of the
Furnace" (extra damage DEALT of one element) — the offensive twin of "of Warding"'s resist.
`brand` decides what your attacks ARE and Furnace decides what that is worth, so the two are a
build: brand to ice and stack ice power to answer something that shrugs off fire. Both roll
straight off the `AFFIXES` pool, and `every_affix_can_actually_roll_on_something` holds every
affix in the registry to being reachable loot rather than paper.

⚠️ **`ward` from Mnd alone makes LEVEL the elemental defence, which is why the Aegis affix
matters.** Measured on
the end fight, adding ward took the geared party from 33 hero-turns to 43 (+30% survival) —
real, and it applies equally to a party wearing nothing, because Mnd does not come from loot.
Gear now gates *physical* ability damage through `def`; for gear to gate the elemental half it
needs a **ward affix**, which does not exist yet. That is the remaining piece of `EW-0`'s
gear-check story.

 A creature ability's damage is
`stat x coefficient` through `apply_typed_damage`, which applies the target's
`damage_modifiers` and `min_damage` and then stops — `atk - def` and
`damage_floor_fraction` live in `physical_hit`, which only the basic `Attack` action reaches.
So armour is close to irrelevant against anything that fights with abilities, which is every
champion and every boss. Measured on the end fight: an **ungeared** level-100 party lasted
**26 hero-turns to a geared party's 25**. Any "gear check" built on `def` is therefore
fiction, and any model that computes survivability as `hp / (atk - def)` is describing a
minority of the damage the player actually takes. Nothing documents this as intentional, and
the Psyker's Foci are called out *specifically* for ignoring armour — a pointless
distinction if every ability already did (`EW-0` tracks the three ways out).

**A potion heals a fraction of the DRINKER's max HP, so who drinks it is a real decision.**
`item_heal_fraction` is 0.4, which is 417 HP on a level-100 Phoenix Guard (1042 max) and 113
on a Psyker (282) — the same bottle, 3.7x apart. A hero may pour one into somebody else (the
engine takes `ally_target`, and the client's Item row opens the ally picker), and the pouch it
comes from is the ACTING hero's, because the shared bag is out of reach in a fight. So triage
means "where does the most HP come back", not "who looks worst": healing the
proportionally-worst hero measurably turned a won end fight into a loss at 67%. Starting
potions are dealt into pouches round-robin, so the kit is ~1130 HP of healing on a 2648 HP
party — **any combat measurement that does not drink understates the party by ~42%**.

**Tune against a PLAYED fight, not a spreadsheet** — `mcp/` exists for this and a run costs
about a minute. The end fight shipped impossible three times from arithmetic, and the last
time the arithmetic was *correct*: its test asserted "clears in 15-35 rounds" and "survives
3-8 hits" side by side and never divided them. A balance test that checks two magnitudes
independently has not checked the fight; assert the RATIO.

**A creature fights to a PROFILE** (`meld_proto::TargetProfile`, CR-9): Weakest, Random,
Backline, Role, GangUp. **At most ONE creature per encounter may be `Role`**
(`cap_role_hunters`): profiles roll independently, so a pack of five could all reach "kill
the healer first", which is not five decisions but one applied five times. Measured — a
capped pack puts **20%** of its damage on the healer where an uncapped one put **100%**. Set from the kind's own nature, then the encounter class (a
champion is smarter than its escort), then a level roll — deeper creatures are smarter on
average — seeded off the creature's own id so it never flickers and never perturbs another
roll. `choose_target` is the ONE place a creature picks; it was two near-identical copies,
so an ambusher hunted the back rank with its claws and the weakest hero with its breath. A
**gang-up mark is shouted** when set or moved, because a pack converging on your healer
with no explanation reads as the game cheating.

**A number belongs to the combatant it landed on — anchor to the ACTOR, never to a role.**
`render_hit_fx` used to place a floating number by identity: `monster_combatant` (which is
only ever `enemies.first()`) drew top-centre and everything else fell through a
`your_ids.position(…).unwrap_or(0)`, so every enemy past the first — and every joined ally —
printed its damage over **hero slot 0's cell**. Packs are standard and the deep rungs added a
wave of all-enemy abilities, so one Purging Light sprayed four numbers onto the first hero.
Every number is now projected over its own arena actor, the way `render_enemy_panel` hangs the
HP bars; the hero-cell path is a fallback and only for a hero we actually field. Numbers that
land together **stack** rather than overstriking, since a sweep resolves them onto one pixel.
The battle's timings and magnitudes live in one runtime-tunable place
([`feel.rs`](client/crates/meld-client/src/feel.rs) `BattleFeel`, `MELD_FEEL=` / `?feel=`) —
they are a *look*, so they stay client-side; authoritative pacing (`tick_ms`,
`gauge_fill_divisor`, `turn_timeout_ms`) is a *rule* and stays in `balance.toml`.

**Everything you can act on is over your head, and the thing in reach glows.** The interact
prompt, the boon prompt, the channel bar and each tick's payout ("+1 bog myrrh") live on a
mostly-see-through plate above the player, because that is where you are looking while you
work — and every prompt on it is its own tappable chip, so touch has a target per action.
There is no corner Interact/Boon button and no corner channel bar any more; the corner keeps
only Menu. Whatever `[E]` would act on wears a slow, infrequent **rim glow** (a copy of its
own sprite, slightly larger, drawn behind it) so "in reach" is visible without a HUD line —
and it **throws light** on the same breath, so the ground near it brightens and the affordance
survives being half behind a tree. Note that emissive on a TEXTURED billboard paints the whole
quad, which is what made the old whole-sprite pulse erase the art — the rim is an unlit
alpha-blended copy instead. Which animation FRAME lights up is set by `animate_chars` beside
the base texture, never mirrored from another system: `illuminate_players` lives in a different
system tuple with no ordering against it, so reading the texture from there was a frame stale
on whichever frames the scheduler ran it first, and the hero juddered in the dark.

**Every item wears its own icon, and never instead of its name.** One rule, in
[`icons.rs`](client/crates/meld-client/src/icons.rs): if we drew art for it, show the art —
every harvestable has a `resource_<kind>.png`, and a shrunk copy of the bush you pulled it
off beats any symbol. If we did not, show a **Nerd Font glyph for its TYPE** (sword, shield,
flask, gold bars for refined stock, a bone for a trophy), coloured by type so the glyph
carries two facts. The count and the name always stay on the row; an icon narrows the guess,
it is not the answer. Used by the counters, the Vault list, the extraction tally and the
over-head harvest pop, which is why it lives in one place. **Name the glyph, never the
codepoint**: this font's Material Design block is shifted from the upstream table, so a
hand-copied codepoint lands on a neighbour — `md-tshirt_crew` drew a *keyboard*, and a test
that only asked "is this glyph in the font?" passed on it. `nf::ALL` is checked against the
face's own `glyph_name`, so being wrong now requires naming the wrong thing.

**Town has a nav, not just a plaza.** Every district is a chip in a frosted travel column
(1/6 width, same as the menu's nav): click it or press its number to go there, and the one
you are standing in reads as selected so the column doubles as "where am I". Walking still
works — travel just lands you inside the district's radius so `[E]` behaves identically.
`TRAVEL_KEYS` is held against `CITY_DISTRICTS` by a test, because the column advertises its
keys and a district past the end of that list would silently have none (it already did).

**A district's name is fiction; the player still has to be told what the room is for.**
"The Drill Yard" and "The Vault-Deep" are canon (§G) and stay, but nothing on screen said
that one is where you pick your party and the other is where your gear lives — so town
read as seven pretty doors. Every `District` now carries a plain-language **`purpose`**
alongside its label, and the two always travel together: the nav chip shows the name with
the purpose under it, the walk-up prompt is `name - purpose  [E] <verb>`
(`district_prompt`), and every counter carries a `subtitle` under its title so a room
explains itself the moment it opens. Tests hold both halves — a purpose is required, must
be short enough for the 1/6 column, and must read as a phrase rather than a title —
because a new district shipping with no purpose is a new district that reads as scenery.

**The three-column convention.** Every cascade screen is **nav | main | detail** at fixed
fractions of the window — **1/6, 1/2, 1/3**, which tile it exactly (asserted at compile time
in [`glass.rs`](client/crates/meld-client/src/glass.rs)). Fractions, not content-sizing,
and every column's SLOT is spawned even when empty: the menu's row used to have no width at
all, so opening a third column re-centred the whole thing and clicking a nav item moved the
nav item you just clicked. Nothing shrinks; only `main` grows, so it absorbs whatever the
minimum widths leave over. Build new panels out of `glass::columns()` + `glass::column()`
rather than hand-rolled widths, and use `glass::row_chip` for list rows (full width,
left-aligned) and `glass::chip` for tabs. **The town counters are on it too** — the Apothecary,
the Broker, the Forge & Alembic and the Vanguard Wall all build a `CounterView` (title, nav,
rows, detail) that `render_counter_panel` draws centred, rather than composing one long string
into the city's bottom strip, where they read as scenery running off both edges. Rows as data
is what lets each be its own tappable chip; the strip keeps the walking-around prompt and the
anvil's heat bar, which wants to hold still. The travel column stands down while a counter is
open, since both want the same left sixth.

**The board tells you what to go and do, and pays when you come home.** The Bounty
Board district used to be the one thing in town that said "come back later"; it is now
eight posted **hunts** (`AD-4`) — fell N of a kind, fell N elites or a Gatekeeper, reach
a depth, extract from a depth, clear a dungeon. A hunt is ONE registry
([`meld_proto::hunts`](shared/meld-proto/src/hunts.rs)) that both sides read: the server
credits progress through `HuntGoal::credits` and the board builds its rows from the same
defs, so the board cannot advertise a condition the server does not check. Every credit
comes off server-owned state — the carcass's own `monster_kind`, the validated avatar,
the run's own record — and is announced as it happens (`run.hunt_progress`), because a
goal you cannot watch fill is a goal you forget you have. **Progress survives death**
(what a dive costs you is your Backpack, not your standing), but the **reward is taken at
the board**, once per account, which is what makes finishing one a reason to come home.
A hunt names a **quarry**; only its goal carries a count, and `objective` formats the
sentence — a number written twice is a number that will disagree with itself. Reward
magnitudes are `[hunt]` `[TUNABLE]`s resolved server-side and ridden onto the wire, and the
**deep hunts pay a rolled piece** as well — through the same `rolled_gear` path the Forge
uses, never the epic pool, because a champion must stay the better *source* of a great item.

**A goal you cannot find is not a goal.** Every hunt row carries a `where_to_look` line
derived from the tables the world generates from (`biomes_of_creature` + `[biome_gate]`,
`gatekeeper_min_distance`), never written down twice — "Fell 6 Dune Wyrms" would otherwise
send a level-1 player hunting a desert the world holds until d400. A **Gatekeeper is
already guaranteed**: one stands in the pass at every biome border, on the clear path,
every run — that was true long before anything said it out loud. In the field the quarry of
an unfinished hunt is **force-included in that player's own snapshot** and tagged
`mob:<kind>:<faction>:quarry` (the portal/node-sense pattern — never a wider shared cull,
which would show everyone everything), and the client floats a QUARRY plate over it. A
**Hunter** senses it from much further out. Screenshot flag: `MELD_HUNTS` / `?hunts`.

**The Den posts contracts with your name on them.** A hunt is a checklist everyone shares;
a **bounty** ([`meld_proto::bounties`](shared/meld-proto/src/bounties.rs)) is generated for
ONE player against a **hunter rank** — the `hunting` Meld skill, raised only by finished
board work, so the ladder asks "how many marks have you put down" rather than "what level
is your party". Every bounty ends in a **boss fight**: an FS-4 named boss wearing a rolled
epithet ("Ironmaw the Unburied"), promoted by the contract's own `power` rather than the
Gatekeeper constants, sighted at a depth the rank earned, in the open or at the bottom of a
descent. It stands in the world for its owner ALONE — `MonsterSpawn.owner` keeps it out of
every other player's snapshot *and* out of their `check_touch`, so co-op can fight beside
you but cannot trigger your contract. Contracts expire and re-roll lazily on read
(`[bounty] active_slots` / `window_hours`); only a *standing* one expires, because a felled
mark is owed its reward however long the walk home takes. The menu's **Quests** column is
gated on owning `class_hunter` — the menu never advertises what you have not earned — and
is reading-only, because the reward is taken at the board.

**A dive departs from a hub you have STOOD on** (PG-2, `meld_proto::hubs`). A hub at
distance D starts every hero at `base_run_level(D)` = `1 + 0.078 × D`, so the deepest
(d3250) starts them at the 255 cap — which is why that distance is the structural end of the
game: past it a hub buys nothing while creatures keep scaling. The gate is the account's own
all-time deepest distance from the `vanguard` table (written off *validated movement*, read
across ALL seasons — a season rollover must not revoke ground you stood on). It is a
**lookup, not an entity**: the run reads one integer, so when `BD-5`'s player-built forward
towns land they add a row rather than replacing a system — a hub deliberately has no
placement or ownership of its own, because that is the `Structure` primitive. A requested
hub is **clamped, never rejected**, exactly as `party` is clamped to owned classes.

**There is no hotkey for going home.** A Town Portal is an *item*, so spending one is an
explicit choice on the menu's **Map** column ("Return to town", enabled only while you
hold one) — the primary way out of a dive belongs somewhere a player can find, not on a
key they have to be told about. The deep portal stays an `[E]` world interaction, and
walking west into the city wedge is still an instant free return (no channel, no item).

Build your **party of four** on the Join screen (keys 1–4 cycle each slot's class),
or preset it: `?party=explorer,psyker,resonant,explorer` / `?class=psyker` (lead) in the
browser, or `MELD_PARTY=…` / `MELD_CLASS=…` natively. `?autoplay` self-drives the
loop for demos/screenshots.

## Testing

```sh
cargo test --workspace                    # unit tests — no DB, no cloud, fully deterministic
bash qa/scripts/local_pg.sh cargo test -p meld-qa   # DB-backed conformance suite (boots throwaway PG)
cargo clippy --workspace --all-targets    # keep clean
```

The engine (`meld-battle`) and world (`meld-world`) are pure state machines with no
wall-clock/RNG-globals/I/O, so they are exhaustively unit-tested. The `qa/` suite
drives **real headless bot clients over the real wire protocol** — no shortcuts, no
client-side combat math: `four_players_kill_monster`, `extraction`, `death_durability`,
`progression`, `raid_merge`, `auth_conformance`.

### Play it yourself: the MCP server (`mcp/`)

A `qa/` test measures a question you already knew to ask. **`meld-mcp` lets you pick up the
controller** — it boots the whole server in-process on an in-memory DB (no Postgres, no
port to collide with) and exposes the game as tools: `new_game`, `look`, `walk`, `battle`,
`abilities`, `act`, `auto_battle`, `interact`, `say`, `chat`, `wait`. Every tool is a
**player input over the real wire protocol**; nothing reaches into `MazeInstance`, because a
harness that reads the engine measures the model, and the model is what has been wrong every
previous time.

```sh
cargo build -p meld-mcp     # `.mcp.json` points at target/debug/meld-mcp
```

**A harness that cannot see the bag measures nothing about the bag.** `run.backpack_update`
rides the wire as `{changes: [{item, delta, cause}], chits_delta}`, and the harness read it
as `{added, removed}` with a `count` — both lookups missing silently, so **every `look` in
this harness reported an empty backpack no matter what you had gathered, spent or looted**,
for as long as it has existed. Any measurement taken through it that involved materials,
chits or loot was blind. The same applies to VERBS: building shipped as three player
intents (`BD-2`) with no harness verb, so the loop could not be driven at all from the tool
that exists to drive it. A player verb without a harness verb is a verb nobody measures.

`new_game` takes the DEV/QA overrides as arguments rather than env vars: `party`, `seed`,
`tutorial`, `start_level`, `gear_tier`, `end_fight_at`, `biome`. `MELD_START_LEVEL` sets a DISTANCE and lets the level follow from it (`base_run_level` and creature level both read 40 at d500), because level and depth are the same fact and a party holding one without the other cannot exist. It streams the frontier out and spawns you on the ring; extraction still assumes d0, which is why it is a TEST flag and not a departure hub — everything past
roughly level 16 is authored for a party that walked an hour to reach it, and PG-2's hubs
are deliberately inert, so without it the only level deep content could be observed at was
1, the one level it was never tuned for.

**The clock does not stop while you think.** A fighter awaiting input stops filling its own
gauge, but everything else keeps ticking, so wall-clock between two tool calls is play-time:
33 seconds spent composing the next call is a whole boss fight, resolved by the 15-second
auto-act. Anything being *measured* must therefore happen inside ONE tool call
(`auto_battle`), or with the calls queued back-to-back. This is also why `turn_timeout_ms`
is not a safe knob to widen for an agent: a longer window is strictly more enemy turns.

**A policy that plays badly measures its own incompetence.** `auto_battle`'s stands on three
rules learned by watching it fail: a spent once-a-fight call (`spent:<key>`) must not be
re-pressed, a Hunter must ATTACK to bank the Adrenaline its skills spend (opening with its
capstone contributed literally nothing for six straight turns), and a hurt party wants its
healer to heal. Fixing those took a geared level-100 party from 4.4% to 11.5% of the end
fight — a bigger swing than most tuning changes, so read any number from a weaker policy as
a floor, not a result.

**A Psyker's `skill_kind` is an OP, not an ability key** — `cast:<kind>` / `reinforce:<kind>`
/ `revoke:<kind>`, and `resolve_psyker` silently falls through to `hold` on anything else.
Sending the bare key spends the turn doing nothing, successfully, which is exactly what a
Psyker party looked like from outside for two rounds of measurement.

### Visual verification (screenshots, not interactive driving)

For anything the browser renders (HD-2D art, HUD/UI, overworld, battle screen),
**verify by screenshot** — boot the stack, load the page, and capture the frame;
don't click through the app interactively (Bevy paints to a `<canvas>`, so the
accessibility/DOM tools see nothing useful anyway). Boot the backend and web client
as two processes, then screenshot:

```sh
# 1) Postgres + game server on :18090 (stays up; Ctrl-C to stop)
client/scripts/serve.sh bash -c 'tail -f /dev/null' &
# 2) wasm client dev server on :9080 (proxies /v1 + /v1/realtime → :18090)
client/scripts/trunk-build.sh          # first build compiles wasm — a few minutes
client/scripts/trunk-serve.sh --port 9080 --address 127.0.0.1 --no-autoreload &
# → open http://127.0.0.1:9080 and screenshot the canvas
```

`?tally` (`MELD_TALLY`) holds an extraction haul on screen — the real one rolls off on a
timer, which is long enough to be gone before a capture lands.

`?party=…` / `?class=…` preset the party, `?autoplay` self-drives the loop, and `?city`
(+ `?wall` for the Vanguard Wall, `?shop` for the counter, `?forge` for the Forge &
Alembic) parks in Last City — handy for deterministic
screenshot states. The `meld-web` entry in `.claude/launch.json`
runs the trunk step for the browser-preview tooling. Pre-build the wasm once
(`trunk-build.sh`) so the preview server starts fast instead of timing out on the
cold Bevy compile.

#### Biome/scenario harness (native embedded build)

Two server-side env overrides (read only at the server boundary; `meld-world` stays
pure) let you load a SPECIFIC world on demand instead of random-walking into it:

- **`MELD_BIOME=<forest|desert|ashfall|tundra|mire>`** pins *every* section to that
  biome (and forces the tutorial off), so you can inspect one biome's maze directly.
- **`MELD_SEED=<u64>`** fixes the world layout for reproducible screenshots/repros.
- **`MELD_DUNGEON=<name>`** forces which authored dungeon a descent loads (any
  `[[floor]]` def in the content pool, e.g. `guardia_forest`), so you can screenshot
  a *specific* dungeon instead of whichever the entrance rolled.

- **`MELD_END_FIGHT=1`** (+ optional `MELD_END_FIGHT_AT=<distance>`, default 30) drops
  **the end fight** — EW's three named bosses — next to the hub instead of an hour's walk
  out at d3200. It only moves WHERE the encounter is placed: the bosses carry authored
  absolute stats (`set_piece`), so the fight you meet at d30 is numerically the one you
  would meet at d3200. Keep `MELD_END_FIGHT_AT` above `[ai] hub_safe_radius` (13) or no
  creature spawns there to promote and nothing is placed at all.
- **`MELD_GEAR_TIER=<n>`** dresses every hero as though wearing a full six-slot set of
  tier-`n` insured epics, without a Vault full of them. The end fight is tuned as a **gear
  check** (tier-32 gear is ~3.5x survivability there), so without this the only observable
  case is the ungeared one — which is exactly the number that does not need checking.
  ⚠️ **Neither flag gets you an automatic screenshot OF the fight.** Autoplay walks the
  clear path and the bosses sit at a scattered creature position, so it does not reliably
  collide with them; the placement is verified by
  `lowering_the_floor_brings_the_end_fight_to_the_hub` rather than by a captured frame.
  Watching the fight itself still wants a `qa/` bot that steers into it — which is what a
  real tuning pass should use anyway, since it can report rounds and damage rather than a
  picture.

Combine with `MELD_AUTOPLAY` + the file-channel screenshot request. The wrapper
`client/scripts/view_biome.sh <biome> [seed] [frames]` boots the embedded binary with
those flags, sets a pulled-back survey camera via `LOOK_FILE`, and drops
`/tmp/meld-biome-<biome>-<n>.png`. Density ramps with distance (the hub ring is
deliberately sparse), so let it walk out a few frames to see the maze thicken.
## Working alongside other agents (up to ~20 concurrent)

Many agents share this one repo at once. The workflow is built to make that safe — but
only if you respect two shared, machine-global resources: the **server port** and the
**local Postgres**. Read this before you run anything.

- **One worktree per agent; stay in yours.** Each agent works in its own git worktree
  under `.claude/worktrees/<slug>` on its own branch (`claude/<slug>`), branched off
  `main`. Never edit, build in, or delete files in another worktree or in the primary
  checkout — you only touch your own tree. When you're done, `make stop` (below) then
  `git worktree remove <path>` to clean up.

- **Never switch branches — one branch, one worktree.** Your worktree is pinned to
  `claude/<slug>` and *stays there*. Do **not** `git checkout <other-branch>` / `git switch` /
  `git reset --hard <other-branch>` / `git branch -f`, and never touch the primary checkout
  or another agent's worktree. The worktree model exists precisely so nobody has to switch
  branches in a shared tree — switching yanks the files out from under whatever you (and, in
  the primary checkout, the other 19 agents) were doing. Need code from another branch? Bring
  it *to* your branch with `git rebase main` / `git cherry-pick <sha>` / `git merge` — never by
  checking the other branch out. (Git refuses to check out a branch already active in another
  worktree, but don't lean on that as your only guard.)

- **Give your server a unique port.** `MELD_ADDR` is a *single fixed port* (default
  `127.0.0.1:18090`). If two agents run `make play` / `make server` / `make smoke` on the
  default, the second fails to bind. Pick a per-agent port and export it, e.g.:
  ```sh
  export MELD_ADDR=127.0.0.1:181NN   # NN unique to your worktree (18101, 18102, …)
  make server                        # or play / play-dev / smoke — all honor MELD_ADDR
  ```
  `make stop` kills only the server on *your* `MELD_ADDR` port (`lsof tcp:$PORT`), so it
  never disturbs anyone else. **Never** `pkill cargo` / `pkill meld-server` — that kills
  every agent's server on the box. Stop yours by port.

- **Postgres is shared on purpose — don't fight it.** `make play`/`make test` reuse a
  single local Postgres (port `MELD_PGPORT`, default `5433`; data under `target/pg`; DB
  `meldworld`; trust auth). Whoever boots first starts it; everyone else *reuses the one
  already listening* on that port. This is by design and is safe because:
  - the schema is idempotent + additive (concurrent boots don't clash), and
  - the QA suite isolates every run behind **unique UUID-suffixed accounts**, so many
    agents can `make test` at the same time without stepping on each other.
  Therefore: **never** `pg_ctl stop`, `dropdb meldworld`, `rm -rf target/pg`, or truncate
  tables — you'd break every other agent's server and tests. Don't write tests that assume
  an empty DB or use fixed usernames; mint a fresh UUID account like the existing tests do.
  If you genuinely need an isolated DB, set your own `MELD_PGPORT`/`MELD_PGDATA` (then you
  own that instance's lifecycle) rather than mutating the shared one.

- **Build cache & disk.** Each worktree compiles its own `target/` (several GB) — 20 of
  them is a lot of disk and 20 cold Rust builds. To share one build cache across worktrees
  on the same machine, export `CARGO_TARGET_DIR=/abs/shared/target` (Cargo serializes
  builds behind a lock, so this trades disk for occasional build waits). The Bevy client is
  a **separate** workspace under `client/` with its own target — the same applies there.
  `target/` is gitignored, so build artifacts and `target/pg` never get committed.

- **Coordinate on global files — prefer additive edits.** These are shared across every
  branch and are the main merge-conflict hotspots: `balance/balance.toml`, `meld-proto`
  wire types/enums, the spec docs (`GDD.md`, `CANON.md`, `behaviors/`, `interfaces/`),
  `AGENTS.md`, and `Cargo.lock`. *Adding* a `[TUNABLE]`, an enum variant, or a new message
  is conflict-friendly; renaming, reordering, or reformatting existing entries collides
  with everyone. Keep each change small and scoped to one crate/feature, and rebase your
  branch on `main` before opening a PR.

- **Always rebase onto latest `main` before opening a PR *and* before requesting review**
  — this is the standard, not an optional last step. `git fetch origin main && git rebase
  origin/main`, resolve conflicts, then **re-run the build + the relevant tests on the
  rebased code** (a clean rebase can still change behaviour). With ~20 branches in flight,
  `main` moves under you constantly; a stale branch merges broken. Real example: the
  concurrent-battles work and the verticality PR both edited `check_touch` in `meld-world`
  — only a rebase surfaced the same-function conflict (elevation check *and* the
  `in_battle` skip both had to survive). If your branch was cut days ago, rebase before
  you touch it again, too.

- **Commit/push only when asked** (see Conventions). Twenty branches merge more cleanly
  when each is a tight, single-purpose diff.

## Conventions

- **Server-authoritative, always** (CANON §S, D11). All combat math, movement, loot, and
  world generation happen server-side. The client sends *intents* and renders whatever
  the server reports — it never computes combat or generates world content.
- **No gameplay literal in code** (working agreement #2). Every tunable number lives in
  `balance/balance.toml` behind the `meld-balance` loader. Formula *structure* is code;
  *coefficients* are config. Adding a mechanic ⇒ add its `[TUNABLE]`s to `balance.toml`
  and a field to the `meld-balance` struct.
- **Deterministic engine.** `meld-battle` and `meld-world` must stay pure: no `Instant::now`,
  no global RNG, no I/O. Seeded PRNGs only (world gen uses a splitmix64 from the instance seed).
  This is what makes them unit-testable and the game replayable.
- **Wire protocol** (`meld-proto`): realtime envelope is `{type, seq, ts, payload}`, snake_case
  on the wire (CANON §I). Per-session monotonically-increasing `seq`. C2S = intents, S2C = authoritative state.
- **Extending combatant state without a proto change:** per-combatant extras ride the
  `Combatant.statuses: Vec<String>` field as `key:value` tokens the client parses —
  `class:<key>`, `barrier:<n>`, `regen:<n>`, `evasion:<pct>`, `adrenaline:<n>`, `adrenaline_max:<n>`,
  `focus_slots:<n>`, `focus:<kind>:<stacks>`, `pack:<leader|minion>`, and the timed tokens
  `marked` (Explorer Trailblaze: everyone hits it harder) and `distracted` (Explorer
  Misdirection: it swings wide, and the party can flee), `hasted` (a real haste — the gauge
  fills faster while it holds).
  A token nothing renders is a token that does not exist to the player: `pack:` drove
  combat (pack rout) for a long time without reaching the client, so a leader at 1.7x HP
  and its minion at 0.45x — the same species, 3.8x apart — drew at identical size and read
  as a bug.
  Prefer this over adding wire fields for slice-scoped additions.
- **Distance is the difficulty axis.** `tier(d)=floor(d/100)`, `mlevel(d)=max(1,round(d/12.5))`,
  and creature power is **three different curves, not one**: attack
  `stat_mult(d)=(1+d/500)^2.0`, defence `(1+d/500)^0.7`, and HP **linear in tier**
  (`1 + 5.4 × tier(d)`). At d3200 that is 55x attack and **171x HP** — so anything sized as
  "a multiple of the local creature" out there is far bigger than it reads. The single
  `^1.25` this line used to claim was wrong on all three counts, and sizing a set piece
  against it produced a boss ~14x harder than any party can bring. **Read
  `[world_scaling]`, never this sentence, before tuning anything against depth.**
  All threshold checks use the **floored integer** distance.
- **Git worktree layout.** Work happens in worktrees under `.claude/worktrees/`. Branch off
  `main`; commit/push only when asked. Co-author trailer: `Co-Authored-By: Claude <noreply@anthropic.com>`.
  Many agents share this repo at once — see [Working alongside other agents](#working-alongside-other-agents-up-to-20-concurrent)
  for the port/Postgres/build rules that keep concurrent runs from colliding.

## Combat & class taxonomy

Use these terms consistently in code, comments, and UI.

| Term | What it is |
|------|-----------|
| **Run** | One player's ephemeral dive (`PlayerRun`): run-level, XP, backpack, result. Ends on extract or death. |
| **MazeInstance** | One seeded world + its party's runs. Ephemeral; discarded on close. |
| **Area** | A stretch of the seeded corridor in one biome, holding several creatures + a portal. Areas trend larger with depth. |
| **Party** | A player's battle team of up to `party_size_per_player` **heroes of mixed classes** (default: Explorer + Psyker + Resonant + Explorer). Each hero is commanded by its own class's menu. |
| **ATB** | The 100 ms-tick combat: each fighter's gauge fills by `speed_stat/gauge_fill_divisor`; a turn fires at gauge 1.0. Players get a 15 s window then auto-act. |
| **Barrier** | Temp HP: a pool that absorbs damage **before** HP and decays a fixed amount at the start of the holder's turn. |
| **Regen** | HP restored at the start of the holder's turn. |
| **Evasion** | A temporary dodge bonus (added to a fighter's Dex dodge) that decays a fixed amount at the start of the holder's turn. Granted by the Shifter's Flicker. |
| **Adrenaline** | **Hunter** mechanic: a banked resource (0…`hunter_adrenaline_max`) that basic **attacks** build and **skills** spend. A Hunter skill is rejected unless its cost is banked. Rides the wire as `adrenaline:<cur>` + `adrenaline_max:<max>`. Every ability that spends it is a Hunter ability, so the class that earns it must be the class that owns them — see `a_class_that_pays_in_adrenaline_is_the_class_that_earns_it`. |
| **Focus / Manifestation** | Psyker mechanic: a Psyker has N Focus slots (grows with level); each holds a persistent Manifestation that fires every Psyker turn. Each turn it also casts / reinforces / revokes one. |

**Classes** (per-hero; stats in `[player.<key>]`, kit in `meld-battle`):

- **Explorer** — the **default** class: the Explorers map and anchor the unstable world
  ([`docs/lore/factions.md`](docs/lore/factions.md)). Its kit is **tempo and stability**
  rather than burst, and its opener is a **mark**, not a bigger hit: Trailblaze blazes its
  target so *every* ally hits it harder for a window (`marked`, `[battle] explorer_mark_*`)
  — the order whose belief is that nobody accomplishes it alone should be paid for helping.
  Then Field Dressing (L5), Misdirection (L10, the creature is **distracted**: it swings wide and
  the party can leave), Stable Ground
  (L20, party Barrier — deliberately **not** an Anchor: that is the setting's load-bearing
  artifact, takes three orders to make, and only an Explorer of Serin may set one), Safe
  Passage (L35, party **Evasion** — the Guides get you through untouched, they do not
  bandage you afterwards), A World Known (L50, a real **haste** — every ally's gauge fills faster while it holds),
  **Now** (L75, the Globemaster's one call per fight: every ally acts *immediately*,
  refused on the second ask), and **The World Entire** (L100, every enemy marked AND the
  party hastened in one turn). See
  `Battle::resolve_explorer_kit`.
- **Hunter** — the martial baseline (disposal-of-dangerous-creatures guild).
  Front-line bruiser with the standard Attack / Defend / Item / Skill menu. It has no resource
  until it earns one: each basic **Attack** banks **Adrenaline**, and **every** skill SPENDS it —
  Power Strike (heavy hit), Second Wind (L5, self-heal), Snare (L10, damage + ATB-gauge drain),
  Frenzy (L20, biggest hit, biggest cost), Crushing Blow (L35, *upgrades* Power Strike) and
  Apex Predator (L50, *upgrades* Frenzy — the same blow against every enemy). Past 50 it
  learns only **once-a-fight calls**, never more DPS: Iron Lung (L75, a deep self-heal that
  leaves Regen) and Pin the Prey (L100, the whole pack snared at once). A skill is rejected
  unless its Adrenaline cost is banked.
  See `Battle::resolve_hunter` (the Adrenaline resolver, shared with nothing else).
- **Psyker** — psychic channeler. Instead of the martial kit it manages **Foci**: Gravity Well
  (armour-ignoring damage tick), Kinetic Aegis (L5, grants **Barrier**), Mind Spike (L10,
  stronger), Temporal Anchor (L20, drains the enemy's ATB gauge), out through Kinetic Wave
  (L35), Thermal Flux (L50), Matter Dissolution (L75), Phase Shift (L100), Dominate Mind
  (L150), Reality Collapse (L200) to **Event Horizon** (L255, no enemy gauge may pass
  halfway while it is held). See `Battle::resolve_psyker`.
- **Resonant** — healer, and **the best healer by rule**: nothing else may out-heal it
  (`the_healer_is_the_best_healer` holds the Keeper's and the Smithwright's numbers under
  its own). Innate **Regen**, plus ally-auto-targeting skills: Transfuse (L1, heal paid from
  its own HP), Regen Boon (L5), Ward (L10, **Barrier**), out through Mend All (L20),
  Sanctuary (L35), Revitalize (L50), Lifewell (L75), Bloodbond (L100), Martyr (L150) and
  Eternal Bloom (L200) to **Second Life** (L255, once a fight: a *fallen* ally stands back
  up). See `Battle::resolve_resonant`.
- **Shifter** — rogue / fortune-explorer ("Runner"). Fast, fragile front-line skirmisher and the only
  class with innate dodge (base Dex clears the dodge floor). Str/atk-driven kit: Backstab (heavy strike
  that pierces most armour), Flicker (L5, self **Evasion** blink), Steal (L10), Ransack (L20,
  damage + drains the enemy's ATB gauge), then its upgrades — Mug (L35, *upgrades* Steal) and
  Assassinate (L50, *upgrades* Backstab, ignoring armour *entirely*). Its one deep call is
  **Grand Larceny** (L100, once a fight: a Mug against every enemy, and every pocket picked).
  See `Battle::resolve_skill` (the `flicker`/`backstab`/`ransack` arms).
- **Smithwright** — **The Foundry's** builder, and the first of the two profession
  classes (MS-1). A front-line support: Hammer Fall (a staggering blow with the tool
  itself), Quench (L5, self **Barrier**), Plant the Bulwark (L10, **party** Barrier),
  Tempering Blow (L20, an ally's atk for the fight), Slag Spray (L35, all-enemy,
  armour-ignoring), The One True Forge (L50, party heal + Barrier), Anvil Chorus (L75,
  Tempering Blow for the *whole* party) and The Great Work (L100, party heal + Barrier +
  atk together). Out of combat it is
  the class that **raises the field forge**, and a second Smithwright in the party makes
  the anvil's rhythm easier for whoever is working it. See `Battle::resolve_smithwright`.
- **Keeper** — the **Order of the Open Flower's** grower, the other profession class. A
  between-fights mender: Thornlash (damage + gauge drain), Poultice (L5, heal + Regen),
  Bloomfield (L10, **party** Regen), Root Snare (L20, damage + a long wait), Vital Draught
  (L35, Barrier + Regen), Terra's Gift (L50, party heal + Barrier + gauge), Thorn Grove
  (L75, the order's only all-enemy answer: Mnd damage + a drain on each) and World Tree
  (L100, party heal + Barrier + Regen). Its damage
  rides **Mnd**, not Str. Out of combat it **raises the alembic**, whose regen field is
  the only rest a party without a Resonant gets. See `Battle::resolve_keeper`.
  *Neither class has its own sprite set yet — `class_frames` falls back to the Explorer's
  until the art lands.*
- **Phoenix Guard** — the Last City's **anti-undead** order. The tankiest, slowest class
  (most HP + armour, no dodge), and every damaging ability of theirs hits **undead**
  `phoenix_guard_undead_mult` harder. Its ladder is the order's rank ladder: Silvered
  Strike (Initiate), Rite of Rest (Purifier L5, self **Barrier**), Holy Censure
  (Exemplar L10, zeroes the gauge), Purging Light (Luminary L20, **all-enemy**), Unbroken
  Vigil (Redeemer L35, **party** Barrier), Eradication (Apotheosis L50, an execute that
  scales with the target's missing HP), Hallowed Ground (L75, all-enemy damage that zeroes
  *every* gauge) and Phoenix Ascendant (L100, heavy all-enemy fire + party Barrier). See `Battle::resolve_phoenix_guard`.
  *The kinetic/oar kit it used to carry belongs to the **Order of the Iron Hull**, a
  future monk class whose `iron_hull` key is reserved.*

**Abilities are one registry.** [`meld_proto::skills`](shared/meld-proto/src/skills.rs)
owns every ability's key, name, owning class, unlock level, **org rank**, **description**
and **`target`** (Enemy / Ally / Caster / AllEnemies / Party). The server gates on it, the battle menu builds its rows and tooltips
from it, and the party screen lists each hero's ladder from it — so a kit is defined
once, **in ladder order** — `skills_for_class` sorts by unlock, because the table is
written in authoring order and the Explorer's `Now` (49) sits above `A World Known`
(36) in it.

**Nobody stops learning, and the archetype governs WIDTH not depth.** Every class learns
something at **50 and again at 100** — five of the eight used to stop at 25
or 36 against a level cap of 255, so levelling stopped paying for most of the roster. What
still separates a martial class from a caster is how it gets there: a **martial** class
(Hunter, Shifter) climbs via `upgrades`, so Frenzy *becomes* Apex Predator and its menu
stays lean; **hybrid** may field 8 and **caster** 11 (`menu_width`). A martial class's
*repeatable* rows stop improving at 50 — everything it learns after is a **once-a-fight
call**, not more DPS, which is what "it scales on gear" means mechanically. Tests hold both
halves.

**A once-a-fight call is spent CENTRALLY** (`resolve_skill`, on any successful resolve), not
by each arm remembering to push its own key — that was a list, and an ability left off it is
simply infinite. `is_once_per_battle`: Now, The World Entire, Iron Lung, Pin the Prey, Grand
Larceny, Hallowed Ground, Second Life.

**Crafters have a SECOND ladder, and it is the perk system.** Every class earns an
overworld perk that scales with run level — the Explorer's lantern and map, the Hunter's
prey-sense (level, HP, battle gauge — and threat sense, which is the same eye at range),
the Shifter's Shift-sense, the Resonant's walking
regen, the Phoenix Guard's bulwark — and the two PROFESSION classes had none at all, which
is the pair whose whole identity is what they do between fights. `compute_perks` is now a
free function (it reads only balance) so it is unit-tested, and
`no_class_walks_the_overworld_with_nothing` reads the class list off the registry rather
than a hand-written one, because the two that were missing were missing for a whole
release and nothing said so.

- **Smithwright** — *Prospector's Eye* (ore veins revealed past the interest radius),
  *Efficient Setup* (benches raise quicker and cost less stock), *Travelling Forge*
  (packing one up returns the WHOLE stock, not the salvage), *The Long Shift* (its benches
  serve extra jobs before they are spent).
- **Keeper** — *Forager's Path* (reagent beds revealed at range), *Green Thumb* (a tick
  sometimes pays two units), *Rooted Ground* (the alembic's regen field reaches further and
  heals harder), *The Whole Vein* (a unit sometimes costs the bed no stock).

**A passive must not do the job the class is FOR.** The Resonant's walking regen used to
mend the whole party between fights, so a party carrying the best healer in the game never
needed healing and its kit went unspent. It now tends **only the Resonants themselves**; the
Keeper's alembic field still reaches everyone, because a field is a PLACE you choose to
stand rather than something you get for bringing someone. Both pour through `pour_regen`,
which takes the eligible slots — the two sources bank their sub-1 remainders separately, or
the field's overflow would heal straight through the Resonant-only rule.

**The Psyker's overworld perk is a VERB, because seeing is taken.** Its threat sense moved
to the Hunter (where it stopped duplicating the intel lane), and the map is the Explorer's,
so what is left for the order of manifestations is **telekinesis**: tap a creature and it is
PINNED where it stands. It stops moving, chasing and skirmishing — but it is still touchable
and still fights, because a pin is an opening the party chooses to take, not a way to delete
an encounter. Engaging a pinned creature opens the fight with **every hero's gauge full**
(`build_battle(.., surprise)`), which is the entire reason to spend one. Held on a
**cooldown**, and the numbers answer one question: *can a Psyker keep everything it can
reach pinned forever?* To sustain N pins you must lay one every `seconds / N`, so the
cooldown stays above that line at **every level** — a test walks 1..=255, because the first
tuning passed at level 1 and failed at 255. **Mind Link** (a later rung) force-includes
co-op teammates in the snapshot at any distance — positions only, since the map belongs to
the Explorer.

**A Focus can grow ASPECTS, and that is what makes the Psyker a controller.** A manifestation
in the class doc is a chain with prerequisites, not one effect: Gravity Well is Pressure,
then **Gravity** on what is being crushed, then **Anchor** on what is already slowed.
`SkillDef.requires` names the parent; an aspect may only be cast onto a target already
holding it, occupies its own Focus slot, and falls when the parent does
(`drop_orphaned_aspects` loops — a chain is three deep). It takes the parent's OWN target
**only when it lands on the same side**: Gravity drags what Pressure is crushing, but
Acceleration hurries an ALLY while its parent grinds an enemy, so it keeps its own aim.
Seven aspects ship — Gravity/Anchor, **Shield** (the ward covers the party), **Acceleration**
(fills an ally's gauge), **Freeze** (slows, and pins anything already slowed), **Brittle**
(strips ONE elemental resistance a turn, for good) and **Blackout** (its own blows go wide).
**Accuracy lives on the DEFENDER here** (`roll_dodge` reads the ATTACKER's `distracted` /
`blinded` off `active_actor`), so "it cannot see" has to land as a bonus to whoever it is
swinging at — a creature has no dodge of its own to take away (`Fighter::dodge` is only ever
set for heroes).
The doc's REACTION aspects (Dampen, Static, Vent, Flicker, Rewind) and its POSITIONAL ones
(Push, Pull, Warp) stay unbuilt: this engine has no reactions and no battlefield positions,
and reinventing them as something else wearing the name is worse than leaving them out. Both tiers of slow are
**rates, never caps**, and the strongest wins rather than stacking. An aspect is **not a
menu row**: `skills_for_class_at` excludes it, `aspects_of` returns it, and the battle menu
lists it indented under a parent that is currently held — so the menu grows in DEPTH without
growing in width, and you can never press something the engine would refuse.

**Mind's Eye and Dual Manifestation are ONE mechanic** — a cast that does not spend the
turn (`Fighter::free_casts`). Mind's Eye seeds the pool at battle start from the hero's
level; Dual Manifestation tops it back up each Psyker turn. Only a cast that LANDED spends
one, or a refused aspect would quietly burn the opening. **Expansion** widens an offensive
Focus across the line for a share of the tick: a controller should get wider with level,
and hitting one target harder is what the Hunter and Shifter are for.

A crafter reads only the half of the world its own trade is built on — the Foundry sees
`ore`, the Open Flower sees `reagent`, keyed off the material registry's class. Node-sense
is **force-included** in that player's snapshot the way the portal is, never by widening the
shared interest cull, which would show everyone everything. Both harvest perks roll off
`hash_str(node) ^ hash_str(player) ^ tick` so the outcome is reproducible rather than
wall-clock.

**A gauge CAP is a soft-lock; slow the RATE instead.** Creature `speed_stat` is a fixed
constant (40–125) that never scales with distance, while a hero's climbs with Dex — so a
deep hero takes several turns per creature turn. Anything that pins a creature's gauge to a
ceiling therefore knocks it back below the line every time it approaches one, and it never
acts again. Event Horizon slows the fill RATE (`HORIZON_STATUS`, through the same
`status_slow_mult` a web or chill uses), which cannot lock; Hallowed Ground zeroes every
gauge outright and is gated to once a fight for the same reason.

**Every lasting effect answers to ONE ceiling: `[battle] max_effect_stacks` = 5.** Regen,
Barrier, Evasion and the fight-long attack buff all count stacks on the `Fighter`, and a
grant past the ceiling is REFUSED rather than silently wasted — so "how many of these can I
hold" is one number a player learns once instead of four they discover. Everything goes
through `grant_regen` / `grant_barrier` / `grant_evasion` / `grant_atk`; **never write
`fighter.regen += x` at a call site**, because the ceiling lives in the helper and a direct
write is a stack nobody counted. Consumables answer to it too — a stack is a stack whether
it came from a spell or a bottle.

**Regen DECAYS, like the Barrier beside it** (`regen_decay_fraction`, floor 1). It was the
one lasting effect in the game with neither decay nor expiry: `regen +=` accumulated without
limit and never faded, so a healer spending turns on it bought permanent, ever-growing party
sustain — measured at 5 stacks healing 150 HP a turn, forever. When an effect drains to
nothing its stacks come back, so a long fight can be re-tended rather than capped for good.

**A fight-long attack buff is a share of `base_atk`** (snapshot at battle start), so five
stacks is a linear 5x the single bonus. Computed off the CURRENT attack and added it would
compound instead — five Anvil Chorus casts to 1.76x and ten to 3.1x.

**Two hand-written lists used to shadow this registry, and both had gone stale.** The
engine dispatched `resolve_skill` by a per-class list of keys, where an unlisted ability
fell past every arm and returned "unknown skill" — a row that is in the menu, costs a turn
and does nothing. The client picked targeting from another list that still named the Iron
Hull's `root` / `toll_of_the_deep`, so the Phoenix Guard's self-cast Rite of Rest and its
all-enemy Purging Light both asked the player to aim at a single creature. Both now ask the
registry (`skill_owner`, `target_of`). **Never reintroduce a list of ability keys** — a
list is a list a new ability gets left off, silently.

**A PASSIVE must not beat a spent TURN, and nothing party-wide may out-heal the healer.**
`synergy_party_regen` was flat 3 — 7.5% of a level-1 hero, granted to EVERY hero for free,
nearly **4x the Resonant's own innate regen** (`resonant_regen_fraction`, 2%) and 5x what a
whole turn of the Keeper's party-wide Bloomfield buys. Played through `mcp/`, it made a
standard on-ramp encounter arithmetic rather than a fight: 24 HP of party healing against 25
points of incoming damage, so a four-hero party lost 12 HP of 176 across 32 hero-turns.
Both synergy grants are fractions now, and
`a_free_synergy_is_worth_less_than_a_spent_turn` holds the ordering.

**Every magnitude that lands on a hero is a FRACTION, never flat points.** A hero runs 40
max HP and 12 atk at level 1 to ~535 and ~309 at level 100, so a flat grant is a third of a
hero early and a rounding error late. That is not hypothetical: the Keeper's heals were flat,
so World Tree — its level-100 capstone — restored 4.9% of a hero where the Resonant's
restored 85%, and the class stopped being a healer around level 30. The Smithwright's
`+4 atk` Tempering Blow was worth 33% at level 1 and 1.6% at 100. Barrier decay was flat too,
so a deep hero's Barrier outlasted the fight. `Battle::scaled_to` / `grant_regen` are the one
way to turn a fraction into a grant; `every_magnitude_that_lands_on_a_hero_is_a_fraction`
fails on any tunable that reads like points.

**Unlock levels are ROUND** — `skills::RUNGS` = 1 / 5 / 10 / 20 / 35 / 50 / 75 / 100 / 150 /
200 / 255. Squares (1/4/9/16/25/36/49) are retired: a player counting to their next ability
should count in tens, and `49` was the only thing standing between the deep rung and a
legible **50**. `ladder_top` is 255 for a caster and 100 for everyone else — so each new
ability costs a step up in commitment rather than an ever-flatter trickle. A test holds
EVERY class in the registry to it: a hand-written list of classes is a list a new class
gets left off, which is exactly how the Smithwright and the Keeper shipped on
1 / 4 / 12 / 20 / 28 / 36. The **org ranks** are a separate, far slower ladder
(1 / 25 / 65 / 115 / 165 / 215) and gate nothing — they are standing, not power.

**A description without a number is flavour.** The registry can only say what KIND of
thing an ability is — magnitudes are `[TUNABLE]`s and `meld-proto` is shared with a
client that has no `balance.toml` — so every row read as mood: "A heavy blow. Spends
Adrenaline." never said 40 of 100, and you could not tell Power Strike from Frenzy
without pressing one and being refused.
[`meld_run::ability_effects`](server/crates/meld-run/src/ability_effects.rs) formats the
magnitudes from balance and they ride the roster (`run.party` → `abilities`), so the
battle tooltip and the Abilities panel both show prose then numbers, and a retuned
`[TUNABLE]` retunes the tooltip. A new ability with no arm there **fails a test** rather
than shipping a blank line. The two halves also have to agree: the registry had shipped
Sanctuary promising Barrier while granting Regen, and Revitalize advertising "no HP cost
to you" while charging 30% of the heal — `the_prose_and_the_numbers_agree` is what
catches that now.

New classes: add the enum variant (`meld-proto` `CharacterClass`), `[player.<key>]` stats +
any `[battle]` tunables, the `class_key` mapping (`meld-run`), the kit in `meld-battle`, and the
client menu branch (`menu_entries` keyed off the active hero's `class:` status).

## Leveling & attributes

- **The XP curve is stated in FIGHTS, not points**: level `L` costs
  `fights_per_level_base` same-level encounters, plus one more every
  `fights_per_level_ramp` levels (`[runs]` in balance; `meld-run::fights_per_level`).
  `xp_to_next` multiplies that by what a same-level encounter actually pays, so
  retuning creature XP retunes the ladder with it instead of silently desyncing.
  At the shipped values a level costs 2 fights at first and level 10 — the gate on
  your **second party slot** — costs 22, with the ramp biting later (65 to L20, 128
  to L30). `PlayerRun::award_hero_xp` levels each hero on victory.
- **An encounter is a POOL divided among the heroes still STANDING.** The last survivor
  of a bad fight banks the whole thing — a fight that nearly killed you is worth what it
  cost. `award_hero_xp` takes `shares` (the living count) SEPARATELY from `party_size`
  (which only sizes the per-hero vectors); they used to be one argument
  (`party_size.max(slot + 1)`), so a lone survivor in slot 3 still divided by four and
  three-quarters of the pool evaporated. Each hero also carries its **own** banked XP and
  its own next-level bar on `run.party` — those were one shared run-level pair, which is
  why the split was invisible from inside the game.
- **Encounter XP is split across the party, once.** A four-hero party meets
  creatures with `encounter_party_scale` more HP, so the encounter pays that same
  multiple before the split — otherwise the scale is charged twice and a full party
  earns at a fraction of the solo rate for the same effort. The split itself is the
  intended cost of fielding more heroes. A co-op **joiner** does not re-scale the
  creatures and so does not inflate the payout: more heroes splitting the same XP is
  what pushes a full co-op group toward much harder fights.
- **Four attributes** (`[player.<key>]`: base + `*_per_level`): **Str**→physical atk,
  **Mnd**→manifestation/spell power, **Dex**→ATB speed + dodge, **Wll**→HP + defence. A hero's
  attribute = base + per-level gain × (level−1). Each derived stat = *class base stat* +
  (attribute − base attribute) × coefficient (`[attributes]`), so **a level-1 hero has exactly
  its class base stats** (nothing shifts) and every level's auto-gained attributes become growth.
  Derivation lives in `meld-run::party_fighters`; the `Fighter` carries `str_/mnd/dex/wll`,
  `spell_power` (Mnd-driven, used by Psyker Foci instead of `atk`) and `dodge`. Attributes ride the
  wire on `statuses` (`str:`/`mnd:`/`dex:`/`wll:`), shown in the battle party cell.
- **Skill unlocks by level**: the single source of truth is `meld_proto::skills::unlock_level`
  (server rejects a locked skill in `resolve_skill`; client greys the menu row). Second Wind L4,
  Mind Spike L10, Temporal Anchor L20, Regen Boon L5, Ward L10; every class's ladder sits on
  `skills::RUNGS` (1 / 5 / 10 / 20 / 35 / 50 / 75 / 100 / 150 / 200 / 255), not the org
  ranks, and every class has a rung at **50** and at **100**.
- *Deferred*: MP (the ATB adaptation has no cast resource yet — Mnd would gate it later).

## Overworld: exploration, extraction & harvesting

The overworld is not a single-file corridor: it's a tall (±`lateral_half_extent`),
scroll-in-every-direction map. Creatures **scatter across ±y** (area 0 stays on the
centre line for the deterministic tutorial), so you explore in 2D to find fights and
nodes. Placement + roaming live in `meld-world::Arena::generate` / `step_creatures`;
the snapshot tags entities on `avatar_state` — `mob:<kind>:<faction>`, `portal`,
`resource:<kind>`, `obstacle:<kind>:<radius>`.

- **Biome terrain.** Each area (≥1) is scattered with impassable `Obstacle`s —
  biome-specific trees/cliffs/water/lava (`obstacles_for_biome`, `[worldgen]` radius
  tunables). Movement collides with them and **slides** (`Arena::apply_move`); roaming
  creatures avoid them too. A **guaranteed clear path** (`Arena::path`, a meandering
  polyline hub→portal) is carved first and obstacles are rejection-sampled to never
  enter its `path_clear_radius` tube — so a route to the exit is *always* feasible by
  construction (unit-tested across seeds). The client draws the path as a faint trail
  (sent on `run.started`, field `path`).

- **Density is a per-AREA question, and the fan distorts it.** WG-4 bends a fixed-width
  corridor into an arc that grows with radius, so anything placed *per unit of corridor* is
  smeared ever thinner outward: at r=230 the arc is ~1400 units across. Both creatures and
  maze fill compensate (`creature_radial_lane_cap`, `maze_radial_scale_cap`) — creatures by
  walking the corridor once per corridor-width of arc, obstacles by scaling their count. The
  trap is the other half: **any spacing/adjacency check must be asked in the BENT frame**,
  because corridor y is an *angle*. Comparing raw corridor distance is what made the forest
  ask for 392 trees and place 90 (a wood that read as a field), and it is why creature
  placement measures separation in world space. Both use a grid rather than a scan, since the
  world streams outward without bound. Two invariants are held by test: density-per-unit-area
  must not collapse with depth, and no two standard spawns sit inside `[ai] group_radius` of
  each other — a PACK is the only thing that may make a group, or the encounter ramp promises
  duels and quietly hands out fives.

- **Field and Forest are the same ground, different tree counts.** They share fauna, flora,
  ground texture and dungeon pool; only `field_obstacle_mult` vs `forest_obstacle_mult` (1.3
  vs 7.0) separates grassland you can see across from a wood you cannot. That contrast IS the
  content, and a test holds the ratio.

- **A WORLD OUTLIVES ITS DIVERS, AND THE SHIFT IS WHY IT CAN.** The `WorldActor` is no
  longer torn down when the last run ends (CANON §W1): it keeps its seed, its streamed
  sections, its cleared ground and its Shift schedule, and it keeps ticking with nobody
  in it. It outlives the **process** too — `worlds` in Postgres stores four integers and a
  small JSON delta, never the map, because §W5's whole claim is that the baseline is a
  pure function of the seed. Restoring is the *normal* build reading its seed off disk:
  regenerate, stream the frontier back out, **replay the Shift log**, then re-apply what
  players changed. The log stores each Shift's *span* and not just its generation because
  `shift_region` picks the least-recently-disturbed section half the time — at restore
  every section exists at once, so re-deriving the span would pick differently than a
  world that grew into it did. That is history, and history has to be written down. What used to justify the teardown was that a felled creature never came back, so
  the second dive found an empty map — that is answered by content now, not by amnesia.
  **A tutorial world is the one exception** and still dies with its diver, because
  persisting the guided first dive hands the next player a corridor someone already walked.
  Two things ship with persistence rather than after it, because a world nobody refreshes
  is strictly *worse* than an ephemeral one: **regrowth** (`Arena::regrow` — a slain
  creature's ground is banked by `prune_defeated` into `Arena::fallen`, a few fields
  rather than a corpse the AI and the snapshot walk every tick, and a fresh one of the
  species stands back up there; nodes re-stock, chests re-seal far more slowly because
  treasure is the one thing farming must not print), and **the Shift**.

- **The Shift is scheduled from `(seed, generation)` and driven by the TICK COUNTER,
  never wall-clock** (CANON §W2, structural — [`meld_world::shift`]). That is not a style
  point: it is what lets two integers replay a world's entire history, which is what makes
  §W5 persistence cheap. `WorldActor::tick_count` is the world clock; `meld-world` stays
  pure and is *handed* the tick rather than reading one. **A region is a whole SECTION
  span** (1-3 contiguous), the translation of CANON's 1d6 Tiny…Cataclysmic table — and it
  is section-granular for a concrete reason: the client already keys its ground shader and
  biome label off per-section radius rings, so re-sending `world.terrain_section` **is**
  the retile and the Shift needed no new rendering path at all.
  **The props are RE-SCATTERED and the mountains RE-CUT, not reskinned** (`reroll_props`
  / `reroll_peaks`): the incoming biome strews its own count at its own density in its own
  places, so a wood becoming desert genuinely thins instead of turning into
  differently-coloured trees in the same spots. **Elevation is the heightmap plus PEAKS**
  — discrete terraces are retired (`[worldgen] terraces_per_area = 0`), so a peak is what
  "the land changed shape" has to mean, and it is biome-weighted through the same
  `biome_terrace_mult` generation uses: Ashfall raises ranges where Desert flattens them.
  A peak is a **smooth walkable dome** (`height <= radius * PEAK_MAX_ASPECT`), which is
  exactly why it is safe to re-roll mid-run where re-rolling the height FIELD would not
  be: a per-region height offset would tear at the ring's edge into a wall nobody can
  cross, and the clear path crosses that edge. Peaks are keyed **per section** on the
  client now — a re-sent section replaces its own mountains rather than growing a second
  one beside each of the first, which is what re-sending for a retile would otherwise do.
  Placement rejects the clear-path tube exactly as generation does, so the way out stays
  feasible **by construction** — but a prop can still land on a player standing off-trail,
  and the answer is to **move the player, not to constrain the world**: `rescue_stranded`
  walks anyone a prop landed on — or anyone the ground rose under — back to the region's
  entry (the clear-path waypoint at its inner edge, open ground, level 0, on the route they
  were already following) and the server sends them a `movement.position_correction`,
  because the local avatar chases the snapshot exponentially and a teleport with no
  correction renders as a second-long slide with the camera along for the ride.
  **The trail is safe from the rescue, and that is the rule worth knowing**: the
  clear-path tube holds no prop and no raised ground by construction, so a player who
  stayed on the route is never moved and a Shift costs them only HP. Walking off-trail is
  what puts you somewhere the world can rearrange out from under you. Force damage is a **fraction of
  each hero's own max HP** scaled by the region's size, and a party wiped by one dies
  through `world_death` (the dungeon-trap path, renamed — it is now also how you die to
  the weather). **The `[biome_gate]` is checked SIDEWAYS too**: the gate that holds the
  tundra's armoured bruisers outward on the way out would otherwise let a Shift drop them
  onto the d80 on-ramp. Half of every Shift's picks are least-recently-disturbed so the
  churn cannot pool in one ring while the rest of the world becomes a museum, and half are
  uniform so it is not simply "it always lands where you aren't". Bounty marks, chests and
  raised stations survive: contesting a Shift is what BD-3's **anchors** are for, and
  `apply_shift` is the one place a pin will have to be consulted.

- **WHAT PLAYERS BUILD IS ONE PRIMITIVE** (CANON D21/§W3, BD-2). A `Structure` is
  HP-bearing and destructible and a `function` tag varies its role —
  [`meld_proto::structures`] is the registry both sides read, `[building]` holds the
  magnitudes, and the handler branches on the function for its NUMBERS and nothing else.
  CANON is explicit that this is a discipline, not a convenience: *do not build towns,
  anchors, portals and camps as separate systems*. A new buildable is a row in that table.
  A `wall`'s `blocks` flag joins terrain in **`Arena::blocking_field`** — ONE list built in
  ONE place, because the alternative was tried and failed inside a single release: the
  collision line went into `step_creatures_with_aggro` and not into `apply_move`, so walls
  stopped creatures while players strolled through them. A comment saying "some call site
  will forget" does not stop a call site forgetting; a shared function does. It rides the
  snapshot as one `structure:<function>:<hp>:<building>` tag for the same reason.
  **You cannot wall someone in**, answered twice over: a blocking structure is REFUSED
  within `no_build_near_player` of anyone else (an anchor is exempt — it does not block, so
  it cannot pen anyone), and the spacing rule keeps a ring escapable
  (`min_spacing > 2 x (structure_footprint + player_radius)`, true by 0.6 units and by
  accident until a test made it a rule — one rings a player with the tightest legal cage
  and walks them out, another closes the gap to prove the first can fail).
  **And when something puts a player inside geometry anyway, the world walks them out**:
  `Arena::rescue_trapped` is the Shift's rescue with no event behind it, swept on
  `stuck_check_ticks`. Both rescues share one `trapped_at`, which reads `blocking_field` —
  two copies of "what blocks" drift, and the drift is invisible until a player is standing
  in the difference.
  Placement is validated BEFORE the stock is spent, and **nothing may be built on the
  clear path** — the route out is feasible by construction everywhere else, and a
  player-built wall across it is the one thing in the world that could seal the exit.
  A structure goes up weak and ramps to full, and the ramp never heals damage taken
  mid-build, or a besieged wall repairs itself for free. **Anyone may repair; only the
  owner may demolish** — hauling ore to a teammate's anchor is the co-op verb, while taking
  something down is a decision about someone else's work. MS-1's field benches are the one
  pre-primitive holdout; folding them in is `BD-6`.

- **AN ANCHOR IS PERMANENCE YOU KEEP PAYING FOR** (BD-3). It holds every section its
  `pin_radius` reaches and the Shift due there does not land — but it does **not change
  the schedule**, only the outcome: the cadence stays a pure function of the seed
  (§W2/§W5) and the suppression is the *event*, which is why a held Shift never reaches
  the replay log. Every Shift it turns aside takes `shift_hold_damage_fraction` of its own
  max HP, so an anchor nobody hauls ore out to falls on its own and hands the ground back.
  Without that, BD-3 would be "plant one, forget the region" — the opposite of the loop it
  is named for. Creatures do not siege structures yet (`BD-4`), so the weather is currently
  an anchor's only enemy.

- **Per-section seeds & streaming.** Each area is a **section** generated from its OWN
  seed `section_seed(run_seed, n)` (`meld-world`), so sections are independent +
  reproducible. `Arena::ensure_frontier` streams new sections on demand as the player
  advances (endless past the initial `area_count` chain; the deep portal stays at the
  chain's end). The game loop streams new sections' terrain each tick.

- **Verticality (terraces + connectors).** Each section carries a `Terrain` elevation
  grid + `Connector`s (slope/ladder/rope). Terraces are raised plateaus kept OUT of the
  clear-path tube, so extraction stays on level 0 and always feasible; cliffs are
  impassable walls and a **connector is the only way to change level** (no free
  climbing). `apply_move`/`check_touch`/`harvest`/`at_portal` are elevation-aware.
  Rides the wire as `SnapshotEntity.level` + the `world.terrain_section` message; the
  client builds a stepped ground+cliff mesh per section and connector props. See
  [`VERTICALITY-PROPOSAL.md`](docs/proposals/verticality.md). `[worldgen]` tunables:
  `terraces_per_area`, `max_level`, `terrace_min/max_size`, `terrain_cell`,
  `connector_radius`, `stream_lookahead`.

- **Extraction is mostly the Town Portal item.** There is a **single fixed portal**,
  deep at the end of the last area (`Arena::portal`). The primary way home is the
  **Town Portal** consumable (`begin_extraction { method: "town_portal" }`): it works
  from anywhere, is checked at channel start and **consumed on completion** (not on
  interrupt). Each dive starts with `starting_town_portals`; felled creatures drop
  more at `town_portal_drop_chance`. Client keys: `E` = deep portal, `T` = Town Portal.
- **Harvestable resource nodes** (`ResourceNode`) scatter through every area (area 0
  gets one guaranteed starter node) and hold **finite stock**. `run.harvest { entity_id }`
  opens a **channel** (MS-2) that hands over **one unit per tick** while you stand still:
  each unit banks the node's `material` into the run backpack (extract to keep it; feeds
  Forging/Alchemy crafting) and credits the node's Meld `skill` its `xp`. Stock + tick
  pace come from the material's **class** (`[harvest]`: reagent = quick units, ore = a
  slower dig). Interruption is strict but costs only the tick in flight — moving, a
  battle, `run.cancel_harvest` or walking out of range ends it and keeps every banked
  unit. Biome→node ids in `resources_for_biome`; stats under `[resource.<kind>]`.
- **Materials are one registry** ([`meld_proto::materials`](shared/meld-proto/src/materials.rs)):
  every material key with a **class** — `reagent`/`ore` (harvest nodes), **`refined`**
  (smelted stock) and **`trophy`** (the combat drop a felled creature banks,
  `combat_material_for_biome`) — plus a tier per biome band. The class is what recipes
  and the Forge gate on: Alchemy's **trophy line** takes monster parts; the **smelt
  line** turns 2 raw ore into 1 `refined` (Forging, gated by band); the Forge builds
  from *refined* stock plus an optional *trophy* **catalyst** (a tier past the smith's
  own reach); and the **Broker** (`/v1/vendors/broker`) buys any material for chits +
  Mercantile XP. So Forging's pipeline is `harvest ore → smelt → forge`. A drop key missing
  from the registry is loot nothing can spend, and unit tests fail on it. Design:
  [`proposals/crafting-and-professions.md`](docs/proposals/crafting-and-professions.md).
- The run backpack rides the wire on `run.backpack_update` (added/removed changes with
  a `cause`); the client mirrors it into `RunBackpack` for the overworld HUD.

## Fights are opt-in (no auto-pull)

Each player is their **own battle-party** (`form_run` adds one party per player), so
touching a creature pulls only YOUR heroes. A teammate near an ongoing fight opts in
with `run.join_battle` (server checks they're within `[ai] join_radius` of
`ActiveInstance::battle_pos`) — touching a creature while a fight is in progress does
nothing. Fighting players show a ⚔ marker + a "Press [J]" prompt on the overworld;
joiners render as an "allies" strip on the battle screen.

## Heroes: persistent names, stats on the party screen

- **Names persist per account** (`heroes` table, one row per slot; seeded on register).
  Loaded into the session on connect (`flush_hero_loads`), attached to the run, and
  ridden into battle on each ally combatant's `statuses` as `name:<name>`. Rename via
  `run.rename_hero` (realtime — updates the run + session + persists + re-sends the
  roster) or `PUT /v1/heroes/:slot`. The party builder / inventory party screen edit them.
- **Attributes live on the party screen, not the battle HUD.** The server sends the
  caller's roster (`run.party` → `HeroView` name/class/level/Str/Mnd/Dex/Wll/HP) at run
  start and on level-up; the client shows it in the inventory overlay. The battle cell
  deliberately omits stats.

## Deep Dives

- **Combat / ATB** (gauges, turns, flee, merge, statuses): [`behaviors/combat-atb.md`](docs/behaviors/combat-atb.md)
- **World generation** (distance→difficulty, biome bands, areas, portals): [`behaviors/world-generation.md`](docs/behaviors/world-generation.md)
- **Run lifecycle** (enter-maze, extraction, death durability): [`behaviors/run-lifecycle.md`](docs/behaviors/run-lifecycle.md)
- **Economy / meta / endgame / disconnect / async**: [`behaviors/`](docs/behaviors/) (`economy.md`, `meta-progression.md`, `endgame-seasons.md`, `disconnect-handling.md`, `async-interaction.md`)
- **Realtime protocol** (session, movement, battle, run/social messages): [`interfaces/realtime-protocol.md`](docs/interfaces/realtime-protocol.md) + [`interfaces/realtime-protocol/`](docs/interfaces/realtime-protocol/)
- **HTTP API** (auth, runs/world, vault/gear, crafting, economy, leaderboards): [`interfaces/http-api.md`](docs/interfaces/http-api.md) + [`interfaces/http-api/`](docs/interfaces/http-api/)
- **Data models**: [`interfaces/data-models.md`](docs/interfaces/data-models.md) + [`interfaces/data-models/`](docs/interfaces/data-models/)
- **What we're building next (checkable worklist)**: [`ROADMAP.md`](docs/ROADMAP.md)
- **Milestones & tasks**: [`BUILD-PLAN.md`](docs/BUILD-PLAN.md)
- **Feature proposals**: [`proposals/last-city.md`](docs/proposals/last-city.md) (the hub), [`proposals/verticality.md`](docs/proposals/verticality.md), [`proposals/crafting-and-professions.md`](docs/proposals/crafting-and-professions.md) (crafting depth; why professions are Meld skills, not classes)
