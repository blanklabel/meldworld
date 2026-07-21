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
epics can run in parallel. The two highest-leverage epics are **Last City**
(closes the social/economic loop) and **Gear & Items** (gives the extract-or-die
loop real stakes).

---

## Epic LC — Last City (the persistent hub)

The hub is named **Last City**. This supersedes the "The Weld" working name in
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
- [ ] **LC-3 — Adopt "Last City" as the canonical name.** Rename "The Weld" in
  fiction, all in-game UI/labels, and the proposal's name line; keep the district
  names. Add a CANON glossary entry (§G) for **Last City**.
- [ ] **LC-4 — Interact with your inventory inside town.** Open and manage the
  Vault + equipped gear + (pre-dive) loadout from within Last City — the Vault-Deep
  district UI reading the live `GET /v1/vault` / `/vault/gear`, plus equip/unequip.
  Prereq for GR-1/PT-1/PT-2/SV-1 having a home. (Depends on GR-1's slot model.)

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
  via ClassEmblem." Existing classes (Hunter/Psyker/Resonant/Shifter/Iron Hull)
  define the taxonomy — see [`CLAUDE.md`](../CLAUDE.md) "Combat & class taxonomy."

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
distance model, biome bands, per-section streaming, verticality).

- [ ] **WG-1 — Dungeons.** Discrete, enterable sub-spaces off the overworld
  (hand-flavored, denser encounters/loot, maybe a mini-boss) distinct from the
  open corridor. Define generation, entry/exit, and how they scale with distance.
  New `behaviors/dungeons.md` when the design hardens.
- [ ] **WG-2 — Random starting biome (except the first run).** A player's dive
  should start in a **random** biome every time — *except their very first run*,
  which stays the deterministic Forest tutorial. Seed the run's origin biome from
  account+run state with the first-run carve-out. Touches
  [`behaviors/world-generation.md`](behaviors/world-generation.md) biome-band
  assignment.
- [ ] **WG-3 — Randomized biome ordering.** The sequence of biomes as you march
  outward should be **randomized per run** (seeded), not a fixed Forest→Desert→…
  chain — while keeping the distance→difficulty curve intact. Shuffle the biome
  band order from the run seed.
- [ ] **WG-4 — Radial spread with Last City always to the west.** The world opens
  outward across ~**350°**; **Last City always sits just to the west** of where
  you leave it, appearing as a giant wall. Cross the western border and you step
  **right back into the city** — the one permanent, safe anchor in a world that
  worsens in every other direction. Establishes the city↔maze boundary as a
  walkable seam (ties LC-1's presence loop to the maze exit; reframes the current
  Threshold entry). Fold into [`behaviors/world-generation.md`](behaviors/world-generation.md).

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
- [ ] **FS-4 — Gatekeepers & unique bosses.** Massive, unavoidable boss arenas at
  biome/hub borders (GDD §3/§4) — progression blockers, multiplayer rally points,
  and the source of **class-emblem** drops feeding CL-1. Add the encounter type,
  HP-sizing, arena placement in world gen, and the merge/raid behavior
  ([`behaviors/combat-atb.md`](behaviors/combat-atb.md) battle merge).

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
