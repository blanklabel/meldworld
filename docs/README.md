# MELDWORLD docs

All design, spec, planning, proposal, and status docs for MELDWORLD. (The only
markdown that stays at the repo root is [`AGENTS.md`](../AGENTS.md)/`CLAUDE.md` —
the agent convention-of-record — and the repo [`README.md`](../README.md).)

Read [`AGENTS.md`](../AGENTS.md) first if you're an agent; it's the map. This index
is the human table of contents.

## Start here

| Doc | What it is |
|---|---|
| [`ROADMAP.md`](ROADMAP.md) | **The live worklist** — what we're building next, as checkable items with stable IDs. Tick boxes here when you land work. |
| [`GDD.md`](GDD.md) | Game Design Document — the vision. Source of truth for *intent*. |
| [`CANON.md`](CANON.md) | Authoritative resolutions of GDD gaps: names, enums, formulas, `[TUNABLE]`s. **Wins over the GDD on conflict.** |
| [`BUILD-PLAN.md`](BUILD-PLAN.md) | Milestones (M0–M…) and team/task decomposition (T1–T6). |
| [`spec-index.md`](spec-index.md) | Index of the behavior + interface specs below. |

## Spec — how it must behave

- [`behaviors/`](behaviors/) — observable behavior: [world generation](behaviors/world-generation.md),
  [verticality](behaviors/verticality.md), [dungeons](behaviors/dungeons.md),
  [run lifecycle](behaviors/run-lifecycle.md), [combat/ATB](behaviors/combat-atb.md),
  [economy](behaviors/economy.md), [meta-progression](behaviors/meta-progression.md),
  [disconnect handling](behaviors/disconnect-handling.md),
  [async interaction](behaviors/async-interaction.md),
  [endgame & seasons](behaviors/endgame-seasons.md),
  [the Hunt Board](behaviors/hunt-board.md).
- [`interfaces/`](interfaces/) — wire/data contracts:
  [HTTP API](interfaces/http-api.md), [realtime protocol](interfaces/realtime-protocol.md),
  [data models](interfaces/data-models.md).
- [`edge-cases/`](edge-cases/limits.md) — the consolidated table of every numeric
  limit, cap, and timeout.
- [`lore/shifting-lands.md`](lore/shifting-lands.md) — the world fiction + the **Shift**.
  The **world model** it drives — persistent player-seeded worlds, the Shift, and
  Structures/anchors — is authoritative in **[CANON §W](CANON.md)**.
- [`lore/biomes.md`](lore/biomes.md) — the **master biome registry**: all 27 biomes in
  five categories, verbatim design intent and the source of record. Our five shipped
  biomes are its **Pale Echoes** category. Written in tabletop terms — what the engine
  would need before any of it is buildable is
  [`proposals/biome-hazards.md`](proposals/biome-hazards.md).
- [`lore/factions.md`](lore/factions.md) — the **orders** a hero belongs to: source of
  truth for faction names and the six-rank ladders the ability registry is generated
  against.
- [`lore/city-institutions.md`](lore/city-institutions.md) — the Last City's *non-class*
  organisations: government, Sentinels, Archivists, Artificing, Messengers, Wall Defense
  Force, infrastructure, and the criminal syndicates.
- [`design-notes/`](design-notes/worldgen-research.md) — non-normative design
  rationale (e.g. the worldgen research survey).

## Proposals — designs not yet folded into CANON

- [`proposals/last-city.md`](proposals/last-city.md) — **Last City**, the persistent
  social/economic hub (M0 shipped; M1–M3 = roadmap epic **LC**).
- [`proposals/crafting-and-professions.md`](proposals/crafting-and-professions.md) —
  **Crafting depth & the non-combat class question** (the rest of epic **MS**): the
  material registry (`reagent`/`ore`/`trophy`), the trophy potion line, trophies as the
  Forge's catalyst, permanent recipe level gates, and the Broker — plus the answer of
  record on non-combat classes (**no** — professions belong on the Meld ladder; build
  rank titles and gathering yield-lenses on the existing `[perks]` system instead),
  the soft-gate/hard-byproduct model for who gathers what, the open "nobody mines"
  gap, and a survey of the prior art (SWG, FFXIV, EVE, BDO, UO, Mabinogi, PoE).
- [`proposals/server-scaling.md`](proposals/server-scaling.md) — lifting the
  authoritative server's concurrency ceiling (interest indexing → sim/IO split →
  world sharding) without breaking the single-owner/no-locks loop; includes a
  forward-compat analysis of overworld hazards, sieged player towns, and the Shift.
