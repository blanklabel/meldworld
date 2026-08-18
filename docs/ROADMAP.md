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
  clear this dungeon" hunts, not the full system. 🟡 *The light cut ships* — eight posted
  hunts, credited off real kills/depth/extractions/dungeon clears and claimed at the
  Bounty Board; `AD-4` stays open for the rest of the system (see its epic).

**④ Polish the feel — as important as any new system.** A slice becomes "want to play"
through feel & clarity, not more mechanics.
- [x] **P1-2 — Combat & moment-to-moment feel pass.** Hit feedback/juice, damage/heal
  readability, turn/telegraph clarity, pacing — make the ATB *feel* good, not just be
  correct. Screenshot/video-verify (CLAUDE.md "Visual verification").
  - **Most of the juice was already there** and nobody had recorded it: the struck sprite
    flashes white, recoils and judders, the attacker lunges and plays its own action clip,
    and the numbers carry a vocabulary (`CRIT!` gold, `WEAK!` big and shaking, `RESIST!` /
    `IMMUNE!` / `ABSORB!` behind the Psyker's threat-sight). #216 gave conditions a palette
    on the cells, the creature bars and the sprites themselves; #227 got the victory menu
    out of the way. What was missing was narrower than the line suggested.
  - **A number now belongs to the combatant it landed on.** `render_hit_fx` anchored by
    *identity* — `monster_combatant` (only ever `enemies.first()`) drew top-centre and
    everything else fell through `your_ids.position(…).unwrap_or(0)`, so every enemy past
    the first, and every joined ally, printed its damage over **hero slot 0's cell**. Packs
    are standard and the level-50/75/100 rungs added a wave of all-enemy abilities, so one
    Purging Light sprayed its whole sweep onto the first hero. Each number is now projected
    over its own arena actor, the way `render_enemy_panel` already hangs the HP bars — the
    class of bug, not the instance. The hero-cell path survives only as a fallback, and
    only for a hero we actually field: printing someone else's number on slot 0 was the
    fault.
  - **Simultaneous hits stack instead of overstriking.** `Hit::stack` records how many live
    numbers already shared that target, and each is lifted clear (`stack_step`, held above
    the font size by test) with an alternating sway — an all-enemy sweep used to resolve
    four numbers onto one pixel.
  - **Turn clarity is the enemy's own ATB gauge, not a turn-order list.** It already ships:
    a second bar under each foe's name/HP, gated on Predator's Eye's top tier
    (`hunter_intel_atb_at`, run level 6). Left with the **Hunter** per `CL-2` — sizing up
    prey is the guild's trade — and the stale comment calling it the Explorer's is fixed.
  - **The feel is tunable at last** (`meld_client::feel`). The timings and magnitudes were
    bare `const`s plus magic literals inside the animation systems, so dialing them in was
    a recompile per guess. One `BattleFeel` resource with the shipped values as defaults
    and a runtime override — `MELD_FEEL="lunge_ttl=0.5,number_rise=70"` / `?feel=`. A bad
    knob warns and is skipped rather than failing the boot. Authoritative pacing
    (`tick_ms`, `gauge_fill_divisor`, `turn_timeout_ms`) stays in `balance.toml`, since it
    is a rule rather than a look. `number_height` was picked *with* the dial: 2.0 put every
    number through the sprite's own art, 3.1 clears the head and sits just off the target
    diamond.
  - Verified natively (`MELD_BATTLE`, whose mockup is now the fixture for both bugs — a
    second **live** enemy that is not `enemies.first()`, and two numbers on one target).
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
  - 🟡 *Chat landed early, additively, ahead of the presence loop* — `chat.say` /
    `chat.line` in [`meld_proto::realtime::chat`](../shared/meld-proto/src/realtime.rs),
    handled at the **Router** level in `game.rs` (never the world: a world-scoped handler
    swallows every line said by anyone not currently in a maze, which is most people most
    of the time). Two channels, `party` (the people you are among — in the maze with you,
    or in town with you) and `world`. Deliberately **not** proximity yet: distance makes a
    line silently vanish, and "did that send?" is the worst first experience of a chat
    box; LC-1's ward sharding is what narrows it later. Sender and timestamp are stamped
    server-side, because a chat line's whole value is trusting who it came from. Driven by
    AX-1's `say` / `chat` tools, so an agent and a human in one world can actually talk.
    **Remains:** the town presence loop, proximity scoping, emotes, and rendering other
    players in The Commons.
- [ ] **LC-2 — Fix the reversed walk direction.** In Last City the hero sprite
  walks *opposite* the pressed arrow (push one way → walk the other). Camera-
  relative movement sign/axis bug in the city controller (client
  [`main.rs`](../client/crates/meld-client/src/main.rs) `Screen::City` movement).
  Screenshot/verify the four directions.
- [x] **LC-3 — Adopt "The Last City" as the canonical name.** Renamed "The Weld" in
  all in-game UI/labels + client code, the proposal's name line, and added a CANON
  glossary entry (§G) for **The Last City**. District names kept.
- [x] **LC-4 — Interact with your inventory inside town.** Open and manage the
  Vault + equipped gear + (pre-dive) loadout from within Last City — the Vault-Deep
  district UI reading the live `GET /v1/vault` / `/vault/gear`, plus equip/unequip.
  Prereq for GR-1/PT-1/PT-2/SV-1 having a home. (Depends on GR-1's slot model.)
  **Shipped:** `[V]`/`[E]` at the Vault-Deep opens the three-column menu in town (the City
  state registers `menu::render_main_menu` and the whole equip flow, not just
  `render_overlay`, whose Inventory arm is empty — with only the latter registered the vault
  looked like it would not open at all), the material and gear lists read the live endpoints,
  and named loadouts save/apply from the Drill Yard.
  **Equip/unequip was the last piece, and it was broken until #218:** every hero starts
  dressed in all six slots, and `set_equipped` refused a full slot with a 409 — so *every*
  press in the picker was a refusal on a slot that is always occupied, and nothing displayed
  the reason. A full capacity-1 slot now SWAPS (the displaced piece returns to the Vault);
  only a full multi-capacity category still refuses, since there the player is choosing which
  ring comes off. Vault writes report through `ServerMsg::VaultNotice`, so a refusal is spoken
  where the press happened rather than on a HUD line behind the panel.
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

- [x] **PT-1 — Change party rows (front / back row).** Let a player assign each
  hero to a front or back row and swap them, with the row affecting combat
  (melee reach / damage taken / target priority — pick the rule, add its
  `[TUNABLE]`, cite it in `combat-atb`). Server-authoritative; rides existing
  party/roster surface. Editable in Last City (LC-4) and on the party screen.
  - *Shipped:* `run.set_formation`, persisted on `heroes.back_row`, the toggle on each
    hero's own cell, `row:back` on the wire, and back-row heroes rendered deeper as
    busts. Both combat rules chosen: **damage taken** and **target priority**.
  - **The row is a TRADE, and it was not.** `back_row_damage_mult` halved every incoming
    blow and nothing was given up for it, so the optimal formation was the whole party in
    the back rank for a flat **2x effective party HP** — `handle_set_formation` has no
    rule against it and needs none, because the trade is the rule. Now: only a
    **physical** blow is stopped by the rank (a spell, a Focus or an elemental breath
    reaches it at full force), and a back-row hero gives up half its own **physical**
    output in return. A caster loses nothing standing back, which is exactly why the back
    row is a caster's home and the front line is a martial's — and a hero whose weapon
    carries an elemental `brand` (`AD-3`) keeps full damage from the back, which is a real
    reason to want one.
  - **Three classes had been dealing TRUE damage.** `hero_attack_type` listed five classes
    and fell through to `DamageType::None`, which bypasses the modifier map entirely — the
    **Hunter** (the martial baseline, and where the Explorer's kit moved in #206), the
    **Smithwright** and the **Keeper** were all missing, so all three ignored every
    creature resistance and immunity and would have held the front line for free.
    `no_fielded_class_swings_untyped` reads the class list off the registry now.
- [x] **CR-9 — Creatures fight to a profile, not one rule.** Every creature in the game
  picked its target the same way — lowest HP, with a back-row redirect — so a party
  learned one lesson at the hub and it held to the deep. Five profiles
  (`meld_proto::TargetProfile`): **Weakest** (finish the wounded), **Random**
  (unpredictable rather than stupid), **Backline** (hunts the rank on purpose — the
  counter to hiding every caster behind a wall), **Role** (the healer first, then the
  casters — not more damage, damage spent where the party can least afford it), and
  **GangUp** (the pack shares one mark and commits to it).
  - **Three inputs, in order of authority:** the kind's own nature (an ambusher slips
    past the line, a pack animal converges, a big mindless body swings at whatever is in
    front of it); then the **encounter class** — an Elite, Gatekeeper or boss is smarter
    than its escort whatever its kind; then **level**, because deeper creatures are
    smarter *on average* (`[ai] smart_*`: a share of ordinary spawns rolls tactical past
    a floor, climbing to a cap). Rolled off the creature's own id, so the same creature in
    the same fight always thinks the same way and promoting one cannot shift any other
    roll in the battle.
  - **A gang-up mark is announced.** A pack converging on your healer with no explanation
    reads as the game cheating, so the mark is shouted on the turn it is set *or moved*
    (`Resolution::callout_text`, the same bubble a telegraphed ability uses) — and it can
    move: `gang_switch_chance` lets a pack switch to a better target mid-fight instead of
    committing to one hero until it dies.
  - **One targeting function, finally.** The rule existed as two near-identical copies —
    one inline in `resolve_monster_turn`, one in `pick_weakest_hostile` for abilities — so
    a creature could hunt the back rank with its claws and the weakest hero with its
    breath. `choose_target` is the single place a creature decides.
- [x] **PT-2 — Save, name, and swap party loadouts in town.** Named compositions
  AND the gear they wore, saved and re-applied at the Drill Yard (whose placeholder
  had promised "build templates" all along). `party_loadouts` + HTTP CRUD +
  `POST /:name/apply`.
  - **The client never names gear — not on save, not on load.** Save captures the
    equipped set server-side from `get_gear`; load sends only a NAME and the server
    replays its own captured ids through `set_equipped`, which scopes every lookup to
    the owner and refuses broken or class-illegal pieces. A client that could name the
    gear could name gear it does not own and have it equipped on the next load.
  - **Load-time re-validation, not save-time trust.** A loadout is a promise made in
    the past: gear gets wrecked, sold and lost, and unlocks change. Anything that no
    longer qualifies is skipped and reported (`gear_missing`), so the worst case is an
    empty slot rather than a wrong one. Compositions are re-clamped the same way.
  - Rows are NOT part of a loadout yet — they persist per hero slot already, so
    folding them in is additive when `PT-1` wants it.

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
  Phoenix Guard gauntlet+shield, Shifter dagger with **two** legal off-hands (second dagger
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
    Stave, Phoenix Guard Warhammer → Kinetic Gauntlet).
  - *Also shipped:* the equip **UX** — `GET /v1/heroes` returns each slot's class so the
    Equip tab knows the rules in town too; a row this hero's class cannot wear renders
    dim with the reason (`-- too heavy`, `-- cannot wield`) and a press does nothing,
    reading the same `meld_proto::equipment` table the server enforces so the UI can
    never disagree with it; and picking a two-handed weapon **puts the off-hand away for
    you** instead of returning a 409. Screenshot-verifiable via
    `MELD_INVENTORY_TAB=equip`.
  - **Remains:** authoring signature pieces (the class-exclusive armor `AD-1` uniques
    hang off), and a stray-descriptor audit once loot tables grow.
- [x] **GR-7 — Persist a hero's class per slot.** Today the party is chosen per dive and
  gear equips to a *slot*, so in town the server cannot say what class hero 2 is — which
  is why `GR-5` can only enforce at derivation. Persist a class per hero row (the
  `heroes` table already holds name + `back_row`), so a hero becomes a character rather
  than a slot. Unlocks: equip-time legality (`GR-5`), saved loadouts (`PT-2`), and
  per-hero progression later. Party choice at dive time becomes *which* heroes you take.
  - *Shipped:* `heroes.class_key` (additive), written from the **resolved** party in
    `form_run` (so default mixed parties are recorded too) via `DbWrite::HeroClass` —
    the party you take down is the roster you come home with. `set_equipped` now
    refuses an illegal equip with the **rule that failed**
    (`EquipResult::ClassLocked(Legality)` → a `409` that says "cannot wield that kind of
    weapon" / "cannot wear armor that heavy" / "belongs to another class"), and
    `TwoHandedConflict` enforces both-hands-or-neither in either equip order. A hero with
    no recorded class is never locked out (derivation stays the backstop). The starter kit
    no longer hands a buckler to a two-handed class.
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
- [x] **GR-4 — Consumable potions that do more than heal.** Six potions
  (`meld_proto::consumables`), each reusing a state the ATB engine already models so a
  potion is content rather than new machinery: **Bloom Salve** (part heal), **Elixir**
  (full heal), **Bulwark Tonic** (Barrier — drink *before* the blow), **Mending Draught**
  (Regen), **Ghostdust** (Evasion), **Fury Philtre** (banked Adrenaline; inert on any
  class without it, exactly like the matching affix). Magnitudes are `[consumable]`
  `[TUNABLE]`s. `resolve_item` reads the registry instead of treating every item as a
  heal, and an unknown item id still heals so an older client is never stranded.
  - *Inventory half done too:* consumption was already server-side (an Item action is
    checked against the run backpack and spends one, `battle_item`), but the client's
    Items page was a hardcoded `Salve`/`Elixir` pair — and **`salve` was not a real item
    kind**, so that row could only ever answer "Out of salve". The page is now built from
    the run backpack: only potions the party carries, with counts ("Bloom Salve x3"), and
    a `(no potions)` row when empty, so it can never offer what the server will refuse.
    The starting kit and the consumable/material split now use registry keys instead of
    string literals, so a new potion is never mistaken for a crafting material.
- [x] **GR-8 — Loot that actually drops.** Two faucets were plumbed but shut, so a dive
  produced almost nothing you had not paid for — the bug in Epic ①'s "every dive should
  produce something exciting" was not that drops were *flat*, it was that there were
  none.
  - *Gear was gated above where anyone plays.* `roll_creature_loot` refused gear below
    `red_chest_floor_distance` = 300, but the 8-area chain's deep portal sits at
    `d ≈ 342–384` — two tunables set independently and never checked against each other.
    Measured: **0 gear in 2000 kills at every depth up to d=299**, 34% from d=300, i.e.
    the chase existed only in the stretch you cross on the way out, and dungeon chests
    inherited the same gate. The hard cutoff is now a **ramp** (`[loot]
    gear_ramp_start_distance` = 40, `gear_ramp_start_mult` = 0.15): 5% at d=40 climbing
    to the full 35% at d=300. CANON §B's intent is preserved — deep is still where the
    gear game lives — and the ramp reaches exactly 1.0 at the floor, so every deep drop
    rate is untouched (held by test).
  - *Potions had no drop path at all.* Every one of the eleven was shop- or craft-only:
    after the starting 3 salves + 1 elixir, a dive yielded no consumable it had not
    bought or brewed. A kill or a chest now also drops one at `[loot]
    potion_drop_chance` = 0.18, from the pool whose own `ConsumableDef::tier` is at or
    below `tier(d)` — the hub ring gives the Apothecary basics, the deep bands open the
    trophy line, so the drop softens the K3 sink without replacing it. The pool is
    derived from the registry (a new potion joins by existing) and excludes Revive and
    Experience, which already have their own faucets and would otherwise double-roll.
  - The potion draw takes its **own RNG sub-stream**: a draw from the shared one would
    have shifted every gear/rarity/affix roll above it. A test turns potions off and
    asserts the rest of the loot is byte-identical.
  - **Remains:** `class_emblem_drops` is `vec![]` at all three call sites — a wire field
    nothing has ever filled. Gatekeeper emblems belong to `FS-4`/`MP-1`.
- [x] **GR-9 — Two containers: the Party Inventory and the hero pouches.** A hero can
  only use what **that hero** is carrying in a fight. Everything found lands in the
  shared **Party Inventory**, which is **unbounded** — a slot cap punishes finding
  things, which is the opposite of what loot is for. Each hero has a capped **pouch**,
  and moving items between the two is an overworld action, both directions
  (`run.move_item`). The scarcity is **reach**, not capacity: the pouch cap is what
  turns "who is carrying the heals" into a decision you make before the fight.
  - *What was there before:* `backpack_slots` (40) and `hero_pouch_slots` (10) summed
    into ONE flat number over ONE `Vec`, so the "pouch per hero" was capacity arithmetic
    rather than a container — nothing tracked which hero carried what, and both the
    client menu and the server's battle Item action read the whole pile. The two-tier
    model existed only in the comment beside the two tunables.
  - `PlayerRun::pouches` is now a real per-hero container, `run.pouches` rides the wire
    as a whole snapshot (small and bounded, so cheaper than reconciling deltas), and the
    battle Item action is checked and spent against `pouch_qty`/`spend_from_pouch` for
    the **acting** hero — resolved from `player_combatants`' party-slot order. The
    acting hero's pouch pays even when the potion targets an ally.
  - `backpack_slots` is **gone** rather than left dead: a tunable claiming to cap a
    container nothing caps is the same class of bug as `GR-8`'s gear floor.
  - **Death takes both**, by the same rule — a pouch is not a safe-deposit box, so no
    arrangement of your items survives a wipe, and `lost` reports them together. The
    flee toll and a creature's steal reach pouches too (the steal tries a hero *first*,
    since that is where the potions now live); `move_item` mints real `item_id`s because
    the flee roll is keyed on them and blank ids would share one roll. A flee reports the
    two halves **separately** — `backpack_update` removals for the inventory, a fresh
    `run.pouches` for the rest — since a pouch loss announced against the shared
    inventory would decrement a bag stack the client's mirror really does hold.
  - *Field use stays generous:* out of combat **either** container is in reach, spending
    the drinker's own copy first, so a potion you already handed out is never stranded.
    The starting kit is dealt into pouches round-robin (balance's totals spent, not
    multiplied per hero) so the first fight needs no transfer ritual; Town Portals stay
    in the inventory, since extraction is a menu action.
  - *Client:* the menu's column is **Party Inventory**, listing the shared stock plus
    every hero's pouch (`3/10`, tap to take back); picking an item opens **GIVE TO** and,
    for a field-usable one, **DRINK NOW**. Staging is no longer gated on
    `usable_in_field` — that gate made the fight-only potions, the exact ones that need a
    pouch, the ones you could not put in one. The battle Items page reads
    `held_potions(backpack, active_slot)`.
- [ ] **GR-4b — Consumable healing items (legacy line).** Field/battle-usable heal items that are
  **consumed on use** (decrement + destroy at zero). Wire into the existing async
  battle-injection path (GDD §6; [`behaviors/async-interaction.md`](behaviors/async-interaction.md))
  and direct self-use. Stackable in the backpack; add `[TUNABLE]` heal amounts.
  - Battle use ships (finite, inventory-backed; `qa/tests/potions_are_finite.rs`).
  - **Field use ships** — `run.use_item { item_kind, hero_slot }` heals / full-heals /
    revives / pours an Insight Mote out of combat, from the menu's Items column.
    Effects that only exist inside a fight (Barrier/Regen/Evasion/Adrenaline) are
    refused, and a potion that would change nothing is refused rather than consumed
    (`qa/tests/field_item.rs`).
  - Still open: the **async battle-injection** path (a teammate posting an item into
    someone else's fight) — that is what keeps this box unticked.

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

- [ ] **PG-1 — Progression foundation: a ladder that reaches 255, and dead heroes earn
  nothing.** Design: [`proposals/progression-and-unlocks.md`](proposals/progression-and-unlocks.md).
  - *Shipped:* the level curve **is its design statement** — level L takes
    `fights_per_level_base` fights against a same-level encounter plus one more every
    `fights_per_level_ramp` levels, with the XP number **derived** from the encounter
    tables rather than tuned separately, so creature XP and the ladder cannot drift apart.
    Two earlier shapes are retired: doubling every level made the 255 cap unreachable by
    construction, and `L + 1` charged 54 at-level fights for the level-10 second party
    slot — most of a first session spent in the game's least interesting configuration. XP stays
    **dive-scoped** (depth is the meta-progression: a deeper hub starts a run at a higher
    `base_run_level`). **A level-up raises nobody** — it tops up the living, and the fallen
    come back on a **Waking Salt**; the world also sprinkles **Insight Motes** (bankable
    XP). `[runs] max_hero_level = 255`.
  - *Also shipped:* **per-hero levels inside the run** — each hero climbs its own ladder
    from `base_run_level`, so the hero doing the killing is the one that gets stronger, and
    **a fallen hero earns nothing**; the player's headline level follows their best hero.
    `PlayerRun::heroes_at_level` is what the slot rules will count. And the one thing that
    *does* persist: **`class_bests`**, the best level ever reached per class — monotonic, so
    a shallow dive never lowers a record earned deep.
  - *Also shipped:* the **undead rite** — the encounter the Phoenix Guard unlock will
    hang off. Bosses now have a **lineage of their own** (`abilities::boss_faction`:
    Choirmother/Hollowbishop/Miredrowned/Sepulcher are *undead*; Ironmaw/Rustfang/
    Gloamhound/Weeping Colossus/Pyrewarden are *constructs*) instead of inheriting the
    faction of whatever creature they were promoted from. Past tier 4 a spawn can become
    an **undead boss with four undead minions** (`[encounters] undead_rite_*`) — a pack
    with a champion at its head, harder than an Elite and short of a Gatekeeper, and
    fenced off so no ordinary pack merges into it. Every boss also gained a **deep-gated
    ability** (level 45+) and a **palette band** that darkens with the level it is met at
    (`boss_palette_band` → `boss_band:<n>` on the wire → the client's material tint), so
    the same named boss escalates in both kit and look.
  - *Also shipped:* **Iron Hull → Phoenix Guard.** The Order of the Phoenix Guard
    walks out of fires nothing else survives, which is why the class is earned by
    surviving the undead rite. Key is `phoenix_guard` everywhere (wire, balance,
    sprites); `iron_hull` stays a serde alias so heroes persisted under the old key
    keep their class rather than silently falling back.
  - *Also shipped:* the **unlock system** (`CL-1`). `meld_proto::unlocks` is the
    registry both sides read — eight unlocks, each carrying its trigger, the line a
    locked party-builder row shows, and the line its banner shows. A new account
    starts with ONE slot and the Explorer; slots open at 1×L10 / 2×L20 / 3×L30
    (counted *simultaneously* in a dive), and the four earned classes each wait on
    the slot that seats them. `WorldEffect::Milestone` reports the fact, the Router
    grants it against the session's in-memory set (so a milestone fired every tick
    still only grants once), `DbWrite::Unlocks` persists it off the tick, and
    `run.enter_maze` CLAMPS a requested party to what the account owns rather than
    rejecting it. Client: a `run.unlocked` banner in the house style and a "Still to
    earn" block on the party screen.
  - *Also shipped:* **one menu look** — `meld_client::glass`, the single definition
    of the frosted-glass menu surface (fill, edge, radius, scrim, selected/hover),
    replacing sixteen hand-rolled panel colours across five files. Every menu now
    imports it: inventory/equip/status, the battle command cross, the level-up
    screen, the unlock banner, the city title + Apothecary shelf, Join and Lobby,
    and the overworld HUD. Selection is one gold wash everywhere.
  - *Also shipped:* **the ability ladder + tooltips, and the faction canon behind
    them.** [`docs/lore/factions.md`](lore/factions.md) is now the source of truth for
    the nine orders and their six-rank ladders; every order gates its senior ranks at
    character level **5 / 9 / 13 / 17**, and that IS the ability ladder — an ability
    arrives as a promotion. `meld_proto::skills` became a real registry (key, name,
    class, unlock, **rank**, **description**), so the server's gate, the battle menu's
    rows *and tooltips*, and the party screen's per-hero ladder are one definition
    instead of four hand-maintained lists.
  - *Also shipped:* **the Hunter, reintroduced** — the guild whose mission ("disposal
    of dangerous non-civilian creatures", "adrenaline junkies") is the game's core
    loop, so it carries the martial Adrenaline kit. Unlocked by **extracting** (the
    hall pays on evidence, not stories). The **Explorer** keeps the starting slot and
    gains its own order-true kit: tempo and stability (Trailblaze → Field Dressing →
    Read the Ground → Stable Ground → Safe Passage → **A World Known**, which fills every
    ally's gauge).
  - *Also shipped:* the **Phoenix Guard's anti-undead kit** — Silvered Strike, Rite of
    Rest, Holy Censure, Purging Light, Unbroken Vigil (party Barrier), Eradication (an
    execute) — plus a standing `phoenix_guard_undead_mult` against the risen, which is
    what makes it a counter rather than a re-skin. Its old kinetic kit is **reserved
    for the Order of the Iron Hull**, a future monk class; `iron_hull` is no longer a
    deserialization alias, so that class can claim its own key.
  - *Also shipped:* **the three-column menu** (`menu.rs`) — a Dragon-Quest-remake
    cascade. Column one is the nav (*Items / Materials / Party / Map*); choosing one
    opens column two to its right; from Party, a hero's **Equipment** or
    **Abilities** button opens column three. The nav never leaves, so Back steps out
    one column at a time. Party shows HP, the class's own resource (this ATB
    adaptation has no MP), EXP and stats, with the formation toggle on the hero's own
    cell. **The org rank rides beside the class name** — `Explorer - Pioneer` — scaled
    from the lore's D&D levels (cap 20) to ours (cap 255): ranks at 1/25/65/115/165/215.
  - *Also shipped:* **unlock hints removed everywhere.** No locked ability rows, no
    "reach level N", no trigger text — the Abilities pane lists only what a hero has.
    Discovery is the fun.
  - *Also shipped:* **abilities spread to ~100** on square-number levels (1, 4, 9,
    16, 25, 36…), which on the `L + 1` fights-per-level curve makes each new ability
    cost a step up in commitment rather than an ever-flatter trickle.
  - *Also shipped:* **the crafters' overworld ladder (MS-1's second half).** Every other
    class earns an overworld perk that scales with run level; the two PROFESSION classes —
    the pair whose whole identity is what they do between fights — earned nothing for
    walking around. Smithwright: *Prospector's Eye*, *Efficient Setup*, *Travelling Forge*,
    *The Long Shift*. Keeper: *Forager's Path*, *Green Thumb*, *Rooted Ground*, *The Whole
    Vein*. Each reads only its own trade's materials (`ore` vs `reagent`, off the material
    registry) and is force-included in that player's snapshot rather than widening the
    shared interest cull. `compute_perks` became a free function so the whole system is
    finally unit-tested, and `no_class_walks_the_overworld_with_nothing` reads the class
    list off the registry — the two missing classes were missing for a release.
  - *Also shipped:* **every party-wide capstone is a once-a-fight call.** Eternal Bloom,
    Phoenix Ascendant, Anvil Chorus, The Great Work, World Tree and Hallowed Ground join
    Now, The World Entire, Iron Lung, Pin the Prey, Grand Larceny and Second Life.
    `a_party_wide_capstone_is_a_once_a_fight_call` checks the RULE rather than the list —
    a class's deepest rung, if it covers the whole party or every enemy, must be gated
    (Psyker Foci excepted: a Focus is held, and its limit is the slot).
  - *Also shipped:* **an encounter is a POOL divided among the heroes still STANDING.**
    The last survivor of a bad fight banks the whole thing — risk against reward, since
    the party-assembly loop iterates the full ROSTER, so that survivor still meets a
    four-hero encounter alone (`a_fight_is_scoped_to_the_roster_even_when_only_one_hero_is_left`
    pins the pairing: scale encounters to the living later and the risk half silently
    disappears). `award_hero_xp` takes `shares` separately from `party_size` — they were
    one argument (`party_size.max(slot + 1)`), so a lone survivor in slot 3 still divided
    by four and three-quarters of the pool evaporated. That separation also retired the
    pre-multiply hack at the mote sites.
  - *Also shipped:* **the split is visible, and `pacing_arc` measures it.** Every
    `HeroView` reported one shared run-level `xp`/`xp_to_next`, so four heroes sharing a
    pool showed the identical number a lone hero did — the test's own header had recorded
    that as "all four sizes banked the same 124 XP" and concluded the split could not be
    seen from inside the game. Each hero now carries its own banked XP and its own bar.
    The test had two further faults only that exposed: it read `xp`, which is the
    REMAINDER after a level-up spends its cost (a solo dive that banked 185 reported 61),
    and it asserted XP *per fight*, which is not a claim the balance makes — an encounter
    pays `encounter_party_scale` BEFORE the split, so a full party's per-hero share is
    ~1.1x a lone hero's. The cost of fielding four is TIME, and that is measured now:
    **4.83 / 1.85 / 1.63 / 1.30 XP per second** at one to four heroes, a monotone decline.
  - *Also shipped:* **one stack ceiling, and Regen finally decays.** `regen +=`
    accumulated without limit and never faded — the only lasting effect in the game with
    neither decay nor expiry — so turns spent on it bought permanent, ever-growing party
    sustain (measured: 5 stacks healing 150 HP a turn, forever). Regen now sheds
    `regen_decay_fraction` a turn like the Barrier beside it, and **every** lasting effect
    (Regen, Barrier, Evasion, the fight-long attack buff, and the consumables that grant
    them) answers to `[battle] max_effect_stacks` = 5, refused past the ceiling rather than
    silently wasted. All of it routes through four `grant_*` helpers so a call site cannot
    add a stack nobody counted.
  - *Also shipped:* **the flat-magnitude fault.** Some grants were fractions of max HP and
    scaled with level; others were flat points and did not. A hero runs 40 max HP / 12 atk
    at level 1 to ~535 / ~309 at 100, so the flat ones decayed to nothing: the Keeper's
    World Tree restored **4.9%** of a hero where the Resonant's Eternal Bloom restored
    **85%** — the Keeper stopped being a healer around level 30 — and the Smithwright's
    `+4 atk` Tempering Blow went from +33% to +1.6%. Barrier decay was flat too, so a deep
    hero's Barrier outlasted the fight. Every one is now a fraction of the recipient
    (`Battle::scaled_to` / `grant_regen`), and **the Resonant is the best healer by rule** —
    `the_healer_is_the_best_healer` holds both crafters' numbers under the healer's.
  - *Also shipped:* **round rungs.** `skills::RUNGS` = 1 / 5 / 10 / 20 / 35 / 50 / 75 / 100 /
    150 / 200 / 255, replacing squares — a player counting to their next ability counts in
    tens, and 49 became a legible **50**. `ladder_top` is 255 for a caster, 100 for the rest,
    so the casters genuinely learn most: Psyker **Event Horizon** and Resonant **Second
    Life** land at 255.
  - *Also shipped:* **once-a-fight calls, spent centrally.** A martial class's repeatable
    rows stop improving at 50 and gear carries it from there; what it learns after is one
    dramatic call per fight — Hunter **Pin the Prey** (the pack snared at once) and Shifter
    **Grand Larceny** (a Mug against every enemy, every pocket picked), plus Iron Lung and
    The World Entire. `resolve_skill` marks them spent on any successful resolve rather than
    each arm pushing its own key, which was a list an ability could fall off and be infinite.
  - *Also shipped:* **every class learns at 50 and at 100.** Five of the eight stopped
    at 25 or 36 — the Hunter, Shifter, Phoenix Guard, Smithwright and Keeper all ran
    out of ladder while the level cap is 255 — so levelling stopped paying for most of
    the roster. Thirteen new abilities close it: Explorer **The World Entire**;
    Hunter **Iron Lung** / **Apex Predator** / **Pin the Prey**; Shifter **Assassinate** /
    **Grand Larceny**; Phoenix Guard **Hallowed Ground** / **Phoenix Ascendant**;
    Smithwright **Anvil Chorus** / **The Great Work**; Keeper **Thorn Grove** /
    **World Tree**; Psyker **Event Horizon**; Resonant **Second Life**.
  - *Also shipped:* **archetype now governs menu WIDTH, not ladder depth.** The Dragon
    Quest lesson it encoded — a martial class's late game is its weapon, not a longer
    menu — survives in *how* a class reaches the top: martial (Hunter, Shifter) gets
    there through `upgrades`, so Frenzy becomes Apex Predator and the menu stays four
    rows; hybrid may field 8, caster 10. `menu_width` replaces `ladder_ceiling`, and
    tests hold both halves: every class learns at 49 and 100, and no class outgrows its
    width. "More abilities" still cannot quietly become "ten each".
  - *Also shipped:* **the registry owns targeting and routing.** `SkillDef.target`
    (Enemy / Ally / Caster / AllEnemies / Party) replaced two hand-written lists that
    had both gone stale against it: the engine's per-class dispatch in `resolve_skill`,
    where an unlisted key fell through every arm and came back "unknown skill" — a row
    in the menu that cost a turn and did nothing — and the client's `order_side`, which
    still named the Iron Hull's `root` / `toll_of_the_deep`, so the Phoenix Guard's
    self-cast Rite of Rest and all-enemy Purging Light both asked the player to aim at
    one creature. Tests: every registered ability resolves, and prose and targeting
    agree.
  - *Also shipped:* the **Resonant's full caster ladder** — Mend All (16), Sanctuary
    (25), Revitalize (36), Lifewell (49), Bloodbond (64), Martyr (81) and Eternal
    Bloom (100). Seven abilities of one shape (heal / Regen / Barrier, on one ally or
    all, paid out of the healer's own HP), so they resolve from a table rather than
    seven near-identical engine arms.
  - *Also shipped:* **upgrade chains** — how a martial class progresses without its
    menu growing. `SkillDef.upgrades` marks an ability as REPLACING an earlier one, and
    `skills_for_class_at` drops anything superseded, so a Shifter with **Mug** no
    longer carries **Steal**: the row improved rather than multiplied. Shifter
    Steal (4) → Mug (25, the same theft with a hit on the way past); Hunter
    Power Strike (1) → Crushing Blow (16) and Snare (9) → Pin the Prey (25). Tests
    hold the invariants: an upgrade unlocks later than what it replaces, belongs to the
    same class, hits harder, and the menu's row count does not change.
  - ⚠️ **The deep ladder is authored ahead of what is reachable, on purpose.** XP is
    dive-scoped, and `departure_hub_distance` is hard-coded to the Center Hub in
    `game.rs`, so `base_run_level` is always 1 and every hero starts every dive at
    level 1. On the `L + 1` fights-per-level curve that puts level 16 at ~152 fights
    *in one dive* and level 100 out of reach entirely. **Deeper departure hubs are the
    unblocker** (a hub at distance D starts every hero at `1 + 0.078 × D`), and they
    are a later feature. The persistence they need is already in place: `class_bests`
    holds the best level ever reached per class and the Vanguard board holds the
    deepest distance banked. Until hubs land, the ladder past ~16 is content waiting
    on a system — not a mis-tuned curve to be rescaled.
  - *Also shipped:* **the Psyker's manifestation ladder to 100**, taken from the
    canonical class doc and scaled off its D&D tiers: **Kinetic Wave** (25, grinds the
    whole line), **Thermal Flux** (36, fire-typed so elemental profiles decide),
    **Matter Dissolution** (49, damage *and* permanent armour corrosion), **Phase
    Shift** (64, Evasion it keeps topping up), **Dominate Mind** (81, takes the turn
    outright rather than slowing it) and **Reality Collapse** (100, the line, harder,
    armour irrelevant). Focus slots now grow 2 → 5 across the same span. The client's
    hand-kept four-entry manifest list is gone — every surface reads the registry, so
    it cannot silently stop offering what the engine learned to resolve.
  - *Also shipped:* **the aspect chains are finished.** Seven aspects across six
    manifestations — Gravity Well's Pressure → Gravity → Anchor, plus **Shield** (Kinetic
    Aegis: the Barrier covers the whole party), **Acceleration** (Temporal Anchor: fills an
    ALLY's gauge — the only Focus in the kit that helps someone, and the reason an aspect
    inherits its parent's target only when it lands on the same side), **Freeze** (Thermal
    Flux: slows, and pins anything already slowed), **Brittle** (Matter Dissolution: strips
    every elemental resistance permanently) and **Blackout** (Dominate Mind: it cannot dodge
    at all — checked before the roll, because "cannot dodge" that still rolls is a promise
    the engine breaks one time in twenty).
  - *Balance pass on the above, and it found four things.* **Blackout was dead code** — it
    read as "the target cannot dodge", but `Fighter::dodge` is only ever set for heroes and
    no creature ability grants Evasion, so it took away something no creature had. A blinded
    creature's own blows go wide instead, which is what this engine can express. **Brittle
    was stronger than its source** (all resistances at once vs the doc's one) and now takes
    the strongest one per turn. **Shield reached other players' heroes** in co-op, a property
    the game reserves to set bonuses; it is scoped to its caster's own party, and
    `the_smithwright_is_the_best_party_warder` now guards that axis the way
    `the_healer_is_the_best_healer` guards the other. And **`cap_role_hunters`** holds an
    encounter to one role-hunter: measured, a capped pack puts 20% of its damage on the
    healer where an uncapped one put 100%.
  - **Remains from the Psyker doc:** Psi Points as a real cost, and the Psychic Strain save
    that threatens a Focus when the Psyker is hit. The doc's REACTION aspects (Dampen,
    Static, Vent, Flicker, Rewind) and POSITIONAL ones (Push, Pull, Warp) are deliberately
    **not** built: this engine has no reactions and no battlefield positions, so they would
    have to be reinvented as something else wearing the name.
  - *Also shipped:* **the overworld perk swap** (`CL-2`) — each perk now sits with the
    class whose fantasy it is. The **minimap** moved to the **Explorer** ("a world
    known" — the order that maps the world carries the map). The **predator's eye**
    (mob level → HP → battle ATB reveal) moved to the **Hunter**, whose entire trade is
    sizing up prey. The **Shifter** got Shift-sense instead: it reveals **dungeon
    entrances** within its own radius (plotted on the minimap in the Runner's colour,
    limited by the Runner's sense rather than the map's reach) and, from level 2, reads
    an item's **permanence** before it is picked up — "check the weight".
  - *Also shipped:* **the Shifter actually steals.** Steal/Mug reported only tempo
    before; the engine now raises `Event::Pilfered` — the mirror of the `Stolen` event a
    creature raises when it robs a hero — and the server settles it: chits scaled off
    the creature's tier (a deep theft is worth the trip) plus a rolled chance at the
    biome's combat material. The engine stays pure: it reports that a pocket was
    picked and never learns what was in it. `submit` drains a small pending-events
    buffer so a resolver deep in the call tree can report a fact without threading a
    return value through every signature.
  - **Remains (Shifter):** the world-space entrance beacon and the in-world
    permanence tell. The perk rides the wire and gates the minimap; the extra
    presentation is client rendering, not a rule.
- [x] **PT-4 — the pacing arc: solo is quick, a full party is long.** Two levers, one
  intent. **Encounter XP is now SPLIT across the party** (`xp_split_across_party`)
  rather than paid to every hero in full — a lone hero absorbs the whole lesson, four
  share it. And **creatures scale with the size of the party facing them**
  (`encounter_party_scale = [1.0, 1.9, 3.0, 4.4]`), superlinearly: four heroes bring
  ~4x the damage, so a flat encounter would make a full party's fights the *shortest*
  in the game. Together they give the intended arc — the solo era levels fast on short
  fights (which is the tutorial), and by the time four slots are unlocked the runs are
  long. Indexed to the same progression the player feels, since slots open at 1 hero
  L10 / 2 at L20 / 3 at L30.
  - *Measured, not assumed:* a Gravity Well **cannot** be triple-stacked — the cap is
    2, per the class doc. At realistic level/distance pairs a single stack kills an
    ordinary creature in 5 turns at level 1, 2 by level 10, and 1 by level 25; a
    double stack roughly halves that. The Psyker outscaling creatures is the same
    hero-power-vs-distance divergence noted above, not a stacking bug.
  - *Played, not calculated:* `qa/tests/pacing_arc.rs` drives a real bot through a real
    dive at one, two, three and four heroes and asserts the arc holds — every size can
    win, the lone hero levels faster per fight, and the full party's fights run longer.
    The arithmetic behind these two levers was right and the game was still unplayable
    once (creature attack was scaled by party size and wiped level-1 parties), so the
    numbers get checked by playing them.
- [ ] **PG-2 — Departure hubs: the ladder becomes reachable.** 🟡 *Designed, deliberately NOT wired.* `base_run_level(distance)`
  has always existed and is tested; `departure_hub_distance` is hard-coded to `0` in
  `game.rs`, so every hero starts every dive at level 1 and everything above roughly level
  16 — which is now most of the game's abilities — is authored ahead of what any player can
  reach (see `PG-1`'s ⚠️ note). A hub at distance D starts every hero at
  `1 + 0.078 × D`. This is the unblocker, and it is small, because every part already
  exists.
  - **A hub is somewhere you have BEEN.** Not a purchase and not a trigger: the gate is
    your own deepest recorded distance. The record is already server-owned and already
    written off **validated movement** — the `vanguard` table (`P1-1`), which cannot be
    client-submitted. Read the **all-time** max across seasons, never the live season: a
    season reset must not revoke a hub you demonstrably reached.
  - **Build it as a LOOKUP, not a hub entity.** The run reads one integer. Server-owned
    hubs are rows in "what is the deepest departure point available to this account";
    when `BD-5` lands, a player's forward town becomes another row and nothing is
    rewritten. Do **not** give a hub its own placement/ownership/lifecycle model — that
    is the `Structure` primitive, and `BD-2`'s discipline is explicit (*one primitive,
    many functions — do not build towns, anchors, portals, camps as separate systems*).
  - ⚠️ **HELD OFF.** The registry, the all-time distance read and the "have you been there"
    gate all exist and are tested, but `form_run` ignores them and the chooser is gone: it
    was **half-wired**, because `add_avatar` spawns every dive at the ORIGIN regardless, so
    departing from a deep hub handed out a level-40 party in the tutorial ring — a
    level-select with fiction on it. Finishing it needs spawn-at-distance *and* frontier
    generation around that distance, and extraction (the deep portal, the west-return
    border) still assumes d0 is the start. It may also not be needed: the end-world depth is
    about an hour away on foot, so the ladder it was meant to unblock may not need
    unblocking. Left inert rather than half-live.
  - *Built and inert:* `meld_proto::hubs` (seven hubs, d0 → d3250), `deepest_distance_ever` reading
    `MAX(max_distance)` across every season, `run.enter_maze { hub }`, and `[H]` at the
    Threshold cycling only the hubs you have stood on. `game.rs`'s hard-coded `0` is gone.
    **Clamped, never rejected** — the same shape as `party` being clamped to owned classes:
    a client naming a hub it has not earned gets the deepest one it has, so a stale client
    still gets a dive. No new persistence: the `vanguard` table was already the record.
    The chooser needs a starting level to display and the client has no `balance.toml`, so
    `hubs::start_level` carries the formula — checked against the real `base_run_level` at
    every hub by `the_hub_chooser_agrees_with_the_real_curve`, because a copied formula is
    a formula that drifts.
  - **The end of the game sits at d≈3250** by the LEVEL curve: `base_run_level` reaches
    the `max_hero_level` cap of **255** at **d3256**, where `mlevel(d) = 260` — creature
    level matched to the hero ceiling. Past there a deeper hub buys nothing while creatures
    scale forever.
  - ⚠️ **The "combat ratios hold the whole way out" claim that used to sit here was wrong.**
    It used `stat_mult = (1+d/500)^1.25` from AGENTS.md, which was stale in three ways: the
    real attack exponent is **2.0**, defence is **0.7**, and creature HP is a separate
    LINEAR curve (`1 + 5.4 × tier`). At d3200 that is 55x attack and **171x HP**, not the
    12x the old formula gave. What actually holds is narrower and more interesting: for a
    **hub-matched** party the fight LENGTH stays flat (~15 rounds per ordinary creature from
    d200 to d3200) while durability erodes gracefully (18 hits-to-drop at d200 → 3.5 at
    d3200). For a party that walked or fought out **without** hubs it does not hold at all —
    ~37 rounds per creature and 1.2 hits-to-drop at d3200. **The deep world is tuned for
    hub-fed parties.** That is the real argument for hubs, and it is why the end fight is
    authored with absolute stats instead of a multiple of its surroundings.
  - **Build order, and why the player-built version is not this item.** `BD-5`'s forward
    town *sustains* Run Level — it is a second **source** for the same integer, not a
    different system. But it is the sixth link in its own chain and gated behind the two
    largest deferred epics:
    `SC-3` (world persistence — *a town that dies at instance-close is pointless*; PR-a
    and PR-b landed, the `WorldActor`-as-its-own-task boundary, multi-world, hub handoff
    and Postgres hibernation remain) → `CR-4` (the sim budget `BD-0` must fit inside with
    no new budget) → `BD-1` (wood/stone) → `BD-2` + `BD-9` (the `Structure` primitive and
    builder mode, built together) → `BD-4` (creatures siege structures, extends `CR-2`) →
    **`BD-5`** (towns, and the forward-town Run Level rule) → `BD-7` (persistence wiring)
    → `BD-11` (NPC garrisons, so a hub survives its owner being offline) → `SOC` (guild
    ownership). Ship `PG-2` server-owned now so the ladder is reachable; the lookup is
    what lets `BD-5` add to it instead of replacing it.
- [x] **P1-4 — The board records HOW you got deep, and going quietly is a title.**
  The Vanguard record was a distance and a timestamp, which made two completely different
  runs look identical: 500 encounters and none reach the same tile. A posting now carries
  its **route** — the level it was reached at, fights taken, and fights fled — additive
  columns with defaults, so postings made before this simply report nothing about theirs.
  - 🩹 *Follow-up fix:* `vanguard_me`'s SELECT never listed the five new columns its row
    mapping reads, so `/v1/leaderboards/vanguard/me` panicked with `ColumnNotFound` and
    500'd for **every** player on Postgres — `main` was red on `qa/tests/vanguard_board.rs`.
    A `sqlx::Row` resolves `get` against the RESULT SET rather than the table, so adding a
    column to the schema and to the struct compiles cleanly and fails at runtime; the
    in-memory backend has its own path, which is why nothing local caught it.
  - **The walk is a PLAYSTYLE, not an exploit.** A player outruns every chaser in the game
    (`chase_speed` 4.2 against `avatar_speed` 6.0) and fights are opt-in, so slipping deep
    untouched is real and skilful — and it costs you: you arrive at your hub's base level
    with nothing learned and nothing looted. The **Pacifist** title
    (`UnlockKind::Title` — the first unlock that grants no power at all) marks reaching 500
    deep having taken no fight whatsoever. A **fled** fight still counts as a fight taken,
    because `PlayerRun::fights` increments when a battle is ASSEMBLED rather than when it
    is won: the Pacifist is people who were never seen, not people who ran.
  - The milestone rides the same high-water mark as the depth hunt, so both are asked once
    per new deepest tile rather than on every step of the walk out.
- [ ] **CL-1 — Class unlock system.** Classes become account-persistent unlocks
  rather than always-available. Ship the unlock model (which classes an account
  owns), gate party building to owned classes, and wire the two sources: **Gatekeeper
  emblem drops** (GDD §4; FS-4) and **hiring at a town vendor** (EC-2). See
  [`behaviors/meta-progression.md`](behaviors/meta-progression.md) "class unlocks
  via ClassEmblem." Existing classes (Explorer/Psyker/Resonant/Shifter/Phoenix Guard)
  define the taxonomy — see [`CLAUDE.md`](../CLAUDE.md) "Combat & class taxonomy."
- [ ] **CL-2 — Overworld class perks ("party sense") — deepen the system.** 🟡
  *Partial:* an overworld class-perk system already ships (`[perks]` in balance;
  `game.rs::compute_perks`) — each class's *presence* in the party grants an
  earned overworld capability that scales with the shared `run_level`: the
  **Explorer grants the minimap** (+ mob/portal dots, coverage grows with level),
  the **Hunter grants creature intel**, the **Shifter** reads doors and loot,
  Phoenix Guard shrinks creature aggro range, Resonant grants walking regen, and the
  two crafters read their own trade's materials. **This is where overworld map-reveal
  and threat-reading belong — they're *what a class can do*, a reason to bring it, not
  universal UI.** Remaining: tier them across run level, surface them clearly in the
  HUD, and fold it into CANON with a §/D-number. Anything giving map/threat
  *awareness in the maze* should extend this system, not bypass it. (Contrast UX-1,
  which is town-only, and UX-2, which is universal accessibility.)
  - 🟡 *Threat sense is the Hunter's now.* Marking elites/gatekeepers and aggressive
    mobs, and the widened mob reveal radius, sat with the **Psyker** — where it
    duplicated the Hunter's whole trade (reading a creature before you commit),
    stopped growing at run level 3, and had half of itself invisible: you cannot tell
    you are seeing further than you otherwise would. It is the long-range half of the
    predator's eye, so it lives beside `hunter_intel`. The client's elemental verdicts
    (`WEAK!`/`RESIST!`/`IMMUNE!`) ride the same gate — what a creature is made of is
    the same question as what level it is. `psyker_*` stays a serde alias so a message
    in flight from an older server still parses.
  - 🟡 *A passive must not do the job the class is FOR.* The Resonant's walking regen
    healed the **whole party**, so a party carrying the best healer in the game never
    needed healing between fights and its kit went unspent. It now tends only the
    **Resonants themselves**. A Keeper's alembic field still reaches everyone standing
    in it — a field is a PLACE you choose to stand, not something you get for bringing
    someone — so the two sources bank their sub-1 remainders separately, or the field's
    overflow would heal straight through the Resonant-only rule.
  - 🟡 *The Psyker reaches out and holds things.* Seeing went to the Hunter and the map
    is the Explorer's, so what is left for the order of manifestations is a **verb**:
    tapping a creature **pins** it where it stands (`run.psyker_hold`). It stops moving,
    chasing and skirmishing — but it is still touchable and still fights, because a pin
    is an opening the party chooses to take rather than a way to delete an encounter.
    Engaging a pinned creature opens the fight with **every hero's gauge full**, which is
    the whole reason to spend one. On a **cooldown**, not a cost, and the numbers answer
    one question: *can a Psyker keep everything it can reach pinned forever?* To sustain
    N pins you must lay one every `seconds / N`, so the cooldown stays above that line —
    held by a test that walks **every level 1..=255**, because the first tuning of these
    numbers passed at level 1 and failed at 255. **Mind Link** (a later rung)
    force-includes co-op teammates in the snapshot at any distance, positions only. The
    pin is announced in the world as a `HELD` plate: an affordance you cannot read is one
    you will not use, and the opening expires.
  - *Design note for whatever comes next:* every class now earns something on the
    overworld, so `no_class_walks_the_overworld_with_nothing` is back to covering the
    whole roster with no carve-out.

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
- [ ] **EC-2 — Town vendors: power goods + class hires (the chit sink).**
  - 🟡 *The first vendor is open:* **The Apothecary** (the Market Tiers district, which was
    a "stalls open in M1" notice) stocks the lowest-tier basics for chits — a heal, a
    Barrier, a Regen, and a way home — over `GET /v1/vendors/apothecary` +
    `POST /v1/vendors/apothecary/buy`. The purchase is atomic (chits leave and goods
    arrive in one transaction, so a failed buy can never bill for nothing), the price
    table **is** the stock list (a client cannot buy off-menu by naming an item), and the
    shelf shows what you cannot afford *before* you spend the keypress. Deliberately
    cheap: a player who died with nothing can walk back out equipped.
    `MELD_SHOP`/`?shop` opens it for screenshots. NPC
  vendors in Last City that sell genuinely powerful things — the deliberate
  **chit sink** that makes chits worth chasing — and that **sell class unlocks**
  (you "hire" a recruit to unlock a class, feeding CL-1). Distinct from player
  stalls (EC-1): curated, always-available, chit-priced. Add vendor inventory
  config + purchase HTTP.

---

## Epic MS — Meld skills & harvesting

The persistent non-combat progression (GDD §4.1). Three skills exist and persist
XP; harvesting exists but is instant.

- [ ] **MS-1 — Finish & flesh out the Meld skills.**
  - 🟡 *The crafters earn something for walking around:* every other class had an overworld
    perk that scales with run level — the Explorer's lantern and map, the Hunter's
    prey-sense, the Shifter's Shift-sense, the Psyker's threat-sense, the Resonant's
    walking regen, the Phoenix Guard's bulwark — and the two PROFESSION classes, whose
    whole identity is what they do BETWEEN fights, had none. Smithwright: *Prospector's
    Eye*, *Efficient Setup*, *Travelling Forge*, *The Long Shift*. Keeper: *Forager's
    Path*, *Green Thumb*, *Rooted Ground*, *The Whole Vein*. Each reads only its own
    trade's materials (`ore` vs `reagent`, off the material registry) and is
    force-included in that player's snapshot rather than widening the shared interest
    cull. **Remaining:** the perks are passive; the crafter *actions* a bench cannot
    already do (field repair away from an anvil, a tonic without a still) are still open.
  - 🟡 *Recipes are real:* crafting was ONE hardcoded recipe that credited every craft to
    Forging regardless of what it made. There is now a recipe registry (seven recipes:
    six potions + the Town Portal), `POST /v1/crafting/craft {recipe}` runs any of them,
    `GET /v1/crafting/recipes` lists them with inputs, and each credits the skill it
    actually belongs to — **a potion credits Alchemy**. Bring **Forging/Smithing,
  Alchemy, and Mercantile** to real depth: recipes, gear crafting with stat
  variance, gem/materia synthesis + socketing, durability repair scaling with
  Forging level, and the mercantile tax/stall-gate effects. UIs live in Last
  City's Forge & Alembic. Spec: [`behaviors/meta-progression.md`](behaviors/meta-progression.md)
  §4.1 + [`interfaces/http-api/crafting-meld.md`](interfaces/http-api/crafting-meld.md).
  - 🟡 *The Forge is open (Forging side):* `POST /v1/crafting/forge` makes a piece of
    gear for a chosen slot + class, where **Forging level is the lever on both reach and
    quality** — it sets the tier a smith can work at (`forgeable_tier`) and how tightly
    the stat rolls (`variance_at`: an apprentice is erratic, a master dependable).
    `POST /v1/vault/gear/:id/reroll` buys **another draw on a piece's affixes** (stats
    untouched — a smith sells a chance, not a better item), gated behind
    `reroll_min_forging_level`. `POST /v1/vault/gear/:id/repair` buys back max durability
    a death chewed, restoring more per repair the better the smith
    (`repair_points_per_forging_level`) and billing only for what it actually restored —
    `GR-2`'s repair sink. Crafted gear is **insured**, and never a unique or set piece:
    those are chased, not made. All three are atomic (a smith who cannot pay keeps their
    materials) and credit Forging XP. Knobs in `[forge]`.
  - 🟡 *Combat drops have a sink, and Mercantile has an XP source:* felling a creature
    banked one of five **combat drops** that nothing could spend — no recipe named them,
    the Forge treated every material as interchangeable, no vendor bought anything.
    Materials are now a **registry with a class** (`meld_proto::materials`: `reagent` /
    `ore` / **`trophy`** = the combat drops, plus a tier per biome band), which is what
    lets a recipe or a vendor ask for a monster part specifically. On top of it:
    a **trophy potion line** (six recipes keyed on monster parts, each a step up its
    effect's own dose ladder via `ConsumableDef::potency`, capped by a Quintessence that
    takes one part from all five biomes); **trophies as the Forge's catalyst** — the
    Forge now needs an *ore* for the body and takes an optional *trophy* that buys
    `catalyst_tier_bonus` tiers past the smith's own reach and the epic affix pool, so
    **levelling raises the floor and monster parts raise the ceiling**; **permanent level
    gates on recipes** (`RecipeDef::min_level`, refused `403` naming the missing level);
    and **the Broker** (`GET /v1/vendors/broker`, `POST /v1/vendors/broker/sell`) — an
    NPC that BUYS any material at a Mercantile-scaled floor price, which is Mercantile's
    first XP source anywhere in the game. Knobs in `[material]`, `[forge] catalyst_*`,
    `[consumable] potency_per_step`, `[meld] mercantile_xp_per_sale`; new economy source
    **S3** in [`behaviors/economy.md`](behaviors/economy.md). Design of record:
    [`proposals/crafting-and-professions.md`](proposals/crafting-and-professions.md).
  - 🟡 *The smelt line — Forging finally has a craft ladder:* Forging had **one** recipe
    in the entire game (the Town Portal) against Alchemy's thirteen, and the Foundry's
    **Smelter** caste had no mechanic at all. Raw ore is now volatile: a fourth material
    class **`refined`** (one form per ore, each in its ore's own band so smelting cannot
    launder shallow material into deep gear), five `forging` smelt recipes at **two raw
    for one refined**, and **the Forge builds from refined stock** — refusing raw ore
    with a message naming the smelt to run. A Smithwright's pipeline is now
    `harvest ore → smelt → forge`. The `min_level` ladder rises by band (1/2/4/6/8),
    which is the decision: ore you cannot yet work is ore worth banking. Refined stock
    out-prices its ore at the Broker (`[material] sale_refined_mult`) because a Smelter's
    labour is in it.
  - 🟡 *The Forge & Alembic is open (the client half):* crafting was HTTP-only — every
    recipe, the anvil and the Broker were unreachable from the game. The district in Last
    City now opens a real panel: the recipe book from `GET /v1/crafting/recipes` with the
    cursor on a row, **have/need per input** (`1/2 dune_iron` is the whole answer to
    "what am I missing"), a locked row naming the level it wants, and ENTER to craft. The
    **anvil** line cycles the slot with `[S]`, arms a trophy quench with `[C]`, and forges
    with `[F]`, spending the deepest refined stock in the Vault rather than making anyone
    type a material name. Every refusal comes back in the server's own words. Screenshot
    flag: `MELD_FORGE` / `?forge`.
  - 🟡 *The counter turns around, and the smith takes work in:* the last two HTTP-only
    corners of the economy are reachable. **Selling** is the Requisition counter viewed
    from the other side — `[B]` flips it, and the sell list is the Broker's quotes
    **intersected with what the Vault actually holds**, richest stack first, because a
    price for something you do not carry is noise. **Reroll and repair** are the smith's
    two services on a piece you already own, so the anvil keeps one on a **bench**:
    left/right walk the Vault, `[R]` buys another draw on its affixes (spending the
    deepest refined stock, as `[F]` does), `[P]` buys back the max durability a death
    chewed off. The bench index is taken modulo the Vault, so a stale cursor left by a
    sold or lost piece cannot index out of range. Both replies come back through the
    same line every other refusal uses — a re-drawn affix list, or what the mend cost.
  - 🟡 *A smith takes only the work the tier allows, and charges by depth:* both
    services were tier-blind and flat-priced. **Repair is now insured-only** — insured
    is the only tier that erodes (yours forever, a little less whole each death), so
    `standard` gear is refused with "never wears down" and `ephemeral` with "burns when
    you reach the city". **Reroll** takes `standard` or `insured` but not `ephemeral`
    (a re-draw that burns on the walk home is chits into a hole), and its material cost
    now **climbs with the piece's tier** (`[forge] reroll_material_per_tier`): re-drawing
    a deep item is a bigger job than a starter blade. The server computes each piece's
    cost and it rides the gear row as `reroll_cost`, so the client advertises the real
    number without owning the formula — and only advertises the keys the tier can
    actually take. **Ownership never moves**: both calls act on gear the caller already
    owns, asserted over the wire.
  - 🟡 *The forge goes into the field, and a smith becomes a service:* crafting only
    existed in Last City, so a profession was something you did between dives rather than
    a role in one. A smith who **carries ore** can now raise a **field station**
    (`run.build_station`) where they stand — an explicit menu choice in the Map column,
    like the Town Portal, because it spends what you gathered. Once it stands it is a
    place in the world (`station:smith:<jobs>`), and **anyone** standing at it can ask
    for work (`run.smith_request` → `run.smith_result`): the STATION OWNER's Forging
    level is the skill the job is done at and they take the XP, while the piece is always
    the requester's own. **Ownership never moves** — structurally, not by rule: every
    Vault call is scoped to the requester's player id, so a station cannot reach into
    anyone else's gear. Finite `station_uses` keep the city anvil the cheaper place to
    work in bulk. The world half is pure (`Arena::place_station` / `station_at` /
    `spend_station_use` — one bench to a spot, same elevation, within reach); the DB half
    runs off the tick in `flush_smith_jobs`, so the loop never parks on Postgres. Knobs:
    `[forge] station_min_forging_level`, `station_ore_cost`, `station_uses`,
    `station_radius`.
  - 🟡 *Smithing is a rhythm, not a purchase:* every service was an instant
    transaction — press the key, pay, done — so a master smith was someone who had
    clicked more, not someone who was good at it. Working metal is now a **heat**: the
    bar is **red**, a marker sweeps it, and each blow has one **yellow** band to strike
    on. Quality is the blows that landed, and it is what the work is worth — a flawless
    heat rolls a re-draw from the **epic** affix pool (the same reach a trophy catalyst
    buys, paid in skill instead of monster parts), a missed one from `common`; a repair
    gives back between `repair_quality_floor` and all of the smith's reach.
    **Difficulty rides the piece MINUS the smiths**: a deeper item takes more blows on a
    narrower band at a faster sweep, while the smith's own Forging level and every other
    smith in the party widen the yellow and slow it down again — which is what makes
    bringing a second smith worth a party slot. The schedule is the **server's** (seeded,
    pure, in `meld_world::tempo`) and so is every grade; a client draws the bar and
    reports where the marker was, and blows past the last one are ignored, so spam can
    neither raise nor lower a heat. Both surfaces use it — the city anvil and a field
    station, same message, same rules — and a smith who walks away mid-heat is graded on
    what they actually struck. Knobs in `[tempo]`.
  - 🟡 *A smith can put a temporary edge on your kit:* the third service, and the
    field forge's own — `enhance` sharpens a piece a hero is **wearing** for the rest of
    the dive, scaled by the heat's quality. It is deliberately **never a Vault write**:
    the bonus lives in the run and dies with it, so a temporary buff cannot become a way
    to launder power home, and it is worth asking a smith for on the way *in*. Kept apart
    from the gear mirror so re-equipping cannot wipe it. Knobs in `[forge] enhance_*`.
  - 🟡 *The Keeper gets the same idea, as a cook:* the Open Flower's half of field
    crafting. An **alembic** is raised from **reagents** you carry and gated on
    **Alchemy** (`station_min_alchemy_level`), and brewing at one is a **cook** — the same
    graded bar, at the **recipe's own level** instead of a gear tier — where quality buys
    **extra doses** (`[tempo] cook_bonus_doses`): a good cook feeds more people from the
    same reagents. A forge cannot cook and a still cannot mend; the bench you are standing
    at decides what may be asked of it, and its owner's skill is what the work is done at.
    The Map column now offers both benches, each naming the stock it wants.
  - 🟡 *A bench is a commitment, and the boon is a button:* raising one was
    instant, which made it a thing you dropped while running rather than a place you
    chose. Setup and teardown are now **channels** (`station_setup_ms` /
    `station_teardown_ms`) that break on movement, a battle or `[E]` like every other
    channel, and they ride the same progress bar; the stock is spent up front, so an
    interrupted build costs you the materials. Packing up is the owner's alone and hands
    back `station_teardown_refund` of **the same stock it was built from** (the bench
    remembers). The temporary boon moved out of the bench UI onto its own **prompt and
    touch button** — `[N]` for a smith's edge or a Keeper's tonic — because it is a
    one-press favour, not a screen to open.
  - 🟡 *A set-up still is somewhere to rest:* an alembic radiates a **regen field**
    (`alembic_field_radius` / `alembic_regen_per_sec`) that stacks with the Resonant's
    perk, so a party with no healer has somewhere to stand; and its Keeper can pour a
    **tonic** — the still's answer to the forge's edge, spread across the whole party as
    +atk/+def/+regen scaled by the cook, for this dive only.
  - 🟡 *The two profession classes are real:* `smithwright` and `keeper` are
    playable, with their orders' own six-rung ladders from the lore
    ([`lore/factions.md`](lore/factions.md)) — Indentured Extractor → Master of the
    Foundry, Sprout → Terra. A Smithwright is a front-line support (staggering hammer,
    party Barrier, and a buff that makes somebody *else* hit harder); a Keeper is a mender
    whose damage rides **Mnd** and whose two attacks buy time rather than kills. Both are
    **earned**: forge a piece rather than finding one, or work a node dry — the two things
    those orders actually recruit on. `[player.smithwright]` / `[player.keeper]` stats,
    `[smithwright]` / `[keeper]` kits. They are also what the station easing counts now:
    a second Smithwright at a forge is a second pair of hands, which is the party-slot
    payoff the professions design promised. *No sprites yet — both fall back to the
    Explorer's until the art lands.*
  - **Remains:** gem/materia synthesis + socketing (no socket model exists yet); the
    mercantile tax / stall-gate effects (want `EC-1` stalls first);
    and the crafting-depth layers the proposal scopes: recipe *discovery*,
    *experimentation* (the volatility gamble on smelting), and the **maker's mark** that
    gives a master crafter a reputation instead of a spreadsheet.
  - 🟡 *Trophy supply tracks the fight:* a trophy was a flat **one per encounter** at any
    depth against any pack, while chits in the same roll scaled with both — so the new
    crafting inputs had no supply curve. `CreatureLoot` now carries a `material_qty`
    scaled by pack size × distance band × the elite/gatekeeper spike, drawn **without**
    RNG (so it cannot shift the gear roll that follows it in the same stream, and a
    crafter can plan a hunt). `[loot] material_per_creature`, `material_qty_per_tier`.
  - **Non-combat classes:** answered in the proposal — **no** (hero levels reset every
    dive and a party slot is a combat slot; professions belong on the permanent Meld
    ladder). What to build instead: **profession rank titles**, plus gathering **yield
    lenses on the `[perks]` system that already exists** — every class already has an
    overworld perk, so a gathering specialization is one more entry, not a new concept.
    The model: three material sources, each with a *find* lens (all shipping: Explorer
    node dots, Hunter creature intel, Shifter dungeon/item sense) and a *yield* lens
    (none shipping: Open Flower → reagents, Hunter → trophies, Shifter → salvage).
    Base materials stay **ungated**; the specialist multiplies yield and solely produces
    a **rare byproduct**, gated on a run-level rank rather than mere presence.
    Profession homes are settled in the lore: **Forging → The Foundry**, **Alchemy → The
    Open Flower**, and **Mercantile → no order at all, by design** (merchants are just
    merchants — its ladder is market *standing*, not a promotion).
    Forging's home order is **The Foundry** (the city's quota-driven industrial branch):
    its Extractors / Smelters / Smithwrights castes *are* the Forging pipeline, and its
    rank ladder already lands on the 1/2/5/9/13/17 rungs. Ore stays ungated until it is
    playable. The Smelters also name a missing mechanic worth building — a **raw → refined
    smelting tier**, which is where Forging's absent recipe line (it has exactly one
    recipe today, vs Alchemy's thirteen) should come from. Prior art surveyed there.
