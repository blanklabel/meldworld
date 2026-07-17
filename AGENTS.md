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

This repo is currently an **architecture spike**: a thin but honest vertical slice
on the real stack. See [`SPIKE-NOTES.md`](SPIKE-NOTES.md) for exactly what is
implemented vs deferred (chunk streaming, Gatekeepers, economy/meta, seasons).

## Spec hierarchy — read this before changing behavior

Behavior is specified top-down; **on conflict, the higher doc wins**:

1. [`GDD.md`](GDD.md) — the game design vision (source of truth for *intent*).
2. [`CANON.md`](CANON.md) — authoritative resolutions of every gap/ambiguity/name in the GDD. **CANON wins over GDD.** Names, enums, formulas, and `[TUNABLE]` constants live here. If you're implementing a rule, find its CANON §/D-number.
3. [`behaviors/`](behaviors/) + [`interfaces/`](interfaces/) — the spec: observable behavior (behaviors) and wire/data contracts (interfaces). Each references its CANON source.
4. [`BUILD-PLAN.md`](BUILD-PLAN.md) — milestones (M0–M…) and task IDs (T1–T6) the code is executed against.

When you add or change a gameplay rule, cite its spec (`combat-atb.md`, `CANON §B`, etc.) in the code comment, as the existing code does.

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
make play         # boot throwaway Postgres + server + browser client, then open the printed URL
make play-native  # same, but the native desktop window
make smoke        # headless: drive the whole loop through the real client netcode (exits 0 on victory)
make server       # server only
make test         # the Postgres-backed QA suite
make stop         # stop the local server (Postgres left running, reused across runs)
make help         # list every task
```

`make play` builds the wasm client and has the **server itself serve it**, so the
whole game lives at one URL (`$MELD_ADDR`, default `http://127.0.0.1:18090`) — no
proxy, no second port. It needs `trunk` (`cargo install trunk`) and the wasm target
(`rustup target add wasm32-unknown-unknown`); everything needs a local Postgres
(`initdb`/`pg_ctl`/`createdb` on PATH).

Build your **party of four** on the Join screen (keys 1–4 cycle each slot's class),
or preset it: `?party=squire,psyker,resonant,squire` / `?class=psyker` (lead) in the
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
  `class:<key>`, `barrier:<n>`, `regen:<n>`, `focus_slots:<n>`, `focus:<kind>:<stacks>`.
  Prefer this over adding wire fields for slice-scoped additions.
- **Distance is the difficulty axis.** `tier(d)=floor(d/100)`, `mlevel(d)=max(1,round(d/12.5))`,
  `stat_mult(d)=(1+d/500)^1.25`. All threshold checks use the **floored integer** distance.
- **Git worktree layout.** Work happens in worktrees under `.claude/worktrees/`. Branch off
  `main`; commit/push only when asked. Co-author trailer: `Co-Authored-By: Claude <noreply@anthropic.com>`.

## Combat & class taxonomy

Use these terms consistently in code, comments, and UI.

| Term | What it is |
|------|-----------|
| **Run** | One player's ephemeral dive (`PlayerRun`): run-level, XP, backpack, result. Ends on extract or death. |
| **MazeInstance** | One seeded world + its party's runs. Ephemeral; discarded on close. |
| **Area** | A stretch of the seeded corridor in one biome, holding several creatures + a portal. Areas trend larger with depth. |
| **Party** | A player's battle team of up to `party_size_per_player` **heroes of mixed classes** (default: Squire + Psyker + Resonant + Squire). Each hero is commanded by its own class's menu. |
| **ATB** | The 100 ms-tick combat: each fighter's gauge fills by `speed_stat/gauge_fill_divisor`; a turn fires at gauge 1.0. Players get a 15 s window then auto-act. |
| **Barrier** | Temp HP: a pool that absorbs damage **before** HP and decays a fixed amount at the start of the holder's turn. |
| **Regen** | HP restored at the start of the holder's turn. |
| **Focus / Manifestation** | Psyker mechanic: a Psyker has N Focus slots (grows with level); each holds a persistent Manifestation that fires every Psyker turn. Each turn it also casts / reinforces / revokes one. |

**Classes** (per-hero; stats in `[player.<key>]`, kit in `meld-battle`):

- **Squire** — the martial baseline: Attack / Defend / Item / Skill (Power Strike, Second Wind).
- **Psyker** — psychic channeler. Instead of the martial kit it manages **Foci**: Gravity Well
  (armour-ignoring damage tick), Kinetic Aegis (grants **Barrier**), Mind Spike (L3, stronger),
  Temporal Anchor (L5, drains the enemy's ATB gauge). See `Battle::resolve_psyker`.
- **Resonant** — healer. Innate **Regen**, plus ally-auto-targeting skills: Transfuse (heal an ally,
  paid from its own HP), Regen Boon (grant Regen), Ward (grant **Barrier**). See `Battle::resolve_resonant`.

New classes: add the enum variant (`meld-proto` `CharacterClass`), `[player.<key>]` stats +
any `[battle]` tunables, the `class_key` mapping (`meld-run`), the kit in `meld-battle`, and the
client menu branch (`menu_entries` keyed off the active hero's `class:` status).

## Deep Dives

- **Combat / ATB** (gauges, turns, flee, merge, statuses): [`behaviors/combat-atb.md`](behaviors/combat-atb.md)
- **World generation** (distance→difficulty, biome bands, areas, portals): [`behaviors/world-generation.md`](behaviors/world-generation.md)
- **Run lifecycle** (enter-maze, extraction, death durability): [`behaviors/run-lifecycle.md`](behaviors/run-lifecycle.md)
- **Economy / meta / endgame / disconnect / async**: [`behaviors/`](behaviors/) (`economy.md`, `meta-progression.md`, `endgame-seasons.md`, `disconnect-handling.md`, `async-interaction.md`)
- **Realtime protocol** (session, movement, battle, run/social messages): [`interfaces/realtime-protocol.md`](interfaces/realtime-protocol.md) + [`interfaces/realtime-protocol/`](interfaces/realtime-protocol/)
- **HTTP API** (auth, runs/world, vault/gear, crafting, economy, leaderboards): [`interfaces/http-api.md`](interfaces/http-api.md) + [`interfaces/http-api/`](interfaces/http-api/)
- **Data models**: [`interfaces/data-models.md`](interfaces/data-models.md) + [`interfaces/data-models/`](interfaces/data-models/)
- **What's implemented vs deferred**: [`SPIKE-NOTES.md`](SPIKE-NOTES.md) and [`GAP-ANALYSIS.md`](GAP-ANALYSIS.md)
- **Milestones & tasks**: [`BUILD-PLAN.md`](BUILD-PLAN.md)
