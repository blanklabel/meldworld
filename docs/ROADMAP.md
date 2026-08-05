# MELDWORLD Roadmap

> **This is the live worklist.** It sits above the milestone plan in
> [`BUILD-PLAN.md`](BUILD-PLAN.md) (which decomposes teams/tasks) and below the
> design vision in [`GDD.md`](GDD.md) / [`CANON.md`](CANON.md). Where this roadmap
> and the spec disagree on *intent*, the spec wins — this doc tracks **what we're
> building next and whether it's done**, not new canon. When an item's design
> hardens, fold it into CANON with a §/D-number and a `behaviors/…` file, the way
> verticality and Last City are graduating.

## How to use this roadmap (agents: read this)

- **Every item is a checkbox with a stable ID** (e.g. `LC-2`, `GR-3`). Cite the ID
  in your branch name, commit, and PR title/body — `Fix reversed walk direction
  (LC-2)`.
- **Check the box in the same PR that lands the item.** When you finish an item,
  flip `- [ ]` → `- [x]` here and, if it changed observable behavior, add/adjust
  its `behaviors/` or `interfaces/` spec. A merged item with an unchecked box is a
  bug in this file — fix it.
- **Partial progress stays unchecked.** Use the sub-bullets to record *what's
  done vs. what remains*; only tick the top box when the whole item is shippable
  and verified (screenshot for anything the client renders — see
  [`CLAUDE.md`](../CLAUDE.md) "Visual verification"; a QA test for server rules).
- **Respect the concurrency rules** ([`CLAUDE.md`](../CLAUDE.md) → "Working
  alongside other agents"): additive edits to `balance.toml` / `meld-proto` /
  specs, unique `MELD_ADDR`, rebase on `main` before PR. This file is a merge
  hotspot — keep your edit to *your* item's line.
- **Status legend:** `- [ ]` not started · `- [ ]` + 🟡 note = partially built ·
  `- [x]` done. IDs are permanent; don't renumber.

Ordering below is roughly by dependency and value, not a hard sequence — several
epics can run in parallel. **Phase 1 (right below) is the curated MVP cut — aim agents
there first; the epics after it are the longer-term vision.**

---

## Phase 1 — Path to Playable (fun first)

**The MVP cut. Aim agents here first.** The build already has the core loop (dive → ATB
fights → extract-or-die → bank → dive again), five classes, biomes + verticality, co-op +
raid-merge, and elites/gatekeepers. What it lacks — to be a game people *want to keep
playing* — is a **reward loop that pays off** and **polish**, not more world-sim. Phase 1
closes exactly that gap. **Everything under the CR / BD / SC / EW / SOC / MON epics below
is the *vision*, deferred until Phase 1 proves the core is fun.**

> **How to read this section:** items already defined in their epics are **referenced by
> ID** (`→ **AD-1**`) — do the work there and **tick the box in its epic**, not here.
> `P1-*` items are **net-new** and checked here. Ordered by leverage.

**① Make loot a chase — the #1 retention lever.** Every dive should produce something
exciting; today's drops are flat stat sticks.
→ **AD-1** (gear affixes — the star) · **GR-1** (7 equipment slots — affixes need them) ·
**GR-5** (class-locked kit + two-handed — gives affixes a *class* to twist) ·
**GR-6** (Ephemeral instead of "red", with a tooltip — a trust fix) ·
**GR-2** (durability & the wipe — the repair sink + "you lose what you didn't extract").

**② Close the economy loop — make the Vault mean something.** dive → loot →
craft/upgrade/spend → dive stronger. Without this the persistent half is inert.
→ **MS-1** (crafting: Forging/Alchemy/Mercantile — turn your haul into gear + repairs) ·
**LC-4** (manage/equip/craft inventory inside town — its home) ·
**EC-1** (player stalls — sell your surplus; *may trail — crafting is the must-have, the
market is the multiplier*).

**③ A reason to dive beyond "deeper" — purpose + a scoreboard.**
- [x] **P1-1 — Turn on the Vanguard board (basic).** The seasonal deepest-distance
  leaderboard ([`behaviors/endgame-seasons.md`](behaviors/endgame-seasons.md)) is live,
  end to end: the `vanguard` table in `meld-db` (per-season best, monotonic, earliest-
  `achieved_at` tie-break) with 13-week season math (`season_at`);
  `WorldActor::post_vanguard` feeds it off **validated movement** via `DbWrite::Vanguard`
  (a write only on a new deepest tile — never on the loop, no client-submitted score);
  `GET /v1/leaderboards/vanguard[/:season|/me]` serve the live + archived boards; and the
  **Vanguard Wall** in Last City lights with the season's top names ([E] at the wall, or
  `MELD_WALL`/`?wall` for a screenshot frame). Verified by `qa/tests/vanguard_board.rs`
  (real wire + HTTP, Postgres), `meld-db`/`city` unit tests, and a native city screenshot.
  Deviations from the full designed surface — per-player rather than per-instance entries,
  integer season index, unpaginated top 100 — are tabled in
  [`interfaces/http-api/leaderboards.md`](interfaces/http-api/leaderboards.md) and close
  with `AD-6`.
- → **AD-4** (Hunt Board) at a **light first cut**: a handful of "kill X / reach depth Y /
  clear this dungeon" hunts, not the full system.

**④ Polish the feel — as important as any new system.** A slice becomes "want to play"
through feel & clarity, not more mechanics.
- [ ] **P1-2 — Combat & moment-to-moment feel pass.** Hit feedback/juice, damage/heal
  readability, turn/telegraph clarity, pacing — make the ATB *feel* good, not just be
  correct. Screenshot/video-verify (CLAUDE.md "Visual verification").
- [ ] **P1-3 — New-player onboarding & progression legibility.** The first hour: teach the
  loop, and make "am I getting stronger?" legible (gear power, level, what to do next).
- → **LC-2** (fix the reversed-walk bug — a visible rough edge new players hit).

**Definition of done (Phase 1):** a new player can — with a friend — dive, get *exciting*
loot, come home to **craft/upgrade and spend**, chase a **scoreboard + a couple of
goals**, and have the moment-to-moment **feel good**. An early-access-worthy loop standing
entirely on today's build, **before** any ecology, building, persistence, bosses, or guilds.

**Explicitly deferred (the vision — later phases):** `CR` (living ecology), `BD` (building
& sieges), `SC-3` (world persistence), `EW` (end-world bosses), `SOC` (guilds), the Shift,
`MON`. They layer on *after* Phase 1 proves the core is fun.

---

## Epic LC — Last City (the persistent hub)

The hub is named **The Last City**. This supersedes the "The Weld" working name in
[`proposals/last-city.md`](proposals/last-city.md); that proposal is otherwise the
design of record (fiction, districts, the presence/ward-sharding plan, the
additive `town.*` wire surface). M0 shipped: a walkable HD-2D plaza that closes
the dive→extract→dive loop. This epic finishes M1–M3.