- [x] **MS-2 — Harvesting takes time in the field.** ✅ Instant `run.harvest` is now a
  **channel**: it opens a repeating gather that hands over **one unit per tick** while
  the player stands still, and a node holds **finite stock** rather than being a
  one-tap flag. Pace and stock are per **material class** (`[harvest]`) — a reagent
  patch is several quick units, an ore vein is more units at a slower pace, which is the
  rhythm that separates the two gathering professions. **Interruption is strict but
  cheap:** moving, a battle (either dragged in or opting in), `run.cancel_harvest`, or
  walking out of range ends it and loses only the tick in flight — every unit already
  banked stays banked. That turns "do I dare start" (a cliff) into "how long do I dare
  stay" (a slope), and makes partial-harvesting a real play: nibble a dangerous vein and
  come back. Also, the input layer it needed: **[E] is now the one interact key** on the
  overworld (gather, open, descend, extract at the deep portal, join) with a **contextual
  prompt** that only appears when something is in reach — replacing walk-into
  auto-collect, the static control list, and the per-action keys. A **channel progress
  bar** fills once per payout (`fill_ms` on `run.channel_started`), touch gets the same
  thing as one contextual **Interact** button, and **going home lost its hotkey**: a Town
  Portal is an item, so it is now an explicit "Return to town" row in the menu's Map
  column. Spec: [`behaviors/run-lifecycle.md`](behaviors/run-lifecycle.md)
  "Flow: Harvesting".
  - 🟡 *The Map column carries a real map:* the column was three readouts
    (distance / tier / biome). It now draws **where this dive has been** — a per-run
    memory of walked cells plus the landmarks seen on the way (portal always, chests at
    map tier 2, nodes at 3, dungeon doors while a Runner's Shift-sense is in the party),
    projected on one scale for both axes so a straight march reads as a straight line.
    It is the **Explorer's**: `explorer_map` gates both what is drawn *and* what is
    recorded, and a landmark is only learned from inside the map's own reach — otherwise
    a map would know the whole instance the moment it loaded, which is the opposite of
    exploring. Client-side by design (a memory of a walk is not world state), and blanked
    on `run.started`, because the previous run's world no longer exists.
  - *Next for this surface (stage 2):* click/tap a node directly to target it, and node
    stock in the HUD.
  - *Found and fixed on the way:* an in-flight **extraction** channel survived a battle
    start (`start_battle` never cleared it), so you could Town-Portal out *mid-fight*
    and bank the backpack — a free escape past `flee_chit_loss_fraction` /
    `flee_item_drop_chance`. A battle now breaks both channels.
  - **Why it was load-bearing.** The profession design pays a stacked specialist
    in **tempo** rather than yield — four **Keepers** (Open Flower) gather/plant faster,
    four **Smithwrights** (Foundry) build/repair/smelt/forge faster
    ([`proposals/crafting-and-professions.md`](proposals/crafting-and-professions.md)
    §2.3a). An instant action cannot be accelerated, so every profession verb needs a
    duration before either class can exist: harvest (here), smelting, building, repair,
    planting. Also the reason the field half of gathering has any tension at all — the
    creature-aggro geometry only bites while you are committed to a channel.
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
  **Density is a per-AREA question, and the fan distorts it (fixed #217):** bending a
  fixed-width corridor into an arc means anything placed *per unit of corridor* is smeared
  ever thinner outward — at r=230 the arc is ~1400 units across. Both creatures and maze fill
  now compensate (`creature_radial_lane_cap`, `maze_radial_scale_cap`: the corridor is walked
  once per corridor-width of arc). The half that is easy to miss is that **any spacing or
  adjacency check must be asked in the BENT frame**, because corridor `y` is an *angle*:
  comparing raw corridor distance is what made the forest ask for 392 trees and place 90, a
  wood that read as a field. Both checks are indexed (`SpotGrid`/`BlockGrid`), since the world
  streams outward without bound and a linear scan is quadratic in dive depth. Two invariants
  are held by test — density-per-unit-area must not collapse with depth, and no two standard
  spawns sit inside `[ai] group_radius` of each other, because a PACK is the only thing that
  may make a group or the encounter ramp promises duels and quietly hands out fives.
  **A FIELD biome joins the rotation:** the forest's ground, fauna, flora and dungeon pool at
  `field_obstacle_mult` 1.3 instead of 7.0 — grassland you can see across and a wood you
  cannot, which is the contrast the two exist for.
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
- [ ] **FS-6 — Biome hazards: let the field itself hurt you.** The overworld cannot
  currently damage anyone — all damage lives in the ATB battle — which is why
  [`lore/biomes.md`](lore/biomes.md)'s 27 biomes are unbuildable: nearly every one is
  defined by a hazard, not by scenery. Generalise the one out-of-battle damage path that
  already exists (`apply_trap_hit`, today reachable only from authored dungeon traps)
  into placed overworld hazards. **Start with H-0: make Ashfall's existing `lava`
  obstacle actually hurt** — no new placement, wire field, or art, and it answers whether
  a field that hurts is *fun* before anything expensive is built. Hazards MUST be
  rejection-sampled out of `Arena::path`'s clear tube exactly like obstacles, with the
  existing property test extended to cover them, or guaranteed extraction quietly stops
  being guaranteed. Design, primitives + sequencing:
  [`proposals/biome-hazards.md`](proposals/biome-hazards.md).

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
- [x] **CR-8 — The shallow ring is an on-ramp, in the RANDOMIZED world.** The tutorial
  is opt-in, so a returning player's real second dive is the randomized world — and
  measured there, a level-1 solo won **28%** of its opening fight. Three causes, all
  fixed, all measured rather than reasoned about:
  - **Champions had no distance gate.** Elites were gated only on "not the first
    creature of an area"; peak Gatekeepers only on `hub_safe_radius` (13 units), so a
    `gatekeeper_hp_mult` (10x) boss could stand a few paces from the hub; and seam
    Gatekeepers mount every biome boundary, the first of which is d=100. Now
    `elite_min_distance` / `gatekeeper_min_distance`.
  - **Biome rosters are not interchangeable** even though their scaling is: at the
    same distance and level, forest led with a 32 HP / atk 7 `sporeling` and tundra
    with a 120 HP / atk 14 / def 9 `glacier_maw`. `[biome_gate]` holds ashfall, desert
    and tundra outward.
  - **Bruiser armour could not be distance-balanced.** Flat `def 9` erased 75% of a
    level-1 hero's hit and ~0% of a level-40's, so the "same power budget" the balance
    file claimed was never true. Tankiness moved into HP (def 8-9 -> 3, HP ~120 -> ~155).
  - Plus `[world_scaling] onboarding_floor`/`onboarding_distance`: creature power ramps
    from 0.6 at the hub to full by d=200, and is **exactly 1.0** past it, so the deep
    game is untouched. Result: level-1 solo **83%**, level 3+ 100%, while an *ungeared*
    four-hero level-20 party still only wins 83% of deep fights at ~59s.
- [x] **CR-6 — Encounter packs: a leader and its minions (fights stop being duels).**
  Creatures were placed one at a time at `monster_spacing` gaps, so `group_around`
  almost never caught a second one and every fight was a party-of-four versus **one**
  creature — the root cause of "fights are too easy", of nobody needing heals, and of
  thin per-fight XP (the battle already sums each creature's reward, so a duel pays for
  one creature). A share of spawns now come as a **leader** (1.7× HP, 1.2× atk — a step
  above standard, below an Elite) surrounded by 2–5 **minions** (0.45× HP, 0.6× atk:
  the "one big spider with four little ones"), clustered inside `[ai] group_radius` so
  touching any of them pulls the whole pack in. `pack_mixed_chance` makes some minions a
  *different* species than their leader (mixed groups). Pack frequency and size scale
  with tier; never in the spawn section or the tutorial, so onboarding stays calm.
  Group size follows an explicit **distance ramp** (`[[encounters.group_ramp]]`), not a
  dice roll, so progression is a readable curve: **duels to 150** (learn the ATB one
  creature at a time) → **duos 150–250** (same species) → **mixed triples 250–350** →
  **quads 350–500** → **fives past 500**. Each band carries its own `chance` (some spawns
  stay solo, so a band has texture) and `mixed_chance` (species mixing ramps too). A pack
  also clears `[ai] group_radius` behind it, so two adjacent packs can't merge into an
  accidental eight-creature fight. Measured per band: 1.01 / 1.57 / 2.04 / 2.96 / 3.75
  creatures per fight, biggest 2 / 2 / 3 / 4 / 5 — up from a flat ~1.08 everywhere.
  `the_encounter_ramp_climbs_band_by_band` prints the table for tuning
  (`cargo test -p meld-world the_encounter_ramp -- --nocapture`).
  Feeds `P1-2` (combat feel) and is the placement half of the ecology epic's
  herds/alphas line.
- [x] **CR-7 — Pack AI: a pack fights like a pack (and clearing one is a decision).**
  Placement made packs exist (`CR-6`); this makes them *behave*. Three rules, all
  `[encounters]` `[TUNABLE]`s, carried into battle on `Fighter::pack_role`:
  - A **minion hits harder while its leader lives** (`pack_aura_atk_mult`) and softer
    once the pack has routed (`pack_rout_atk_mult`), so breaking the big one is felt
    immediately.
  - A **leader is shielded by its living minions** (`pack_guard_per_minion`, capped by
    `pack_guard_cap` so a big pack is never immune) — clearing the littles first is the
    other valid line.
  - **Killing the leader routs the littles**: they lose the aura and bolt when they drop
    low, announced per minion as a `routed` status so the client can show the moment the
    fight turns.
  Which order is better depends on the pack, which is the point — a pack fight is a
  *decision* instead of just more HP. Lone creatures, elites, gatekeepers and heroes are
  untouched by all three rules (tested).
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

- [ ] **EW-0 — Boss framework (extends `FS-4`).** 🟡 *A first cut of the END FIGHT ships:*
  past `[encounters] end_fight_min_distance` (d3200) one encounter becomes **three named
  bosses standing together** — peers, not a boss with a retinue — guaranteed rather than
  rolled and placed **once per instance**, because it is the thing the walk out is pointed
  at. d3200 is where it sits because `base_run_level` reaches the 255 cap at d3256: the last
  distance that is a fair fight by construction. Reports as Gatekeeper-class, so it is not
  fleeable like trash.
  - **Felling it ends the dive.** Three **insured** pieces go into the run's loot and the
    player is enqueued for an already-due extraction — the same route `west_return` uses, so
    the tested banking path carries everything home rather than a second one being written.
    Heroes come back at level 1 because levels were only ever dive-scoped; nothing resets
    them.
  - **A wood star and a clear time** land on the Vanguard posting (`record_world_end`).
    Wood is the first rung on purpose: three of the world's bosses is the current top of the
    game, and the material leaves room above it for whatever the real end is worth. A deeper
    posting later in the season keeps a star already earned — the star is for the fight, not
    the tile.
  - **The omen is deliberately unexplained:** *"Three of them fell together. The land is not
    stabilized."* What that means is `EW-4`'s to answer, and the roster it will answer with
    (Termina / Nestiph / Slake → Ometus) is already designed in
    [`proposals/endgame-bosses.md`](proposals/endgame-bosses.md).
  - *Retuned after checking it against the real curves, which found four things.* It was
    sized as `x4 of a local creature`, and at d3200 a local creature already runs ~10k HP
    and two-shots a hero — so the fight was **442 rounds and a 0.3-round wipe**, i.e.
    impossible. It is **authored** now (`set_piece`, absolute HP/atk) because a set piece is
    not a promoted spawn — and it is tuned as a **GEAR CHECK**, because the apex should
    expect really good loot. At 3900 HP / 420 atk each, a level-100 party in tier-32 insured
    gear gets a ~25-round fight surviving ~5 hits; the same party wearing nothing dies in 1.5
    hits and would need 43 rounds. **Gear buys 3.5x survivability**, and
    `the_end_fight_is_a_gear_check` pins that MULTIPLE rather than the raw numbers, so a
    retune has to preserve the shape.
    ⚠️ **`damage_floor_fraction` (0.25) bounds the attack number.** Defence can never cut a
    blow below a quarter of the attacker's power, so past `hero_def / 0.75` (~1205 with full
    tier-32 armour) **more armour buys nothing and the fight stops caring what you wear**.
    The first pass at "make it really hard" put boss attack near 1000, which floored straight
    through the gear: geared and bare both died in ~2 hits and the gate silently vanished.
    On this damage model **"harder" means more HP, not more attack** — attack past the floor
    threshold is where difficulty stops being a conversation with the player's build.
    **`"world_end"` was missing from `creature_target_profile`**, so the biggest fight in the
    game got no champion promotion and rolled its profile like trash; it is `Role` now, and
    `cap_role_hunters` leaves exactly one of the three hunting the healer — three doing it
    independently ends the fight in a round.
    **The victory handler returned early**, so felling it skipped XP, class records, hunt
    credit and ordinary drops — it paid less than a boar. It ends the run LAST now, after
    every reward has landed. Ending the run is the point: this is a roguelite, so the apex
    banks and sends you home.
    **Three insured pieces were the wrong reward alone**: `rolled_gear` cannot produce a
    unique or a set piece by design, so the apex was a worse *source* than a Gatekeeper at
    d300. `end_fight_loot_mult` (14.0, above a Gatekeeper's 9.0) makes it the best in the
    game; the guaranteed pieces are the floor, not the prize.
  - *Hardened against the build that deleted it.* **Four Psykers cleared it in 6 rounds
    against the intended 25, taking no hits at all** — because Foci ignore defence outright
    and ride Mnd, which comes from levelling rather than loot, so neither the armour nor the
    gear gate was in their path; and one Gravity Vortex plus an Anchor left each boss acting
    **0.3 times in the whole fight**, so the encounter's entire danger never happened. Two
    authored defences, neither of which touches the class:
    - **Three different wards.** Each of the three shrugs off one damage family — mind /
      physical / elemental (`end_fight_ward_mult`), merged ON TOP of its own kind's profile
      so it keeps its identity. Rotated, never rolled, so the encounter always covers all
      three and no seed hands out a free run. `no_single_damage_family_clears_the_end_fight`
      holds it. This roughly halves a Psyker stack's rate against the boss that wards it,
      and makes a MIXED party the answer.
    - **A slow floor** (`end_fight_slow_floor`, above both `status_slow_mult` and
      `psyker_anchor_slow_mult`, or the clamp would do nothing). A set piece is not a big
      creature: control can delay it, not remove it from the fight.
  - ⚠️ **What is NOT closed: action economy.** A Psyker's Dex growth puts it at speed ~247
    by level 100 against a creature's fixed ~100, so four of them take roughly **ten actions
    per boss action**. The wards and the floor cut the stack from ~6 rounds to ~11 against
    an intended 25, and the bosses now get real turns — but an all-caster party is still
    about twice as fast as the martial party the fight is tuned for, and takes little damage.
    Fully closing that means the Dex→speed curve or the class itself, which is every fight in
    the game rather than this encounter, and is not a call to make inside a set piece.
  - 🟡 *A bot harness exists but does not yet measure it* (`qa/tests/end_fight.rs`,
    `#[ignore]`d with the reason). It drives a real party, finds encounters, commands every
    hero and reports turns / HP lost / outcome — and it immediately found two things nobody
    knew:
    - **A level-1 four-hero party loses its first non-tutorial fight.** Acting on 24 of 24
      turns, it deals ~240 into one level-2 216 HP creature at d14 and still loses to
      dodges. The tutorial's on-ramp is doing far more work than anyone had measured, and a
      player who skips it is not on a gentle curve — they are on a coin flip.
    - **A bot cannot drive a Psyker.** `action: "attack"` resolves through `resolve_psyker`
      with no op, which is `hold`, so a Psyker party does nothing at all (13 turns, no
      damage). Measuring the caster stack that broke this encounter needs the bot to cast
      Foci. *Now solved* — the wire form is `cast:<kind>` / `reinforce:<kind>`, and the MCP
      harness (`mcp/`) speaks it.
  - ✅ **TUNED BY PLAYING IT, and the model turned out to be right all along.** Every pass
    above was arithmetic; `mcp/` walked a level-100 party in tier-32 gear into all three
    bosses and fought them. First measurement: **14 hero-turns, wiped, 11.5%** of their
    health removed. Then the reason, which was sitting inside the test the whole time —
    `the_end_fight_is_a_gear_check` asserted "clears in 15–35 rounds" and "a geared hero
    survives 3–8 hits" **side by side and never divided them**. 3.5 / 25 = 14%, which is
    what the fight measured. The model was correct; the arithmetic stopped one line early.
    - Retuned `end_fight_boss_hp` 3900 → **1000** and `end_fight_boss_atk` 420 → **210**.
      Incoming scales with the attack number and party output does not, so **attack buys
      fight LENGTH and HP buys the win condition** — HP alone cannot fix it, because
      survival is pinned at ~14 hero-turns whatever the bosses' health is. Measured at the
      new values: **25 hero-turns, one boss dead, a second at 77/4400, 75% removed** — the
      reference party loses, and one that focus-fires instead of spreading across three
      bosses wins.
    - **Potions are part of the reference party, and they decide it.** The starting kit
      (`starting_salves` 3 + `starting_elixirs` 1, dealt into pouches round-robin) is ~1130 HP
      of healing on a 2648 HP party — 42% of its effective health. Four runs of the same
      geared party at the new values, differing only in potion use: never drinks → defeat at
      75%; drinks on itself → victory, one hero left; pours into the lowest-% hero → defeat
      at 67%; pours where the most HP returns → victory in 37 hero-turns *or* defeat at 76%,
      **run to run at an identical seed**. That last part is the finding: ATB is real-time,
      so the order actions land in is not reproducible and the reference policy is on a coin
      flip. A competent player wins reliably, a careless one does not — the shape wanted, but
      a single run is not a verdict, and future tuning should take several.
      - A potion heals a fraction of the DRINKER's max HP, so the same bottle is 417 on the
        Phoenix Guard and 113 on the Psyker. Cross-hero pouring already worked end to end
        (engine `ally_target`, client Item row opens the ally picker); only the harness's
        policy was drinking on self.
    - `the_end_fight_is_a_gear_check` is replaced by
      **`the_end_fight_is_hard_and_winnable`**, which does the division and is calibrated
      from played runs rather than re-derived. Against the old numbers it now says: *"needs
      110 hero-turns to kill and survives 14 — 7.8x more fight than party, i.e.
      unwinnable."* It cannot silently encode a loss again.
  - 🔴 **The gear check does not exist, and no tuning can create it.** Measured: an
    **UNGEARED** level-100 party lasted **26 hero-turns to the geared party's 25**, removing
    67% against 75%. Gear bought nothing. The cause is general, not a set-piece problem:
    **a creature's ABILITY damage never subtracts hero defence.** `apply_typed_damage`
    applies the target's `damage_modifiers` and `min_damage` and stops; only the basic
    `Attack` action goes through `physical_hit`'s `atk - def`. These three bosses deal
    almost all their damage through abilities (`Cinder Lash`, `Crush Bite`, `Ash Maw`,
    `Pyre Eruption`), so armour is close to irrelevant to them — and `end_fight_boss_atk` is
    therefore not a gear dial at all.
    - This looks like an oversight rather than a design: nothing documents abilities as
      armour-ignoring, and the Psyker's Foci are called out *specifically* for ignoring
      armour, which would be a meaningless distinction if every ability already did.
    - **Option (c) has now SHIPPED as a general mechanic** — armour answers damage TYPES.
      Every `ArmorWeight` carries a physical stance (plate turns an edge and fears a hammer;
      mail defeats a cut and lets a spike through; leather soaks impact and opens to a blade;
      a robe is worst at all three and best against fire/ice/lightning/mind), folded per piece
      through `fold_damage_modifiers`, with the shape in
      `meld_proto::equipment::weight_profile` and the step size in `[armor_resist]`.
      Creatures answer the same question through `abilities::Body`. Held by
      `every_armor_weight_is_a_trade`, `the_physical_triangle_holds`, `a_body_is_a_trade` and
      `worn_resistance_reaches_the_fighter_the_engine_asks`.
      - **It does not rescue THIS fight, for a legible reason.** Measured with resistances
        live: geared 33 hero-turns / 76%, ungeared 29 / 68%, fire-warded 28 / 77% — all
        inside the coin-flip band. These bosses lead with **fire and hammers**, and plate is
        weak to blunt, so the armour a Phoenix Guard can wear is close to the worst answer
        available. Which is the mechanic working: "what do I wear against this" now has an
        answer, and for this encounter the answer is "not plate".
      - **`def`/`ward` split shipped.** `def` (Wll) is subtracted from physical damage and a
        new `ward` (Mnd) from everything elemental or psychic, in one place
        (`apply_ability_damage`) — so ability damage is mitigated at all, which it previously
        was not. Measured: the geared party went from 33 hero-turns to **43**, two bosses down
        and the third at 376/4400 (**97%**), and an ungeared run WON at 42 turns. Both inside
        the coin-flip band, which is the point: **`ward` is an attribute, so it makes LEVEL
        the elemental defence, not gear.** A **ward affix on gear** is the missing piece for
        the apex to be a loot check on its elemental half.
      - The fight is now materially closer to winnable than when it was tuned (210/1000 was
        set before ability damage was mitigated at all), so it wants re-measuring across
        SEVERAL runs before any further retune.
      - ✅ **THE GEAR GATE NOW EXISTS**, and it took two affixes to get there: **"of the
        Aegis"** (flat `ward`) and **"of the Furnace"** (extra damage DEALT of one element,
        the offensive twin of "of Warding"). Both roll off the registry pool, so adding them
        was enough — `every_affix_can_actually_roll_on_something` proves every affix in the
        game is reachable loot rather than paper. Measured on the same boss trio:
        **geared → VICTORY in 39 hero-turns with the tank untouched at 1042/1042; ungeared →
        defeat at 76%, whole party dead.** That is a qualitative difference, not the coin flip
        the earlier runs were.
      - ⚠️ **The trio is ROLLED, so "the end fight" is several fights.** A second seed drew
        Hollow Bishop / Gloamhound / Ashen Leviathan instead, and the same geared party lost
        in 16 hero-turns having removed 40% — because Gloamhound is `Body::Amorphous` and
        halves all physical damage, which a martial party cannot answer. Every number above is
        therefore *one trio*, and the apex's difficulty has a spread nobody had measured.
        A tuning pass should sample trios, or the encounter should constrain the draw.
      - **Remaining for a real apex gate:** either bosses whose damage armour can answer, or
        elemental wards that a full set actually rolls (epics roll one biome-themed
        quarter-resist; `MELD_GEAR_WARD` models a prepared set in the harness). Still NOT
        done: (a) routing ability damage through `def` — the bigger, possibly more correct
        fix, which changes every fight in the game and belongs to whoever owns the damage
        model.
  - **Remains:** the `WorldBoss` defs themselves, the raid-scale merge cap, the three-boss
    unlock gate, and the arena hook. This cut reuses the FS-4 named bosses and the ordinary
    encounter path instead. `WorldBoss` defs, raid-scale merge cap,
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

- [x] **AD-1 — Gear affixes & the loot chase (the star).** Server-rolled affixes in three
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
  - 🟡 *Shipped — the affix engine, all five classes live:* the registry
    (`meld_proto::affixes` — keys, what each twists, the name suffix it lends) with the
    numbers as `[affix]` `[TUNABLE]`s; the seeded roll in `meld-world::roll_affixes`,
    **tier-gated per affix class** (stat 0 → element 3 → ward 4 → keyword 6 → synergy 8)
    so the early game stays a legible ladder and builds bloom deep; a `gear.affixes`
    column and the wire/`GearView` field; folding in `equipped_gear_bonuses`. What each
    class *does*: **stat** → atk/def/spd; **element** → a resist, riding the
    `damage_modifiers` plumbing; **ward** → the hero starts each battle already holding
    Barrier/Regen/Evasion; **keyword** → twists one class's mechanic (Explorer banks
    Adrenaline pre-fight, Psyker gains a Focus slot) and is inert on any other class;
    **synergy** → pays out only when the ally it names is in *this* party, resolved at
    battle assembly. Items are renamed by their defining affix ("… of the Bulwark"), and
    the tooltip lists one line per affix.
  - 🟡 *Also shipped — the two chase tiers:* **uniques** (`meld_proto::uniques` — five
    authored named items, each with fixed affixes **and a drawback**, so equipping one is
    a trade rather than an upgrade; they drop **only from a reward spike** — elite /
    Gatekeeper / boss — because a chase item farmable from trash is not a chase) and
    **sets** (a piece can belong to a set; completing one pays **every hero in the
    party**, including other players' heroes in a merged raid — the only bonus in the
    game that reaches past its owner, which is what makes assembling one a group
    project). Drawbacks are floored so a build can be lopsided without being
    unplayable, and the tooltip shows a unique's cost in red right under its upside.
  - *Complete:* affix rerolling landed with the Forge (`MS-1`), and the elemental half
    with the `brand` affix (`AD-3`). Affixes, uniques, sets, damage types and rerolling
    are all live.
