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
  `focus_slots:<n>`, `focus:<kind>:<stacks>`, `pack:<leader|minion>`.
  A token nothing renders is a token that does not exist to the player: `pack:` drove
  combat (pack rout) for a long time without reaching the client, so a leader at 1.7x HP
  and its minion at 0.45x — the same species, 3.8x apart — drew at identical size and read
  as a bug.
  Prefer this over adding wire fields for slice-scoped additions.
- **Distance is the difficulty axis.** `tier(d)=floor(d/100)`, `mlevel(d)=max(1,round(d/12.5))`,
  `stat_mult(d)=(1+d/500)^1.25`. All threshold checks use the **floored integer** distance.
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
| **Adrenaline** | Explorer mechanic: a banked resource (0…`explorer_adrenaline_max`) that basic **attacks** build and **skills** spend. A Explorer skill is rejected unless its cost is banked. Rides the wire as `adrenaline:<cur>` + `adrenaline_max:<max>`. |
| **Focus / Manifestation** | Psyker mechanic: a Psyker has N Focus slots (grows with level); each holds a persistent Manifestation that fires every Psyker turn. Each turn it also casts / reinforces / revokes one. |

**Classes** (per-hero; stats in `[player.<key>]`, kit in `meld-battle`):

- **Explorer** — the **default** class: the Explorers map and anchor the unstable world
  ([`docs/lore/factions.md`](docs/lore/factions.md)). Its kit is **tempo and stability**
  rather than burst — Trailblaze, Field Dressing (L2), Read the Ground (L5), Set Anchor
  (L9, party Barrier), Safe Passage (L13, party Regen), A World Known (L17, fills every
  ally's gauge). See `Battle::resolve_explorer_kit`.
- **Hunter** — the martial baseline (disposal-of-dangerous-creatures guild).
  Front-line bruiser with the standard Attack / Defend / Item / Skill menu. It has no resource
  until it earns one: each basic **Attack** banks **Adrenaline**, and **every** skill SPENDS it —
  Power Strike (heavy hit), Second Wind (L2, self-heal), Snare (L2, damage + ATB-gauge drain),
  Frenzy (L3, biggest hit, biggest cost). A skill is rejected unless its Adrenaline cost is banked.
  See `Battle::resolve_explorer` (the Adrenaline resolver, shared with nothing else).
- **Psyker** — psychic channeler. Instead of the martial kit it manages **Foci**: Gravity Well
  (armour-ignoring damage tick), Kinetic Aegis (grants **Barrier**), Mind Spike (L3, stronger),
  Temporal Anchor (L5, drains the enemy's ATB gauge). See `Battle::resolve_psyker`.
- **Resonant** — healer. Innate **Regen**, plus ally-auto-targeting skills: Transfuse (heal an ally,
  paid from its own HP), Regen Boon (grant Regen), Ward (grant **Barrier**). See `Battle::resolve_resonant`.
- **Shifter** — rogue / fortune-explorer ("Runner"). Fast, fragile front-line skirmisher and the only
  class with innate dodge (base Dex clears the dodge floor). Str/atk-driven kit: Backstab (heavy strike
  that pierces most armour), Flicker (L2, self **Evasion** blink), Ransack (L3, damage + drains the
  enemy's ATB gauge). See `Battle::resolve_skill` (the `flicker`/`backstab`/`ransack` arms).
- **Smithwright** — **The Foundry's** builder, and the first of the two profession
  classes (MS-1). A front-line support: Hammer Fall (a staggering blow with the tool
  itself), Quench (L2, self **Barrier**), Plant the Bulwark (L5, **party** Barrier),
  Tempering Blow (L9, an ally's atk for the fight), Slag Spray (L13, all-enemy,
  armour-ignoring), The One True Forge (L17, party heal + Barrier). Out of combat it is
  the class that **raises the field forge**, and a second Smithwright in the party makes
  the anvil's rhythm easier for whoever is working it. See `Battle::resolve_smithwright`.
- **Keeper** — the **Order of the Open Flower's** grower, the other profession class. A
  between-fights mender: Thornlash (damage + gauge drain), Poultice (L2, heal + Regen),
  Bloomfield (L5, **party** Regen), Root Snare (L9, damage + a long wait), Vital Draught
  (L13, Barrier + Regen), Terra's Gift (L17, party heal + Barrier + gauge). Its damage
  rides **Mnd**, not Str. Out of combat it **raises the alembic**, whose regen field is
  the only rest a party without a Resonant gets. See `Battle::resolve_keeper`.
  *Neither class has its own sprite set yet — `class_frames` falls back to the Explorer's
  until the art lands.*
- **Phoenix Guard** — the Last City's **anti-undead** order. The tankiest, slowest class
  (most HP + armour, no dodge), and every damaging ability of theirs hits **undead**
  `phoenix_guard_undead_mult` harder. Its ladder is the order's rank ladder: Silvered
  Strike (Initiate), Rite of Rest (Purifier L2, self **Barrier**), Holy Censure
  (Exemplar L5, zeroes the gauge), Purging Light (Luminary L9, **all-enemy**), Unbroken
  Vigil (Redeemer L13, **party** Barrier), Eradication (Apotheosis L17, an execute that
  scales with the target's missing HP). See `Battle::resolve_phoenix_guard`.
  *The kinetic/oar kit it used to carry belongs to the **Order of the Iron Hull**, a
  future monk class whose `iron_hull` key is reserved.*

**Abilities are one registry.** [`meld_proto::skills`](shared/meld-proto/src/skills.rs)
owns every ability's key, name, owning class, unlock level, **org rank** and
**description**. The server gates on it, the battle menu builds its rows and tooltips
from it, and the party screen lists each hero's ladder from it — so a kit is defined
once. Unlock levels follow the orders' rank ladders (1 / 2 / 5 / 9 / 13 / 17), which is
what makes levelling read as promotion.

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
  (server rejects a locked skill in `resolve_skill`; client greys the menu row). Second Wind L2,
  Mind Spike L3, Temporal Anchor L5, Regen Boon L2, Ward L3; everything else L1.
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