- [ ] **LC-1 — Finish the hub so hundreds of players sync and interact.** Stand up
  the **town presence loop** (a separate, lighter loop from the authoritative maze
  loop — *do not touch `game.rs`'s no-locks model*, CANON §S): ward-sharded
  presence + proximity chat + emotes over the additive `town.*` messages, render
  other players' avatars in **The Commons**. See
  [`proposals/last-city.md`](proposals/last-city.md) "how a 4-player game hosts a
  city of hundreds" (M1) and its wire/HTTP surface section.
- [ ] **LC-2 — Fix the reversed walk direction.** In Last City the hero sprite
  walks *opposite* the pressed arrow (push one way → walk the other). Camera-
  relative movement sign/axis bug in the city controller (client
  [`main.rs`](../client/crates/meld-client/src/main.rs) `Screen::City` movement).
  Screenshot/verify the four directions.
- [x] **LC-3 — Adopt "The Last City" as the canonical name.** Renamed "The Weld" in
  all in-game UI/labels + client code, the proposal's name line, and added a CANON
  glossary entry (§G) for **The Last City**. District names kept.
- [ ] **LC-4 — Interact with your inventory inside town.** Open and manage the
  Vault + equipped gear + (pre-dive) loadout from within Last City — the Vault-Deep
  district UI reading the live `GET /v1/vault` / `/vault/gear`, plus equip/unequip.
  Prereq for GR-1/PT-1/PT-2/SV-1 having a home. (Depends on GR-1's slot model.)
- [ ] **LC-5 — Rebuild Last City as a friendly *authored space*.** A city is an
  authored, multi-room space you walk around in — mechanically the **friendly
  profile** of the same substrate as WG-1 dungeons (authored glyph-grid + manifest
  layout, placed interactables, a server-known space you're "in"), minus the
  hostile layer (no traps/boss/committed-path/solvability/loot). Today the city is
  **client-local** (hand-placed props, no server sim); this makes its **layout an
  authored content file** (agent-editable, the same format the DG-2 codegen
  compiles) and, when the LC-1 presence work lands, models it as a *space* on the
  shared runtime. **Note the opposite sharing model:** a city is **one persistent
  space shared by many** (LC-1's hard presence-at-scale problem, solved by SC-3
  world-sharding + the ward-sharded presence loop), *not* per-entry-fresh ≤4 like a
  dungeon — the dungeon runtime is a foundation, not a solution, for that scale.
  Factor the shared **authored-space core** out from under `meld-dungeon` when
  DG-3's space runtime lands. Design:
  [`proposals/dungeons.md`](proposals/dungeons.md) §"one authored-space substrate"
  + [`proposals/last-city.md`](proposals/last-city.md). Depends on DG-2/DG-3.

---

## Epic PT — Party & loadout management

Right now the party is fixed at dive time; players can't rearrange or save teams.

- [ ] **PT-1 — Change party rows (front / back row).** Let a player assign each
  hero to a front or back row and swap them, with the row affecting combat
  (melee reach / damage taken / target priority — pick the rule, add its
  `[TUNABLE]`, cite it in `combat-atb`). Server-authoritative; rides existing
  party/roster surface. Editable in Last City (LC-4) and on the party screen.
- [ ] **PT-2 — Save, name, and swap party loadouts in town.** Persist multiple
  named party compositions (which heroes, their equipped gear, rows) and let the
  player swap the active team before stepping through The Threshold. New
  persistent model + HTTP CRUD; surfaces in Last City. Relates to the GDD §4
  "Build Templates" idea ([`behaviors/meta-progression.md`](behaviors/meta-progression.md)).

---

## Epic GR — Gear & items (permanent, ephemeral, consumable)

The stakes engine. Some gear is insured and persists; some is loot that always
burns on death/leave; some is single-use. See
[`interfaces/data-models/gear-item-models.md`](interfaces/data-models/gear-item-models.md)
(blue/red gear, durability, gems, consumables already sketched).

- [ ] **GR-1 — Full equipment slot system.** Seven slots per hero: **Head, two
  Hands, Chest, Legs, two Accessory** slots. Extends today's single-slot
  per-character equip; server derives stat bonuses from the full set; the tabbed
  inventory UI grows to the slot grid. Add the slot enum to `meld-proto`
  (additive), the persistence, and the derivation in `meld-run::party_fighters`.
- [ ] **GR-2 — Durability & the wipe.** 🟡 *Partial:* death already degrades
  equipped Blue-Chest durability (×0.9) and returns the gear to the Vault
  ([`behaviors/run-lifecycle.md`](behaviors/run-lifecycle.md); death_durability
  test). **Remaining:** durability as a real repair sink across the full slot set
  (GR-1), max-durability loss on death, gear breaking, and the rule that **a wipe
  strips everything you didn't extract** (backpack lost; only insured Blue-Chest
  gear comes home). Ties to the crafter repair economy (MS-1) and GDD §7 "Durability Sink."
- [ ] **GR-5 — Class-locked equipment & two-handed weapons.** Every equippable item
  declares a **family** (sword/shield/spear/staff/globe/gauntlet/dagger/parry_blade)
  and every class declares which families it may wear, so classes read as classes:
  Explorer sword+shield *or* two-handed spear, Resonant staff (2H), Psyker globe (2H),
  Iron Hull gauntlet+shield, Shifter dagger with **two** legal off-hands (second dagger
  or parrying blade). A **two-handed** weapon occupies `main_hand` and reserves
  `off_hand` (409 + offer-to-unequip, never a silent stat loss). Armor uses **weight
  classes** (heavy/medium/light/robe) with a per-class allowed *set* so most drops fit
  more than one hero, **plus rare class-exclusive signature pieces** that ignore the
  weight table and carry a class-flavored keyword affix (the armor arm of `AD-1`
  uniques). Enforced server-side at equip, in derivation, and at loot generation —
  never client-side (CANON §S).
  Design: [`proposals/gear-identity.md`](proposals/gear-identity.md) §1.
  - 🟡 *Landed:* the shared legality table (`meld_proto::equipment` — families, hands,
    weights, `check_equip` naming the rule that failed), `gear.family` /
    `gear.armor_weight` columns (additive; an item with no descriptor stays
    unrestricted, so no Vault breaks), the generator rolling a class-appropriate
    family/weight per drop — and **never rolling an off-hand for a two-handed class**
    (no dead drops) — plus the client tooltip line ("staff (two-handed)", "heavy
    armor"). Enforcement is authoritative in **derivation**
    (`equipped_gear_bonuses`): illegal gear grants nothing. Nouns that contradicted
    the families were fixed (Psyker Focus Rod → Psi-Orb, Resonant Ward Scepter → Ward
    Stave, Iron Hull Warhammer → Kinetic Gauntlet).
  - **Remains:** an equip-time `409` (blocked on `GR-7` below — there is no persisted
    hero class to check against in town), the two-handed *equip UX* (reserve the
    off-hand + offer-to-unequip), greying illegal rows in the inventory grid, and
    authoring signature pieces.
- [ ] **GR-7 — Persist a hero's class per slot.** Today the party is chosen per dive and
  gear equips to a *slot*, so in town the server cannot say what class hero 2 is — which
  is why `GR-5` can only enforce at derivation. Persist a class per hero row (the
  `heroes` table already holds name + `back_row`), so a hero becomes a character rather
  than a slot. Unlocks: equip-time legality (`GR-5`), saved loadouts (`PT-2`), and
  per-hero progression later. Party choice at dive time becomes *which* heroes you take.
- [ ] **GR-6 — "Red" becomes "Ephemeral" (and says so).** Rename `Insurance::Red` →
  **`Ephemeral`** and `Blue` → **`Insured`** on the wire (serde alias keeps old
  payloads parsing) and in every player-facing string; the Blue-Chest/Red-Chest
  *fiction* stays in CANON §G but stops being the label a player must decode. Every
  gear row shows the word plus a hover / press-and-hold tooltip — Ephemeral:
  "**Vanishes when the run ends** — win or lose." A player must never lose an item
  they didn't know was temporary. Unblocks `GR-3`.
  Design: [`proposals/gear-identity.md`](proposals/gear-identity.md) §2.
  - 🟡 *Landed:* the enum rename with `blue`/`red` serde aliases, `Insurance::label()`
    / `tooltip()` as the single source of player-facing copy, the API normalizing stored
    chest colours to `insured`/`ephemeral` on the wire, and the gear tooltip showing
    **Ephemeral — "Vanishes when the run ends - win or lose."** on its own amber line
    (an unparseable word reads as Ephemeral: wrongly believing an item is safe costs the
    player the item).
  - **Remains:** the same wording in the run-loot/backpack HUD and the end-of-run
    summary, and press-and-hold on touch.
- [ ] **GR-3 — Ephemeral items/gear.** A distinct class of items (incl. Red-Chest
  gear) that **always** vanish on death *or* on voluntarily leaving Meldworld —
  they never bank to the Vault, only matter for the current dive. Model as an
  ephemeral flag on the item / backpack-only class; enforce at extraction banking
  (they don't transfer) and on run end. Contrast with insured Blue-Chest (GR-2).
- [ ] **GR-4 — Consumable healing items.** Field/battle-usable heal items that are
  **consumed on use** (decrement + destroy at zero). Wire into the existing async
  battle-injection path (GDD §6; [`behaviors/async-interaction.md`](behaviors/async-interaction.md))
  and direct self-use. Stackable in the backpack; add `[TUNABLE]` heal amounts.

---

## Epic SV — Persistence & the Safety Deposit Box

- [ ] **SV-1 — Safety Deposit Box (persistent stash) in Last City.** A guaranteed-
  persistent storage in town, separate from what you carry into the maze, so
  paying/committed players never lose gear they chose *not* to risk. New persistent
  container model + deposit/withdraw HTTP + a Last City district UI (Vault-Deep
  annex). Interacts with GR-2/GR-3: only what's in the box or extracted survives a
  wipe; anything carried in and not extracted is at risk.

---

## Epic CL — Classes

- [ ] **CL-1 — Class unlock system.** Classes become account-persistent unlocks
  rather than always-available. Ship the unlock model (which classes an account
  owns), gate party building to owned classes, and wire the two sources: **Gatekeeper
  emblem drops** (GDD §4; FS-4) and **hiring at a town vendor** (EC-2). See
  [`behaviors/meta-progression.md`](behaviors/meta-progression.md) "class unlocks
  via ClassEmblem." Existing classes (Explorer/Psyker/Resonant/Shifter/Iron Hull)
  define the taxonomy — see [`CLAUDE.md`](../CLAUDE.md) "Combat & class taxonomy."
- [ ] **CL-2 — Overworld class perks ("party sense") — deepen the system.** 🟡
  *Partial:* an overworld class-perk system already ships (`[perks]` in balance;
  `game.rs::compute_perks`) — each class's *presence* in the party grants an
  earned overworld capability that scales with the shared `run_level`: the
  **Shifter grants a corner minimap** (+ mob/portal dots, coverage grows with
  level), the **Explorer grants enemy-HP intel**, Iron Hull shrinks creature aggro
  range, Resonant grants overworld regen. **This is where overworld map-reveal and
  threat-reading belong — they're *what a class can do*, a reason to bring it, not
  universal UI.** Remaining: flesh the system out — round out perks per class
  (Psyker has none yet), tier them across run level, surface them clearly in the
  HUD, and fold it into CANON with a §/D-number. Anything giving map/threat
  *awareness in the maze* should extend this system, not bypass it. (Contrast UX-1,
  which is town-only, and UX-2, which is universal accessibility.)

---

## Epic EC — Player-driven economy & vendors

Gives chits a sink and players a market. See
[`behaviors/economy.md`](behaviors/economy.md) (stalls, contracts, escrow, taxes,
conservation invariants).

- [ ] **EC-1 — Player-to-player selling (stalls / player-led economy).** Deploy a
  Stall from the Vault, atomic taxed purchase, offline persistence, close/refund —
  end-to-end per [`behaviors/economy.md`](behaviors/economy.md) "Stall Lifecycle,"
  surfaced in Last City's Market district. All trades escrowed + atomic (no
  free-form trade window). This is the M1 economy half of Last City.
- [ ] **EC-2 — Town vendors: power goods + class hires (the chit sink).** NPC
  vendors in Last City that sell genuinely powerful things — the deliberate
  **chit sink** that makes chits worth chasing — and that **sell class unlocks**
  (you "hire" a recruit to unlock a class, feeding CL-1). Distinct from player
  stalls (EC-1): curated, always-available, chit-priced. Add vendor inventory
  config + purchase HTTP.

---

## Epic MS — Meld skills & harvesting

The persistent non-combat progression (GDD §4.1). Three skills exist and persist
XP; harvesting exists but is instant.

- [ ] **MS-1 — Finish & flesh out the Meld skills.** Bring **Forging/Smithing,
  Alchemy, and Mercantile** to real depth: recipes, gear crafting with stat
  variance, gem/materia synthesis + socketing, durability repair scaling with
  Forging level, and the mercantile tax/stall-gate effects. UIs live in Last
  City's Forge & Alembic. Spec: [`behaviors/meta-progression.md`](behaviors/meta-progression.md)
  §4.1 + [`interfaces/http-api/crafting-meld.md`](interfaces/http-api/crafting-meld.md).
- [ ] **MS-2 — Harvesting takes time in the field.** Turn instant `run.harvest`
  into a **channeled gather** (a timed action, interruptible, vulnerable while
  channeling) — tension, not a free tap. Add the channel timer `[TUNABLE]` and the
  interrupt rules; mirror the extraction-channel pattern in
  [`behaviors/run-lifecycle.md`](behaviors/run-lifecycle.md).
- [x] **MS-3 — Harvesting grants XP.** Already implemented: `run.harvest` banks the
  node's material **and** credits the node's Meld skill XP (`resource.<kind>` →
  `skill`; see [`CLAUDE.md`](../CLAUDE.md) "Harvestable resource nodes"). *Revisit
  when MS-1/MS-2 land to tune XP curves and confirm per-skill crediting.*

---

## Epic WG — World structure & generation

Make the world feel bigger, less predictable, and legibly anchored on Last City.
Spec: [`behaviors/world-generation.md`](behaviors/world-generation.md) (radial
distance model, biome bands, per-section streaming, verticality). Research +
design for this epic: [`proposals/worldgen-wg.md`](proposals/worldgen-wg.md).

- [x] **WG-1 — Dungeons.** ✅ *Full hand-designed dungeons ship.* 🟡 *Shipped as dungeon sections:* every Nth procedural
  section is now a **dungeon** — rooms divided by walls with a single door on the
  clear path (connectivity guaranteed by construction, like a biome seam), packed
  denser with creatures and ending in a **guaranteed loot chest**, all rendered by
  the normal obstacle/creature path (`meld-world`, `[worldgen] dungeon_*`, unit-tested).
  **Now building the full version** — separately-instanced, **hand-designed** (not
  procgen) dungeons: a per-biome pool of authored multi-floor set-pieces with traps,
  puzzles, a boss, and treasure; entered through a **chanced entrance** in the
  streaming overworld; a **per-entry-fresh subinstance** shared by a group of up to 4;
  a **committed space** (no Town Portal, exit-at-the-end returns to the entrance,
  death = back to town); authored via a **glyph-grid + manifest compiled with a
  solvability gate**; loot both **rolled** (scaled by distance-to-Last-City + floor
  depth) and **authored**. Full design + decisions: [`proposals/dungeons.md`](proposals/dungeons.md).
  - [x] **DG-1** — pure `meld-dungeon` crate: `DungeonDef` model, glyph-grid +
    manifest parser, the condition grammar (`all`/`any`/`not`/`seq`/`count` +
    `has_key`/`boss_dead`/`room_clear`), emitters (lever/plate/key/pedestal/boss)
    + barrier receivers (door/gate/chest), and the validator incl. the
    entrance→exit **solvability search** across the floor stack. `forest_barrow`
    sample + 16 unit tests, all green. (`spawn`/`mover`/`timer` receivers land with
    the runtime in DG-4.)
  - [x] **DG-2** — `build.rs` codegen: new `meld-dungeon-content` crate whose build
    script runs the real parser+validator (incl. the solvability gate) over every
    `content/**/*.dungeon.toml` — **a malformed or unsolvable dungeon is a compile
    error** — and embeds the validated defs as a `&'static` registry (`all()` /
    `for_biome()` / `by_name()`). Content pool: `verdant_barrow` + `guardia_forest`
    (forest), `sunken_vault` + `world_of_ruin` (desert), `ocean_palace` (mire) —
    `guardia_forest` is a compact 2-floor forest maze; `ocean_palace` a 4-floor
    Chrono-Trigger Ocean Palace recreation; **`world_of_ruin`** a LARGE (~20 min) FF6
    World-of-Ruin archipelago — **playtime comes from mandatory combat + gated
    backtracking**: six dragon bosses each guard a switch (all six open the tower
    bridge), then a three-boss tower gauntlet behind boss-gated doors ends at Kefka +
    the vault (9 bosses, 13 treasures). All authored purely in the glyph grid, gate-
    verified solvable, rendered by the DG-6b re-skin. Tests green; gate-failure verified.
  - [x] **DG-3** — runtime subinstance. ✅ *DG-3a (the pure engine) shipped:*
    `meld-dungeon-run` — the `Location` model, a live `DungeonInstance` (barrier/
    emitter puzzle state that opens doors/gates as the group solves them, stairs
    between floors, end-exit detection, the committed-space rule, and the
    per-floor `effective_distance` difficulty stamp), and **seeded entrance
    placement** from the biome pool (`roll_entrance`). Pure + deterministic; 14
    unit tests + doctest. **DG-3b (the `game.rs` wiring, on the merged SC-3
    `WorldActor`) — in progress, staged like SC-3 was:**
    - *(1/n) — entrances appear ✅* Each non-tutorial section (the initial chain +
      streamed ones, via a high-water mark) rolls a chanced entrance from its biome
      pool (`dungeon_spawn_chance`) on the clear path; streamed to clients as
      `entrance:<dungeon>`. (`place_entrance` in `meld-dungeon-run`, tested.)
    - *(2/n) — enter / move / exit ✅* The `WorldActor` owns per-player `Location` +
      live `DungeonInstance`s. A **deliberate** `run.enter_dungeon` (new C2S; never
      automatic on walking past) descends: the dungeon is stamped at the entry's
      distance and the avatar frozen at the entry spot. Inside, movement routes
      through the dungeon (slide + wall collision via `try_move`), reaching a
      lever/plate/key/boss **auto-opens** its gated doors, **stairs** move between
      floors, and the **end-exit** returns you to the overworld exactly where you
      entered. Town Portal is rejected while `InDungeon` (committed space). The
      in-dungeon snapshot is scoped to the floor (crude render — walls/doors as
      obstacles, exit as portal, chest/boss tags — pending DG-6b). New qa test
      `dungeon_enter` drives it end-to-end; core-loop qa stays green.
    - *(3/n) group entry ✅* — descending via `run.enter_dungeon` pulls in every
      teammate gathered at the entrance (within `[ai] join_radius`) into the same
      fresh subinstance — a co-op group of up to 4 enters together. qa
      `dungeon_group_enter` (two bots, one enter, both inside).
    - *(3/n) trap damage + death ✅* — stepping onto an armed trap fires it
      (`spring_trap`), dealing `dungeon_trap_damage` scaled by the floor's stamped
      distance to the party; a wipe ends the run in death exactly like an overworld
      death (`run.member_result died`, backpack forfeited, durability sink), routed
      through the existing `release_from_run`. qa `dungeon_trap_death`.
    - *(3/n) dungeon combat ✅* — entering the boss's cell starts a boss fight: the
      authored boss (scaled to the dungeon's stamped distance, FS-4 named-boss
      mechanics via its sprite/`boss_kind`) vs the party, through the existing ATB
      battle engine. On **victory** the boss is marked dead (`boss_dead` — unlocking
      its gated chest) and survivors return to the dungeon; on **defeat** the run
      dies. Dungeon fixups run *after* the shared `handle_battle_end` (guarded by a
      `BattleSlot.dungeon` tag — overworld battles byte-identical). qa
      `dungeon_boss_battle` (descend → cross floors → kill the boss).
    - *(3/n) chest loot ✅* — `run.open_chest` on a dungeon chest (`dchest-<id>`)
      banks `resolve_chest`'s reward into the run backpack (rolled material/chits/gear
      at the dungeon's stamped distance + authored contents), gated on the chest's
      `when` (the `boss_dead` vault unlocks on the kill) and openable once.
      `DungeonInstance::chest_openable`/`open_chest` unit-tested.
    - *(client) DG-6b — entrances + secluded-space interior render ✅* — the client
      recognizes `entrance:<dungeon>` and draws a distinct **glowing violet
      stone-archway doorway** (vs the exit portal's blue), fixing the bug where
      unknown tags fell through to a player avatar; **walk into one to descend**
      (collision-based, like harvesting — client-side, so bots are never pulled in;
      `F` is a fallback) via `run.enter_dungeon`. Inside, a new **`world.dungeon_scene`** cue (theme +
      bounds, emitted on descent/floor-change/exit) re-skins the whole environment as
      a **secluded, themed space** — a `forest` dungeon renders as a Guardia-Forest
      canopy: interior maze walls become **low foliage** (so the hero stays visible),
      the play area is ringed by a deep collision-free forest bowl (low rim → towering
      backdrop) that fills the frame so no overworld shows even zoomed out, the sky
      dims, and overworld terraces/cliffs are hidden underground. Non-forest themes
      keep tinted stone masonry. Native-screenshot verified. DG-7 (CANON D25 +
      `behaviors/dungeons.md`) ✅ merged. With 3/n + the client render done, dungeons
      are **complete end-to-end** (co-op entry, traverse, puzzles, stairs, traps,
      death, boss combat, loot, exit, rendered) — WG-1 ticks.
  - [ ] **DG-4** — traps + puzzles live. 🟡 *DG-4a (the engine) shipped:* the
    puzzle emitter/barrier runtime already lives in `meld-dungeon-run` (DG-3a —
    reaching a lever/plate/key/boss opens the doors/gates whose condition holds),
    and DG-4a adds the **trap state machine** (armed→disarmed) with `spring_trap`
    (fires on contact, severity rides the stamped distance) and `attempt_disarm`
    (the **Dex check the Shifter is far better at**; failure springs it;
    non-disarmable traps must be routed around). Pure, 7 tests. **Remaining —
    DG-4b:** the `spawn`/`mover`/`timer` receivers (need model additions), the
    `run.interact` wire message, and applying trap hits / interact dispatch in the
    loop — with DG-3b.
  - [x] **DG-5** — loot: `DungeonInstance::resolve_chest` turns a chest's
    `ChestLoot` into a reward scaled by the floor's `effective_distance` (deeper =
    richer, off the *stamped* distance not local position) — **rolled** (reuses
    `roll_creature_loot`), **authored** (fixed designer contents), and **hybrid**
    (guaranteed + rolled). Pure, in `meld-dungeon-run`; tunables (richness / rarity)
    are driver params so no `balance.toml` churn. 5 tests. (Wiring the reward into
    the run backpack is part of DG-3b.)
  - [x] **DG-6** — client rendering. ✅ *DG-6a (visualizer) shipped:* `meld-dungeon-viz`
    renders any `DungeonDef` to a top-down **SVG** (walls, entrance/exit, stairs,
    traps, levers/plates, doors/gates, keys, boss, treasure, legend) — see an
    authored dungeon without running the game; `dungeon-preview` bin dumps the whole
    pool. The reference the in-game view matches. *DG-6b (live Bevy render) shipped:*
    the entrance archway billboard + the in-dungeon stone/timber masonry render, both
    native-screenshot verified.
  - [x] **DG-7** — spec: **CANON D25** (dungeons — hand-authored committed
    sub-spaces) + `behaviors/dungeons.md` (full observable-behavior spec) + the
    `run.enter_dungeon` interface entry.
- [x] **WG-2 — Random starting biome (except the first run).** Every dive now starts
  in a random biome, *except* an account's very first dive — the gentle Forest-first
  onboarding (fixed biome order + centred area-0), gated on the persistent
  `players.has_dived` flag. `meld-world::section_biome` + `meld-server` `form_run`.
  (Hades/RoR2 model — see [`proposals/worldgen-wg.md`](proposals/worldgen-wg.md).)
- [x] **WG-3 — Randomized biome ordering.** The biome *theme* order is now drawn per
  run from the run seed (uniform per section, no adjacent repeat) while difficulty stays
  a pure function of `distance` — biomes are difficulty-neutral skins (creatures scale via
  `stat_mult`). `meld-world::section_biome`, unit-tested for determinism + variety.
- [x] **WG-4 — Radial spread with Last City always to the west.** The world opens
  outward across ~**340°** and **Last City sits just to the west**, marked by a
  **castle wall + gate** with the city skyline behind it; cross the western border
  and you step **right back into the city** — the one permanent, safe anchor in a
  world that worsens in every other direction. Establishes the city↔maze boundary
  (ties LC-1's presence loop to the maze exit; reframes the Threshold entry).
  **Shipped (screenshot-verified):**
  - **Radial + infinite:** the generated corridor is bent into a ~340° arc around
    the hub (`radialize`: corridor `x` → radius so difficulty is unchanged, lateral
    `y` → angle), and `ensure_frontier` streams new content rings endlessly outward
    keyed off the player's radius (`stream_radial_section`) — genuinely infinite AND
    monotonically harder outward (difficulty stays a pure function of `distance`).
    Unit-tested (`wg4_radial_world_streams_endlessly_outward`). Ground plane follows
    the player so the endless fan always has ground underfoot.
  - **Western anchor:** crossing `west_return_border` (now −20, a deliberate walk
    west across open ground, not an accidental step) returns you to Last City as an
    **instant free extraction home** — you **keep your backpack** (banked to the
    Vault), no channel, no death penalty, no item cost.
  - **The wall + gate:** a real Kenney castle wall (Pirate-Kit stone segments +
    gatehouse + towers/pennants) spans the border, with a **city skyline behind it**
    (towers + crypt buildings) glimpsed through the open gate — so the boundary is
    visible before you cross it. All GLBs sit at `y=0` (no floating).
  - **Biome presentation matches the section:** ground texture + HUD label are keyed
    off the ACTUAL per-section biome (streamed on `TerrainSection.biome`, radial LUT
    in `ground_biome.wgsl`), cross-fading at real section boundaries; each biome has
    its own fill density/props + edge taper (forest→open desert→Ashfall rock).
  **Authored climbable mountains + summit rewards:** the procedural cliff-mesas were
  flattened (they read as an accidental corridor), so verticality is now delivered by
  *intentional* landmark peaks — smooth raised-cosine domes summed into the terrain
  (`meld_proto::terrain::peak_height`, mirrored in `ground_biome.wgsl`), kept under the
  walkable-slope aspect (`PEAK_MAX_ASPECT`) so you climb them (zero collision cost). A
  section's clear-path crest raises one (`path_climb_chance`, out past `peak_min_distance`)
  and CROWNS its summit with a reward — a gate-boss (`peak_boss_chance`) or a guaranteed
  treasure chest. Peaks bend with the fan alongside the chests/monsters and ride the wire
  on `run.started.peaks` + `TerrainSection.peaks`, so ground + entity Y raise together and
  the reward sits on top. Screenshot-verified (a Lv-boss atop a desert mountain).
  `[worldgen]`: `peak_radius`, `peak_min_distance`, `path_climb_chance`, `peak_boss_chance`.
  **Remaining (minor cosmetic):** re-homing biome-seam walls into the radial layout
  (see `proposals/worldgen-wg.md` "Known cosmetic follow-up").
  See [`proposals/worldgen-wg.md`](proposals/worldgen-wg.md); fold into
  [`behaviors/world-generation.md`](behaviors/world-generation.md) when built.
- [ ] **WG-5 — Mountains as a content pillar (the "new dungeon").** 🟡 *Backlog.* WG-4
  shipped authored climbable mountains as **landmarks** — a raised dome with a single
  boss/chest on the summit. The bigger idea: promote a mountain into a **destination
  with its own content**, the open-air sibling of a WG-1 dungeon. A climb becomes a
  multi-stage ascent (switchback route, mid-slope guardians/elites, environmental
  hazards, a real summit encounter + reward tier that scales with the peak's distance),
  a **committed space** you enter and clear rather than a prop you walk over. Likely
  reuses the DG authored-space substrate (glyph-grid/manifest for the climb route +
  placed encounters) projected onto the terrain dome, minus the enclosed walls. Design
  TBD — fold into [`proposals/verticality.md`](proposals/verticality.md) +
  [`proposals/dungeons.md`](proposals/dungeons.md) when picked up. Depends on the DG
  space runtime (DG-3) for the encounter/space model.

---

## Epic FS — Field survival & environment

Make time in the field a living, dangerous place worth screenshotting.

- [ ] **FS-1 — Camping in the field.** An item or mechanic to make a temporary
  safe rest in the maze (heal/regroup/pass time, with risk — think
  Warding-Tent/Sanctuary-Campfire family from GDD §5, generalized to a solo rest).
  Define what camping restores and how it can be interrupted.
- [ ] **FS-2 — Weather that does something, per biome.** Weather should have
  **mechanical** effects in the field (visibility, movement, encounter/harvest
  modifiers, elemental interactions) and be **biome-appropriate** — deserts should
  rarely rain; each biome gets its own weather table. Seeded + server-authoritative
  so it's fair. New `[worldgen]`/`[weather]` tunables.
- [ ] **FS-3 — Richer environmental effects (and they emit light).** Expand ambient
  HD-2D life like the **night fireflies**, and make such effects **light sources**
  (the fireflies should actually emit light), plus more per-biome/per-time-of-day
  flourishes. Client HD-2D pass — see the HD-2D pipeline notes; verify by native
  screenshot at night.
- [ ] **FS-4 — Gatekeepers & unique bosses.** 🟡 *Shipped Elites + Gatekeepers:* the
  (previously dormant) `EncounterClass` pipeline is now live — a fraction of creatures
  roll **Elite** champions (tougher, ~3× loot) and a **Gatekeeper** boss guards every
  biome-border pass (a wall of HP on the door, unavoidable, with a fat guaranteed
  reward). Every champion also rolls an **affix** (Swift / Brutal / Armored / Giant /
  Vicious) that twists the fight and rides its battle name, so no two feel the same.
  Stats + loot scale via `[encounters]`; the client sizes + tints them distinctly;
  the merge cap already differs for gatekeepers. `meld-world`
  (`promote` + placement), `meld-server` (loot spike), unit-tested. **Remaining:**
  unique *boss mechanics* (special attack patterns, not just big stats), **class-emblem**
  drops feeding CL-1, party-scaled HP over a full merge, and a proper boss arena.
  See [`behaviors/combat-atb.md`](behaviors/combat-atb.md) battle merge.
- [ ] **FS-5 — Day/night cycle as a first-class system.** A seeded, server-
  authoritative time-of-day clock that other systems read: it drives the fireflies
  and night lighting (FS-3), gates creature sleep/activity (CR-3), and modulates
  weather and encounter tables (FS-2). One source of truth for "what time is it in
  this instance," on the wire so every client agrees.

---

## Epic CR — Creatures & the living world

Make the overworld feel inhabited, not decorated. Creatures already roam, belong
to **factions**, take real damage in hostile-faction skirmishes (their `hp/max_hp`
is a live bar), and leash to their spawn / stop roaming when `in_battle` — see
`meld-world::Arena::step_creatures` / `MonsterSpawn`. This epic builds the ecology
on top. **Hard constraint (the user's, and correct): keep it tightly instanced and
budgeted so the creature sim never threatens the single-owner loop or the server**
— see CR-4.

> **Design doc:** [`proposals/living-ecology.md`](proposals/living-ecology.md) now
> specs this whole epic end-to-end — the CR-4 sim budget (LOD/caps/determinism), turf
> wars + ground loot (CR-2), diets/needs/sleep (CR-3 + FS-5), flora growth, breeding &
> growth stages, herds/alphas/swarms/splitting, materials-for-crafting (MS-1), and the
> bestiary (CR-5) — with a phased build plan (E0 first) and CANON deltas. Boxes stay
> unchecked until code lands.
>
> **Build order & the `SC-3` dependency (doc §J).** Most of the ecology (E0–E2, E5)
> pays off **within a single run** and builds on today's ephemeral world **now**
> (E2 needs **FS-5** first). The *durable* payoff — the trophic **cascade** where an
> over-farmed region stays thin **across sessions** (doc §I) — is gated on **SC-3**
> world persistence: the ecology overlay is the writer of SC-3's *"population diffs"*
> seed-delta line. Deterministic seed-gen (shipping) is **not** persistence; the
> cascade needs SC-3's event log. Build the sim on the precursor, wire persistence when
> SC-3 lands (no sim rework). A **wiped region always recovers** via a colonization
> trickle seeded from the biome table (local extinction possible; global impossible).

- [ ] **CR-1 — Per-creature distance modifiers + deep-biome palette & rarity.**
  Beyond the global `stat_mult(d)`, give each creature its own distance-scaled
  modifier table, so pushing *further out than usual* meaningfully changes what you
  face. Signal it visually: deeper/harder zones get a **randomized, shifted color
  palette** so a dangerous variant reads at a glance, and those tougher creatures
  drop **higher-rarity** gear (GR) and collectables (CR-5). Loot rarity scales with
  distance. *Accessibility: the palette is a bonus cue, never the only one — pair it
  with a redundant non-color signal (level tag / nameplate / icon), see UX-2.*
- [ ] **CR-2 — Creatures fight each other, visibly, with consequences.** 🟡
  *Partial:* hostile factions already skirmish and lose `hp`. **Remaining:** show
  the **fighting state on the map** (so you can read "those two are clashing"),
  make skirmish **deaths drop loot** on the overworld (pickup per
  [`behaviors/async-interaction.md`](behaviors/async-interaction.md)), **persist
  damage** to the creature, and have it **slowly regenerate** as it roams (so a
  wounded creature is a real, time-bound opportunity). Add regen + on-map combat
  state to `MonsterSpawn`/`step_creatures`; tunables in `[worldgen]`/`[ai]`.
- [ ] **CR-3 — Living ecology: diets, needs, and breeding.** Creatures have a
  **diet class — carnivore / omnivore / herbivore** — that drives behavior: they
  eat (hunt prey / graze nodes), sleep (tied to FS-5 day/night), and **breed**,
  spawning more of their kind in an area **up to a hard cap**. Predator/prey
  pressure keeps populations dynamic instead of static. Everything is
  server-authoritative and seeded. **Must respect the CR-4 budget** — population
  caps and per-area instancing are load-bearing, not polish.
- [ ] **CR-4 — Ecology simulation budget & instancing (the guardrail).** Before
  CR-2/CR-3 ship, define the perf envelope: creature sim stays **per-area /
  per-instance**, hard population caps, a bounded tick cost, and it must **never**
  block or contend with the authoritative maze loop (CANON §S — one task owns
  ephemeral state, no locks; memory: game-loop-perf). Simulate only near active
  players; freeze/serialize distant areas. This item is the explicit answer to
  "keep it highly instanced so we don't crash servers." Add a QA load test.
- [ ] **CR-5 — Bestiary / codex & collectables.** A persistent, account-level
  record of creatures encountered/killed and **collectables** dropped by rarer/
  deeper creatures (CR-1) — discovery as its own progression and completionist hook,
  and a natural home for the "higher-rarity collectables" the loot scaling produces.
  New persistent model + HTTP; surfaces in Last City.

---

## Epic BD — Building, towns & the anchor-defense loop

Players **harvest** wood + stone, **build** structures, cluster them into **towns**,
and plant **anchors** that pin ground against the Shift **while defended** — the
"sim / world-builder / desperate roguelite" pillar (CANON §W). Graduates the canon
foundation that already exists: the one `Structure` primitive + its `function` tag
(**D21 / §W3**), the `Structure` model, and the world model that persists it (§W5).
**Discipline (§W3):** one primitive, many functions — *do not build towns, anchors,
portals, camps as separate systems.* **Shares the `CR-4` sim budget** (a siege is the
same always-running-when-unwatched spatial workload as the ecology).

> **Design doc:** [`proposals/building-and-sieges.md`](proposals/building-and-sieges.md)
> — harvest→structures→towns→anchors→siege end-to-end, plus a **builder mode** (the
> construction UX — BD-9), **building up** (buildable verticality extending D24 — BD-10),
> and **hireable NPC garrisons** that defend while owners are offline (BD-11, the
> mechanical layer under AX-3's smart agents). Phased plan (BD-0 first) + CANON deltas.
> **The `SC-3` gate (doc §L):** this is the most persistence-dependent epic — a town
> that dies at instance-close is pointless. Harvest (BD-1) + a within-run camp ship on
> the precursor; **anchors + real towns need `SC-3` world persistence** (structures are
> *the* content of the §W5 event log). Build the sim against the WorldActor now; wire
> persistence when SC-3 lands (no sim rework).

- [ ] **BD-0 — Siege/build sim inside the `CR-4` budget (guardrail).** Prove structures
  are entities the ecology LOD/interest-index/freeze model covers and the siege step
  fits the existing per-tick ceiling with **no new budget**. Build with `CR-4`/`E0` as
  one budget effort.
- [ ] **BD-1 — Harvest wood & stone.** Wood from ecology `Flora` trees (CR); new
  `MineralNode`s (stone/ore/clay) + timed `MS-2` harvest + structural-material tables.
  *Ships as gathering on the precursor.*
- [ ] **BD-2 — The `Structure` primitive: place → build → HP → repair → demolish.**
  One entity, `function` tag, server-validated placement, material cost, build progress,
  upgrade tiers. *Within-run camp (FS-1) is the precursor taste; real towns need SC-3.*
- [ ] **BD-3 — Anchors & the Shift-pin loop.** Anchor pins its region (`pin_radius`)
  against the Shift (D20) while HP > 0; defend or lose it (§W5 `suppressed_by`). **The
  headline loop; needs SC-3 + the Shift.**
- [ ] **BD-4 — Walls, gates, towers & the siege.** Creatures path to and attack
  structures (extends `CR-2`); walls/gates soak; towers auto-defend; repair races
  attrition; always-running-when-unwatched freeze + catch-up (shared `CR-4`).
- [ ] **BD-5 — Towns: composition, guild ownership, forward-town stops.** A town = a
  cluster of the primitive; **guild-owned** structures + permissions (**SOC**); forward
  towns sustain Run Level across a deep push (§W4); `portal` = plantable extraction
  (evolves D15).
- [ ] **BD-6 — Field crafting & storage.** `stash` (siege-able field storage),
  `workshop` (`MS-1` Forge/Alembic in the field), `hearth` (respawn/rally aura).
- [ ] **BD-7 — Persistence wiring (rides `SC-3`).** Structures / anchor-altered Shifts /
  harvest state into the §W5 event log; hibernate/reload; **season GC**. No sim rework.
- [ ] **BD-8 — Sieges at scale & world bosses.** Mega-siege bounded by the realm cap;
  world-boss town sieges; **AX-3** agent garrisons hold towns while owners are offline.
  Endgame.
- [ ] **BD-9 — Builder mode (the construction UX).** The client build sub-mode over the
  overworld: a **palette** (greyed by affordability), a **ghost** that snaps to grid /
  adjacency / level, **rotate** + **level-select**, confirm → `run.build` intent,
  **server validation with reasons**, and an edit/upgrade/repair/demolish sub-mode
  (permission-gated for guild builds). **Companion to BD-2** — you can't build without
  it; build them together. Client UX + intents; ships on the precursor.
- [ ] **BD-10 — Building up (buildable verticality).** `floor`/`platform`, `pillar`/
  `stilt` support + buildable `stair`/`ladder`/`ramp` connectors; a **support rule** (no
  floating floors) + `max_build_level`; collapse-on-support-loss; verticality as a
  **defensive advantage** (creatures can't free-climb, must breach the base). **Extends
  CANON D24** — same integer-level axis, no-free-climbing preserved. Follows BD-2/BD-4.
- [ ] **BD-11 — NPC garrison hire (defend while offline).** `barracks` + a hire vendor
  (**EC-2**/**CL-1**); `GarrisonUnit` tiers that **patrol + fight the siege on the shared
  `CR-4` budget** (`garrison_cap`) **while owners are offline**; **upkeep** (new economy
  sink) + permanent loss on death; guild towns pay from the guild vault (**SOC**).
  **AX-3** is the smart-agent controller for the same unit. Follows BD-4.

---

## Epic EW — End-world bosses & the true end (Ometus)

The keystone the whole loop points at (CANON §W's "seasonal push to a far end-world
boss"). **Three known end-world bosses — Termina** (machine-devil), **Nestiph**
(rebirth-goddess), **Slake** (desire-demon) — and defeating all three unlocks the
**true end boss, Ometus**, the forgotten evil behind every Shift. Two **hidden** bosses
give the non-combat personas their own apex: **All-Father** (mountain-slime origin — the
Gatherer/ecology endgame) and **Terim** (god of crafting & building — the Builder/Crafter
endgame). Apex of `FS-4`'s "unique boss mechanics"; the season's climax; the demand
spike that makes the whole economy cohere.

> **Design docs:** [`proposals/endgame-bosses.md`](proposals/endgame-bosses.md) (the
> roster, mechanics, unlock ladder, scale, seasons, and the lore reconciliation) and
> [`proposals/core-loop-and-personas.md`](proposals/core-loop-and-personas.md) (why
> every persona's output feeds this, and how the hidden bosses close the loop).
> **Lore note:** the bestiary already seeds **Nestiph** (the Chitin-Kilns "Nestiphian
> Cradle") and **Ometus** — a call is needed on Slake inheriting Ometus's desire domain
> so Ometus can be elevated to the meta-antagonist (see the doc's reconciliation).

- [ ] **EW-0 — Boss framework (extends `FS-4`).** `WorldBoss` defs, raid-scale merge cap,
  the three-boss unlock gate on `World`, the arena hook. The apex of FS-4's unique-boss
  work.
- [ ] **EW-1 — Termina** (Seized Engine/Brass Corpse arena; machine/rail/reassembly).
- [ ] **EW-2 — Nestiph** (Chitin-Kilns arena; reanimation/spore-mind-control/rebirth).
- [ ] **EW-3 — Slake** (Hearth-Plains/Lotus-Engine arena; temptation/gluttony/will-save).
- [ ] **EW-4 — The unlock gate + Ometus.** All-three → the path opens; **Ometus** the true
  end boss + its **Shift consequence** (quiet the Shift for the season? — doc §Open).
- [ ] **EW-5 — Hidden: All-Father.** Ecology-discovery unlock (CR migration/swarm); the
  Gatherer apex + rewards (rare mats, a slime companion, CR-5 completion).
- [ ] **EW-6 — Hidden: Terim.** Craft/build-mastery unlock (MS/BD); the Builder-Crafter
  apex + legendary recipes / blueprints / maker's-marks.
- [ ] **EW-7 — Seasonal wiring.** Ladder reset per season, Vanguard boss-kill lines,
  first-clear titles/prestige. Rides seasons (D8) + §W5 GC (needs `SC-3`).

---

## Epic AD — Adventure depth (gear, affixes, synergy & the chase)

The Adventurer's retention layers — what turns "a working dive" into "one more dive."
**Builds come from party composition + gear/affixes + synergies, NOT stat/talent trees**
(a player runs *four* heroes — four talent pages is the wrong depth; attributes stay
auto-gained). A **team-composition ARPG.** The affixed-gear chase is also the connective
tissue between personas (adventurers chase it, crafters roll it via `MS-1`, merchants
trade it, Terim drops the recipes). **Not gated on `SC-3`** — account/run-level, ships on
the current build.

> **Design doc:** [`proposals/adventure-depth.md`](proposals/adventure-depth.md) — the
> affix system, party synergies, elemental affinities, the Hunt Board, keystone
> modifiers, and the leaderboard suite. Refines the Adventurer section of
> [`proposals/core-loop-and-personas.md`](proposals/core-loop-and-personas.md) (which
> had overstated the persona as "whole").

- [ ] **AD-1 — Gear affixes & the loot chase (the star).** Server-rolled affixes in three
  classes — **stat / keyword (twist a class mechanic) / synergy (reference allies)** — from
  distance-banded tiered pools; **uniques** (build-defining + a tradeoff) and **sets**
  (party-wide bonuses). Extends `GR-1` + gear-item-models; rolled/rerolled by crafting
  (`MS-1`). *Highest-leverage item — it's what "crazy grinding" runs on.* Past a
  `[TUNABLE]` tier floor, drops roll **qualities, not magnitudes** — damage types
  (`AD-3`, riding the dormant `gear.damage_modifiers` column), on-hit statuses reusing
  states the ATB already models (Barrier/Regen/Evasion/gauge-drain), keyword affixes
  that twist a class mechanic, and synergy affixes that reference allies (→ party
  builds, `AD-2`). Early bands stay legible for new players (`P1-3`); builds bloom deep.
  Design: [`proposals/gear-identity.md`](proposals/gear-identity.md) §3.
- [ ] **AD-2 — Party synergies + surfacing.** Class-pair + affix-driven synergies; the
  party screen shows **active synergies** (the build feedback loop). Depends on AD-1 + `PT-1`.
- [ ] **AD-3 — Elemental affinities & resistances.** Damage-type weak/resist/immune on
  creatures/biomes; resist/convert affixes; **telegraphed** (`UX-2`). Makes biomes a
  combat *decision*. Extends [`behaviors/combat-atb.md`](behaviors/combat-atb.md).
- [ ] **AD-4 — The Hunt Board.** Directed combat goals (named creatures/dungeons/depth) —
  the mid-game spine; ties `CR-5` bestiary, `FS-4`, `DG`; co-op/guild hunts (`SOC`).
- [ ] **AD-5 — Keystone modifiers.** Opt-in challenge scaling for better loot; seeds from
  `FS-4` champion affixes; feeds the keystone leaderboard.
- [ ] **AD-6 — Leaderboard suite.** Generalize the Vanguard board into **boss / keystone /
  hunt / guild** boards (seasonal, titles/cosmetics). Extends
  [`behaviors/endgame-seasons.md`](behaviors/endgame-seasons.md).

---

## Epic SOC — Multiplayer: parties & guilds

> **Terminology:** in this codebase **"party"** already means one player's team of
> up to four *heroes* (mixed classes). The systems below are about grouping
> *players* — so this doc calls them **"co-op groups"** and **"guilds."** Don't
> overload "party." Today, players form up through an ephemeral **co-op lobby**
> (join code, `run.join_battle`, the Threshold) — `meld-server::game.rs` `Lobby` /
> `LobbyMember`. These items make grouping durable and social.

> **Design doc:** [`proposals/parties-and-guilds.md`](proposals/parties-and-guilds.md)
> now specs both items end-to-end (models, HTTP + realtime surface, tunables, phased
> build plan, CANON deltas). The boxes stay unchecked until code lands.

- [ ] **SOC-1 — Co-op group system.** A real, managed player group that outlives a
  single dive: invite/accept, a named roster, group presence in Last City, dive
  together into one instance, and stay grouped across runs — built on the existing
  lobby rather than replacing it. Clarify how a group maps onto the 4-player
  instance cap and the expandable-party raid merge (GDD §5;
  [`behaviors/combat-atb.md`](behaviors/combat-atb.md)).
  - Design: [`proposals/parties-and-guilds.md`](proposals/parties-and-guilds.md)
    Part A — Phase 1 (group ≤4 = one instance) is the first ship; raid groups
    (5–16 via merge) are Phase 2.
- [ ] **SOC-2 — Guild system.** Persistent player organizations: membership +
  roles, a guild identity/tag, and a home in Last City. Later hooks (scope as it
  firms up): shared guild bank/stash (relates to SV-1), guild bounties (EC/economy),
  and a guild line on the Vanguard board
  ([`behaviors/endgame-seasons.md`](behaviors/endgame-seasons.md)). New persistent
  models + HTTP; fold into CANON when the design hardens.
  - Design: [`proposals/parties-and-guilds.md`](proposals/parties-and-guilds.md)
    Parts B–D — the Charterhouse (registration), ranks/permissions, guild vault +
    immutable audit log, composed-heraldry flags, and guild chat.

---

## Epic UX — Universal interface (town nav & accessibility)

Small but high-leverage interface work — the parts that must work for **everyone,
regardless of party**. Note the deliberate split from classes: **map/threat
awareness *in the maze* is a class perk (CL-2), not universal UI.** These items are
only the things that can't be class-gated.

- [ ] **UX-1 — Last City minimap & compass (town-only).** A minimap and compass
  **for Last City itself** so players can navigate the hub — locate the districts
  (Vault-Deep, Market, Forge/Alembic, Bounty Board, Drill Yard, Vanguard Wall),
  the Threshold gate out, vendors, and other players. Universal (the city is safe,
  social, and shared — nothing to gate). Distinct from the maze minimap, which is
  the Shifter's overworld perk (CL-2). Client UX over the Last City scene
  ([`proposals/last-city.md`](proposals/last-city.md)).
- [ ] **UX-2 — Accessibility & non-color legibility.** Danger and state must never
  depend on **color alone** — CR-1's deep-biome palette shift is a *bonus* cue, so
  pair it with universally-available redundant signals (creature level tags,
  nameplates, threat icons) and a colorblind-safe palette option. Baseline
  readability for all players; the *richer* HP/threat intel on top of this is the
  Explorer's class perk (CL-2). Bake this in while the difficulty-signaling systems
  (CR-1) are being built, not as a retrofit.

---

## Epic MON — Monetization

Revenue features. **Design guardrail:** keep the competitive core (the Vanguard
board, extract-or-die stakes, the player economy) fair — lean toward
convenience/persistence/cosmetic value over raw power, and be explicit in each
item about where it sits on the pay-for-power line, since that's the retention
risk. These are the owner's calls to make; this epic just tracks them honestly.

- [ ] **MON-1 — Subscription-gated Vault.** Put the persistent **Vault** (chits,
  materials, gear storage — [`behaviors/economy.md`](behaviors/economy.md),
  [`interfaces/data-models/`](interfaces/data-models/)) behind a subscription.
  Decide the free-tier fallback carefully — what happens to a lapsed subscriber's
  banked items, and how this interacts with the Safety Deposit Box (SV-1) and the
  death/extract loop (a player who can't bank has no extract-or-die tension). Needs
  a billing/entitlement layer + entitlement checks on the Vault HTTP surface.
- [ ] **MON-2 — Private persistent instance for you + your guild (premium tier).**
  A higher tier gives a player and their **guild** (SOC-2) their *own* instance of
  Meldworld, which unlocks things the shared ephemeral world can't:
  - **Pinned seeds** — reuse a fixed `run_seed` so the world *doesn't* reshuffle
    every session (the opposite of WG-2/WG-3's per-run randomization). Technically
    cheap: world gen is already fully deterministic from the seed
    (`section_seed(run_seed, n)`), so a persistent instance just fixes the seed.
  - **Buildable camps** — set up / build persistent camps in the field (generalizes
    FS-1 camping), which **creatures may attack and try to destroy** (ties to the
    ecology, CR-2/CR-3, and the ward/tent family in GDD §5). This is the big
    architectural lift: today a `MazeInstance` is **ephemeral, discarded on close**
    (CANON §S); a persistent instance keeps mutable world state across sessions —
    new persistence + lifecycle, kept off the authoritative maze tick.
  - **Better performance** — a dedicated/less-crowded instance for the paying group.
    Reconcile with the CR-4 sim budget and the single-owner loop model.

  Scope this as its own design doc before building — it touches guilds, world-gen
  determinism, ecology, and instance lifecycle at once.

---

## Epic SC — Server scale & simulation capacity

Lift the authoritative server's concurrency ceiling *without* breaking the
single-owner/no-locks loop (CANON §S). Full plan + a forward-compat analysis of
overworld hazards and sieged player towns in
[`proposals/server-scaling.md`](proposals/server-scaling.md). Today: no admission
cap, one task drives one global `MazeInstance`; the binding cost is the per-client
overworld snapshot (**O(sessions × entities)** every tick), not the world sim.
Directly underpins CR-4 (sim budget), MON-2 (persistent camps/instances), and LC-1
(hundreds synced in the hub).

- [ ] **SC-1 — Interest index for the snapshot (chunk grid).** 🟡 *Core landed:* the
  per-player linear entity scan in `meld-server::snapshot_msgs` now runs off a
  per-tick chunk grid (cell = `chunk_size`), turning **O(sessions × E)** →
  **O(sessions × visible)** — behaviour-identical to the old scan (proven by a
  300-trial equivalence unit test vs. a naive oracle + an always-include invariant
  test), reusing the `HashMap<(i32,i32), Vec<_>>` pattern from
  `Arena::step_creatures_with_aggro`. Also the broadphase overworld projectiles/traps
  (FS) + dungeon traps (DG-3/DG-4) reuse. **Remaining:** a per-chunk serialize cache
  (second-order — dedup identical chunk bytes across viewers) and a QA bot-ramp load
  test to quantify the win.
- [ ] **SC-2 — Sim/IO split (in-process).** The instance task publishes an
  immutable `Arc<WorldSnapshot>` per tick; a worker pool does cull + serialize +
  send in parallel across cores. Decouples sim cadence from snapshot cadence
  (enables sub-stepped projectiles). Single-owner invariant preserved — workers
  only read a frozen copy.
- [ ] **SC-3 — World sharding (`Router` + `WorldActor`).** Split `GameState` into a
  routing/matchmaking supervisor and one actor per **world/realm** (each its own
  tick, no locks). A **world** = one **player-seeded** persistent overworld + its
  players + monsters + **player towns built on it** (towns are content, not their
  own shard). Worlds are player-created & capped; scale = **many worlds**, a full
  one queues (no auto-fork — unique towns). Pure single-actor refactor first, then
  multi-world + hub handoff. Load becomes **O(N × M)** not **O(N²)**. Makes world
  shards **persistent by default — they hibernate to Postgres when empty and store
  only the *seed delta*** (built/damaged/harvested/population diffs, not the map;
  worldgen regenerates the baseline) — the current `runs.is_empty() ⇒ instance =
  None` is wrong for a buildable/sieged world. Only a player's run/backpack stays
  ephemeral (level resets to 1 on death *or* extraction). Underpins MON-2 camps +
  pinned seeds. Add a two-world isolation QA test.
  - 🟡 *In progress — PR-a + PR-b landed:* PR-a (#129) extracted the `WorldActor`
    struct; PR-b then moved **all** world-touching client handlers (`move` / `submit` /
    `join_battle` / `harvest` / `open_chest` / `equip_loot` / `rename_hero` /
    `set_formation` / `begin_extraction`) from `impl GameState` onto `impl WorldActor`,
    each returning `(Vec<Outgoing>, Vec<WorldEffect>)`; `GameState` is now the **Router**
    (sessions / lobbies / routing) that applies the returned effects. Still one task
    (single-owner/no-locks invariant intact). **Remaining:** the b1-B boundary (spawn
    `WorldActor` as its own task so it never calls `GameState` methods), then multi-world
    + hub handoff + Postgres hibernation, and the two-world isolation QA test.
- [ ] **SC-4 — Cross-process sim + gateways (only when one box can't hold it).**
  Keep the sim central/authoritative; push per-client fan-out to horizontally-scaled
  gateway processes next to the sockets. Determinism makes live instance migration
  a `serde` call; the 100 ms/15 s-ATB clock tolerates the extra hop. Scope as its
  own doc before building.

---

## Epic AX — Agent play (MCP)

Make **AI agents first-class players** via an MCP over the existing wire protocol.
MELDWORLD is an unusually good agent-play target: combat is **server-authoritative**
with a clean `meld-proto` C2S-intent / S2C-state boundary, there's already a **`qa/`
headless-bot framework** driving the real wire, the **turn-based ATB** (100 ms tick,
15 s window) needs *reasoning, not reflexes*, and the **async economy** (stalls,
bounty contracts) is built for offline actors. **PvE-only** (no player-vs-player)
keeps agent participation low-risk. Sequenced so the cheap QA layer lands over
today's protocol; the living-world layer follows **SC-3**. Focus **adventure first**,
then the rest.

- [ ] **AX-1 — MCP over the wire protocol.** An MCP server that lets an agent
  **connect**, **read** authoritative state (overworld snapshot, battle state,
  run/backpack), and **submit intents** (movement; battle actions; `run.harvest` /
  `begin_extraction` / `join_battle`). A thin adapter over the `meld-proto` envelope
  — **no client-side combat math** — reusing the `qa/` bot plumbing. Deliverable: an
  agent completes a full dive→fight→extract loop through the MCP.
- [ ] **AX-2 — Agent-as-playtester harness.** Drive the whole loop with a reasoning
  agent and emit **balance telemetry** — win/extract/die rates by distance, and the
  feel of the loss knife-edge. The honest way to measure the "desperate but not
  despair" tuning *at scale* before the sim/builder layers land. Extends AX-1 + the
  `qa/` conformance suite.
- [ ] **AX-3 — Agent inhabitants (living world).** Agents as first-class **async
  actors**: run stalls, fulfil bounty contracts (**EC**), harvest, and **garrison
  towns/anchors while owners are offline** — populating persistent seeded worlds
  (solves the empty-world cold-start) and answering the offline-siege feel-bad (the
  Shift, CANON §W2). Depends on **SC-3** (populated persistent worlds); PvE-only keeps
  it safe. A natural **premium/convenience hook** (an offline-defense garrison agent,
  pinned worlds — cf. **MON**), sold as *participation*, not "skip the loss."

---

## Not on this roadmap yet (tracked elsewhere)

Endgame breadth — the Vanguard Board leaderboard, the infinite zone past d=5000,
Prestige auras, and seasonal wipes — is specced in
[`behaviors/endgame-seasons.md`](behaviors/endgame-seasons.md) and staged in
[`BUILD-PLAN.md`](BUILD-PLAN.md) M5, but is intentionally *after* the epics above.
Disconnect/resume, sleeping avatars, and wards
([`behaviors/disconnect-handling.md`](behaviors/disconnect-handling.md)) similarly
follow the core-loop work. Pull an item up into an epic here when it becomes the
next thing to build.