- [x] **AD-2 — Party synergies + surfacing.** Class-pair + affix-driven synergies; the
  party screen shows **active synergies** (the build feedback loop). Depends on AD-1 + `PT-1`.
  Three layers, all live (`meld_proto::synergies`; magnitudes in `[adventure]`):
  - **Class-pair synergies** — passive while both classes are in the party: *Fortress
    Front* (Phoenix Guard + Psyker → every hero opens each fight warded), *Blood and Balm*
    (Resonant + Explorer → party Regen), *Covering Blink* (Shifter + Resonant → back-row
    Evasion). Applied at battle assembly, the only place that sees the whole comp.
  - **Sequenced combos** — one hero's ability primes a target and a *specific* follow-up
    cashes it in inside a `combo_window_ticks` window: **Cut the Snare** (Explorer Snare →
    Shifter Backstab, +60%), **Crush the Pinned** (Psyker Gravity Well → Phoenix Guard Kinetic
    Shock, +50%), **Follow the Stagger** (Phoenix Guard Swell Strike → Explorer Frenzy, +50%),
    **Press the Slowed** (Shifter Ransack → Explorer Power Strike, +40%). Three of the four
    need *two different heroes*, so turn order becomes a party decision instead of four
    independent menus. Primers ride the existing `timed_statuses`, are consumed on payoff,
    and expire; the payoff is checked before priming so nothing primes itself.
  - **Surfacing** — `run.party` carries the active synergies and runnable combos (server
    describes them, so the words can never drift from the rules) and the party screen lists
    them: "Cut the Snare : Snare (Explorer) then Backstab (Shifter) (+60% on the payoff)".
  - Note: `PT-1` was *not* a real dependency for surfacing; back-row placement already
    exists and `Covering Blink` reads it.