- [`proposals/parties-and-guilds.md`](proposals/parties-and-guilds.md) — **Co-op
  groups & guilds** (epic **SOC**): durable player groups (the Lobby made
  persistent), guilds chartered in the Last City, ranks/permissions, a shared guild
  vault with an immutable audit log, composed-heraldry flags displayed over avatars
  and in chat, and guild chat — the full `SOC-1`/`SOC-2` design.
- [`proposals/living-ecology.md`](proposals/living-ecology.md) — **The living
  ecology** (epic **CR**): creatures that eat, sleep, roam territory, and **breed**;
  herds with alphas that split and wage **turf wars** (wounds regenerate, deaths drop
  loot on the ground); **flora that grows** to feed the food web; **materials** for
  crafting; and the `CR-4` **sim budget** (LOD + caps + determinism) that keeps it
  all off the authoritative loop.
- [`proposals/building-and-sieges.md`](proposals/building-and-sieges.md) — **Building
  & sieges** (epic **BD**): **harvest** wood + stone, enter a **builder mode** to place
  the one `Structure` primitive (walls/stash/workshop/portal), **build upward**
  (buildable verticality extending D24), cluster them into **towns**, and plant
  **anchors** that pin ground against the Shift **while defended** — the "anchor and
  defend" loop. Creatures **siege** what you build; **hire NPC garrisons** to hold it
  while you're offline. Shares the `CR-4` budget; persists as the §W5 event log, so it's
  the epic most gated on `SC-3`.
- [`proposals/core-loop-and-personas.md`](proposals/core-loop-and-personas.md) — **Core
  loop & personas** (design framing): do the specced systems compose into one loop for
  four kinds of player — **Adventurer / Builder / Gatherer-Crafter / Merchant**? Maps the
  interlock economy, fixes the two loops that don't close (the Crafter's fun, the
  Builder's income), and shows how the hidden bosses give every persona an apex.
- [`proposals/endgame-bosses.md`](proposals/endgame-bosses.md) — **Endgame bosses**
  (epic **EW**): the seasonal ladder — **Termina / Nestiph / Slake** → the true end boss
  **Ometus** (the forgotten evil behind the Shifts) — plus hidden bosses **All-Father**
  and **Terim** that reward the non-combat personas. Maps the roster onto the bestiary
  biomes; apex of `FS-4`.
- [`proposals/adventure-depth.md`](proposals/adventure-depth.md) — **Adventure depth**
  (epic **AD**): the Adventurer's retention layers — a deep **gear/affix** loot chase,
  **party-synergy** builds (composition + gear, *not* stat trees, since you run four
  heroes), **elemental affinities**, a combat **Hunt Board**, **keystone** modifiers, and
  a seasonal **leaderboard suite**.
- [`proposals/dungeons.md`](proposals/dungeons.md) — **Designed dungeons** (WG-1
  full / DG epic): authored, separately-instanced set-piece dungeons (the
  `meld-dungeon` authoring + validation foundation shipped). Built as *content within
  a world-actor* per the SC/§W model — ephemeral, per-entry-fresh, never persisted.
- [`proposals/biome-hazards.md`](proposals/biome-hazards.md) — **Biome hazards** (FS-6):
  the gap analysis between [`lore/biomes.md`](lore/biomes.md) and an engine where the
  overworld cannot hurt you. Sorts 27 biomes into five engine primitives, names the
  invariant most at risk (the guaranteed clear path), and proposes the cheapest order —
  starting with making Ashfall's existing lava actually hurt.

*Graduated out of proposals (shipped + now specced): verticality →
[`behaviors/verticality.md`](behaviors/verticality.md) + CANON D24; worldgen (WG) →
[`behaviors/world-generation.md`](behaviors/world-generation.md) +
[`design-notes/worldgen-research.md`](design-notes/worldgen-research.md).*

## Reference

- [`asset-pipeline.md`](asset-pipeline.md) — **generating art for the HD-2D
  renderer**: surfaces-vs-props, why we want *single seamless tiles* (not Wang/Godot
  tilesets), side-view walls, PNG + 8-direction sprites, and which PixelLab
  generator to use. Read before a tile/sprite generation session.

## What's built vs. next

There's no static status snapshot — they rot. Trust the **code** for what's live
and [`ROADMAP.md`](ROADMAP.md) for what's next. (The GDD also predates shipped
features like Last City and verticality; where it disagrees with the code, the
code wins.)