- [ ] **AD-3 — Elemental affinities & resistances.** Damage-type weak/resist/immune on
  creatures/biomes; resist/convert affixes; **telegraphed** (`UX-2`). Makes biomes a
  combat *decision*. Extends [`behaviors/combat-atb.md`](behaviors/combat-atb.md).
  - 🟡 *Mostly already built, and now two-way:* creature kinds already had typed basic
    attacks (`creature_basic_attack_type`) and elemental profiles
    (`creature_damage_modifiers`), the engine already applied weak/resist/immune/absorb
    (`apply_typed_damage` → `ModifierFlag`), and the flag already reached the client. The
    missing half was that **heroes' attacks were untyped**, so a party could only ever
    *resist* an element, never exploit one. The `brand` affix (AD-1 Element class, weapons
    only — armour does not decide what your swing is) types a hero's basic attack, so a
    creature's profile now cuts both ways and the `resist` affix has an offensive
    counterpart.
  - **Remains:** biome-level affinities beyond per-kind profiles, convert affixes
    (turn damage from one element into another), and the `UX-2` telegraphing pass so a
    player can *see* the matchup before committing a turn.
- [ ] **AD-4 — The Hunt Board.** Directed combat goals (named creatures/dungeons/depth) —
  the mid-game spine; ties `CR-5` bestiary, `FS-4`, `DG`; co-op/guild hunts (`SOC`).
  - 🟡 *The light first cut ships (Phase 1 ③), and the Bounty Board is a real district:*
    the one thing in Last City that told you to come back later ("gathering contracts
    arrive in M2") is now eight posted hunts you can read, work and be paid for. A hunt
    is one registry (`meld_proto::hunts`) both sides read — server credits progress
    against `HuntGoal::credits`, the board draws its rows from the same defs — so the
    board can never advertise a condition the server does not check. Five goal kinds
    cover what a dive is actually made of: fell N of a **kind**, fell N of an
    **encounter class** (elite / gatekeeper), **reach** a depth, **extract from** a
    depth, **clear** a dungeon. Every credit is read off server-owned state (the
    carcass's own kind, the validated avatar, the run's own record) — there is no
    client-submitted progress path — and it is announced as it happens
    (`run.hunt_progress`), because a goal you cannot watch fill is a goal you forget you
    have. Progress survives death: what a dive costs you is your Backpack, not your
    standing with a board.
  - *The reward is taken at the board, not granted on completion*, so finishing a hunt
    is a reason to come **home** — `POST /v1/hunts/:key/claim` pays chits + a material
    stack into the Vault, once per account, with the claim stamp and the payout in one
    transaction so two presses cannot both be paid. Magnitudes are `[hunt]` `[TUNABLE]`s
    resolved server-side and ridden onto the wire, so a retuned reward retunes what the
    row promises. Chits minted here are economy source **S4**
    ([`behaviors/economy.md`](behaviors/economy.md)); the faucet is bounded by the size
    of the roster rather than by grinding, which is what a repeatable board would have
    to solve before it ships. Spec:
    [`behaviors/hunt-board.md`](behaviors/hunt-board.md) +
    [`interfaces/http-api/hunts.md`](interfaces/http-api/hunts.md). Screenshot flag:
    `MELD_HUNTS` / `?hunts`. Verified by `qa/tests/hunt_board.rs` (a real kill over the
    real wire → the board over HTTP), `meld-db` claim/credit unit tests, and a
    `meld-world` test holding every hunt's quarry against the creatures the world
    actually spawns — a hunt naming a creature nothing spawns is a contract that can
    never be filled.
  - 🟡 *The deep hunts pay a piece, and the quarry can be tracked:* a board that paid
    only chits paid in the currency the Broker already prints, and a hunt naming a
    creature you could not find was a goal you could not act on. Tier-3+ hunts now hand
    over a **rolled piece** — insured, at the hunt's own band, for a class you actually
    field, in a slot that class can wear — through the *same* generator the Forge uses
    (`meld_world::rolled_gear`, factored out so there is one roll path) and in the same
    transaction as the payout. Never from the **epic** pool: a champion stays the better
    *source* of a great item, and the board's promise is reliability, not superiority.
    Only the deep hunts pay it, so the board reads as a ladder.
  - 🟡 *And it tells you where to go:* every row carries a `where_to_look` line derived
    from the tables the world generates from (`biomes_of_creature` + `[biome_gate]`,
    `gatekeeper_min_distance`, `elite_min_distance`) rather than written down twice — so
    "Fell 6 Dune Wyrms" is no longer a level-1 player hunting a desert the world holds
    until d400. **A Gatekeeper was already guaranteed** (one stands in the pass at every
    biome border, on the clear path, every run) — nothing had ever said so. In the field
    the quarry of an unfinished hunt is **force-included in that player's own snapshot**
    and tagged `:quarry` (the portal/node-sense pattern, never a wider shared cull), so it
    is trackable rather than stumbled upon; a **Hunter** senses it from much further out
    (`[hunt] quarry_sense_hunter_radius`), which is the guild's whole trade. Marking stops
    the moment the hunt is finished.
  - 🟡 *And the Den posts contracts with your name on them (bounties):* the fixed board is
    a checklist everyone shares, so it has no ladder. A **bounty** is generated *for you*
    against a **hunter rank** — a persistent track (the `hunting` Meld skill) that only
    finished board work raises, so the question is "how many marks have you put down",
    not "what level is your party". **Every bounty ends in a boss fight**: one of `FS-4`'s
    ten named bosses wearing a rolled **epithet** ("Ironmaw the Unburied"), promoted by the
    contract's own power rather than the Gatekeeper constants and always affixed, so a
    deep-rank mark is worse than the door it walked past. It is sighted at a depth the rank
    has earned, in the open **or at the bottom of a descent** (where the mark *is* what
    keeps the door: the first dungeon its owner descends at or past the sighted depth
    builds its boss from the contract, so a descent contract is never also standing in the
    open) — and it stands in the world for **that player alone**: `MonsterSpawn.owner` keeps it out of every other player's
    snapshot and out of their touch check, so in co-op the party can fight it beside you
    but only you can trigger it. Contracts **expire and re-roll** (`[bounty] active_slots`
    / `window_hours`), lazily on read, so the offers are always live with no scheduler;
    only a *standing* one expires, because a felled mark is owed its reward however long
    the walk home takes. Paid at the board like a hunt — chits, the band's trophy, a rolled
    piece from `reward_gear_from_rank` up, and the rank XP, all in one transaction.
  - 🟡 *The menu grows a **Quests** column, and it appears with the Hunter:* the board is
    the Den's, so `MenuSection::Quests` is gated on owning `class_hunter` rather than
    sitting there greyed out — the menu's own rule is that it never advertises what you
    have not earned. It lists the standing contracts (mark, where, how hard, what it pays,
    how long is left) and everything settled. Reading only: the reward is taken at the
    Bounty Board, so a finished contract says so instead of handing you power mid-run.
  - **Remains (the full system):** an explicit *accept* step, bestiary ties (`CR-5`),
    co-op and guild hunts (`SOC`), reputation, and hunt leaderboard points (`AD-6`).
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
- [x] **UX-3 — A login screen you can actually log in on.** The account fields took
  no typing at all: nothing ever set `LoginFocus`, so every keystroke was discarded.
  A field is now focused on arrival, TAB reaches the fields from cold, and clicking
  one focuses it. The screen also moved onto a glass panel over a looping baked-video
  backdrop, because 12 px hint text over the live 3D overworld was unreadable
  ([`asset-pipeline.md`](asset-pipeline.md) for how a clip is baked).
- [x] **UX-4 — The controls move off the field and into a Guide column.** The
  permanent control list across the top of the overworld contradicted the HUD's own
  rule (show only what you can do *right now*); it is now the menu's fifth nav
  column, which is also where the one control that has no key — going home costs a
  Town Portal — can finally be written down. In its place, the **distance** reads
  under the minimap, so the difficulty axis sits with the reading of the ground.
  Both ride the Explorer's map perk, and everyone still has distance on the Map
  column.

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

- [x] **AX-1 — MCP over the wire protocol.** Shipped as [`mcp/`](../mcp/) (`meld-mcp`),
  a stdio MCP server that boots the whole game in-process on a `memory://` DB — no
  Postgres, no port to collide with, a fresh world per `new_game`. Tools: `new_game`,
  `look`, `walk`, `battle`, `abilities`, `act`, `auto_battle`, `interact`, `say`, `chat`,
  `wait`. Every one is a **player intent over the real wire protocol**; nothing reads
  `MazeInstance`, because a harness that reaches into the engine measures the model, and
  the model is what has been wrong every previous time. JSON-RPC framing is hand-rolled on
  `serde_json` (~60 lines) rather than adding a crate to an offline build.
  **Deliverable met:** a full dive → fight → extract loop, banking
  `forest_bloom_petal` + potions into the Vault.
  - **`MELD_START_LEVEL`** joins `MELD_END_FIGHT` / `MELD_GEAR_TIER` as a DEV/QA override,
    surfaced as a `new_game` argument. Deep content is authored for ~level 100 and PG-2's
    hubs are inert, so the only level it could previously be observed at was 1 — the one
    level it was never tuned for.
  - **It found a latent bug on its first geared run**: starting HP came from the class's
    level-1 `base_hp` while the ceiling came from `max_hp_at_level`, so a party departing
    at level 100 opened the dive at **52 of 1042 HP**. Harmless while every dive starts at
    level 1, and live the moment PG-2 lands. Both now go through
    `meld_run::starting_hp`, held by
    `a_hero_starts_a_dive_at_full_health_whatever_level_it_leaves_at`.
  - **The clock does not stop while an agent thinks.** A fighter awaiting input stops
    filling its own gauge; everything else keeps ticking. 33 seconds spent composing the
    next tool call is an entire boss fight, resolved by the 15-second auto-act — which is
    how the first end-fight run was measured with heroes that never acted. Anything being
    measured must happen inside ONE tool call. Widening `turn_timeout_ms` does **not** fix
    it: a longer window is strictly more enemy turns.
  - *Observed, not chased:* a successful extraction reports
    `max_distance_reached: 0` on `run.member_result` for a run that reached d6.
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
