# MELDWORLD

**An MMO roguelite where every dive is a gamble: march out from the hub, fight
your way through a procedurally-generated world, and get your loot home — or lose
everything.** Turn-based JRPG combat, an extraction-shooter's nerve, and a
persistent economy, built entirely in Rust.

---

## What is this?

You start at the Center Hub with a party of four heroes and a nearly-empty
backpack. Ahead of you is an endless world that gets more dangerous the further
you travel — the monsters hit harder, but the loot gets richer. That's the whole
tension of the game: **how far do you push before you turn back?**

Every fight is a classic **Active Time Battle** (ATB) encounter — the same
turn-based rhythm as the 16-bit JRPGs it's built to honor, where speed decides
who acts and a well-timed skill turns a losing fight around. But the world you
fight in belongs to the extraction genre. Anything you pick up on a dive sits in
a fragile **backpack**. Reach a portal and extract, and it's banked forever in
your **Vault**. Die on the way, and the backpack — and every level you gained on
that run — is gone. Only your insured gear comes home (a little more worn than it
left).

Difficulty isn't a menu setting. It's **distance**. The world is a radial plane
expanding out from the hub, and monster level, stat scaling, and loot rarity all
climb the further out you go. Walk east through forest, then desert, ashfall,
tundra, and the mire beyond — each biome its own terrain, creatures, and
harvestable materials.

### The heroes

Your party is up to four heroes of mixed classes, each commanded by its own menu:

- **Hunter** — the martial backbone. Basic attacks bank **Adrenaline**; every
  skill spends it, from a heavy Power Strike to a full-power Frenzy.
- **Psyker** — a psychic channeler who juggles persistent **Manifestations**:
  armor-ignoring gravity wells, kinetic barriers, ATB-draining anchors.
- **Resonant** — the healer. Innate regeneration, plus skills that mend allies
  (paid from its own HP), hand out barriers, and keep the party standing.
- **Shifter** — the fast, fragile rogue. The only class that dodges by default;
  armor-piercing backstabs and evasive blinks.
- **Iron Hull** — the wall. Slowest and tankiest, trading momentum for
  blunt-force strikes that stagger and an all-enemy shockwave.

### The world beneath the fights

Between battles the overworld is a real place: a scroll-in-any-direction map with
biome terrain you collide with and slide against, raised terraces you climb via
ropes and ladders, roaming creatures that avoid the obstacles just like you do,
and resource nodes you harvest for crafting materials. A clear path to the exit
is *always* carved by construction — but the interesting stuff is off it.

Fights are **opt-in**: touch a creature and only *your* heroes get pulled in. A
teammate can choose to jump into an ongoing battle, but nobody drags you into a
fight you didn't pick.

> **Status:** MELDWORLD is a working vertical slice on its real production stack —
> a thin-but-honest cut of the full game. You can play the whole core loop today.
> Larger systems (the full economy and meta-progression, seasons, gatekeepers,
> world chunk streaming) are scoped and on the way. See
> [`SPIKE-NOTES.md`](SPIKE-NOTES.md) for exactly what's live versus planned.

---

## Play it

The fastest way in — one command, one URL:

```sh
make play          # build the web client, boot everything, then open the URL it prints
```

`make play` builds the WebAssembly client, boots a throwaway local Postgres, and
starts the server — which serves the client itself, so the whole game lives at a
single address (**http://127.0.0.1:18090** by default). The first build compiles
the wasm bundle, so give it a minute or two. Then open the URL, click the page,
and press **ENTER**.

Other ways to run it:

```sh
make play-native   # the native desktop window instead of a browser
make play-solo     # a self-contained native build — no Postgres, no setup, nothing to install
make help          # every command, explained
```

**`make play-solo` is the zero-setup option.** It's a single binary with the
server baked in (in-memory database, embedded assets) — great for a quick local
try. State is ephemeral and resets on exit, which is exactly what you want for a
fresh dive every launch.

### Controls

Build your party of four on the **Join** screen (keys **1–4** cycle each slot's
class), then:

- **WASD** — march through the overworld
- Walk into a creature — start a fight
- **H** — harvest the nearest resource node
- **T** — use a Town Portal to extract from anywhere; **E** — the deep exit portal
- **J** — join a teammate's ongoing battle

Prefer to watch it play itself? Open **http://127.0.0.1:18090/?autoplay**. You can
also preset a party without touching the Join screen:
`?party=hunter,psyker,resonant,hunter` (or `?class=psyker` for the lead) in the
browser, or `MELD_PARTY=…` natively.

### Requirements

`make play` needs a Rust toolchain, a local Postgres (`initdb` / `pg_ctl` /
`createdb` on your `PATH`), plus `trunk` (`cargo install trunk`) and the wasm
target (`rustup target add wasm32-unknown-unknown`) for the web build.
`make play-solo` needs only the Rust toolchain.

---

## Quickstart: build the game with an AI agent

MELDWORLD is built to be worked on by AI coding agents (like
[Claude Code](https://claude.com/claude-code)) — and the repo is set up so an
agent can get productive fast. If you want to add a feature, fix a bug, or just
explore, here's the agent-led on-ramp.

### 1. Point your agent at the conventions

The single most important file is [`CLAUDE.md`](CLAUDE.md) (symlinked as
`AGENTS.md`). It's the working contract for any agent in this repo: the
architecture, the workspace layout, how to run and test things, and the rules for
working safely alongside other agents. **Have your agent read it first.** A good
opening prompt:

> Read CLAUDE.md and the spec hierarchy it points to, then summarize how this
> codebase is organized and how gameplay behavior is specified.

### 2. Understand how behavior is specified

Gameplay isn't defined by whatever the code happens to do — it's specified
top-down, and **the higher document wins on conflict**:

1. [`GDD.md`](GDD.md) — the design vision (the *intent*).
2. [`CANON.md`](CANON.md) — authoritative answers to every ambiguity in the GDD:
   names, enums, formulas, and every tunable constant. **CANON beats GDD.**
3. [`behaviors/`](behaviors/) + [`interfaces/`](interfaces/) — the spec: observable
   behavior and wire/data contracts, each citing its CANON source.
4. [`BUILD-PLAN.md`](BUILD-PLAN.md) — the milestones and task IDs the code is built
   against.

When your agent adds or changes a rule, it should cite the spec (`combat-atb.md`,
`CANON §B`, …) in a code comment, exactly like the existing code does. Two rules
carry most of the codebase's character, and your agent should internalize both:

- **Server-authoritative, always.** All combat math, movement, loot, and world
  generation happen on the server. The client sends *intents* and renders what the
  server reports — it never computes gameplay.
- **No gameplay number lives in code.** Every tunable constant sits in
  [`balance/balance.toml`](balance/balance.toml) behind the `meld-balance` loader.
  Formula *structure* is code; *coefficients* are config.

### 3. Get it running, then change something

Have your agent boot the game (`make play-solo` is the least fiddly) and confirm
it works before touching anything. Then pick a small, real change. Good starter
prompts:

> Add a new harvestable resource node to the forest biome, wired end-to-end:
> the `[resource.<kind>]` balance entry, the biome mapping in `meld-world`, and a
> QA test that harvests it. Cite the spec you're implementing against.

> Tune the Hunter's Frenzy skill: find its Adrenaline cost and damage in
> balance.toml, propose a change, and explain the trade-off using CANON's combat
> spec.

### 4. Verify like the repo does

The engine crates (`meld-battle`, `meld-world`) are pure, deterministic state
machines — no clocks, no global randomness, no I/O — so they're exhaustively unit
tested. The `qa/` suite goes further: it drives **real headless bot clients over
the real wire protocol**, no shortcuts and no client-side combat math.

```sh
cargo test --workspace                   # fast unit tests — no database needed
cargo clippy --workspace --all-targets   # keep it warning-clean
make test                                # the full Postgres-backed conformance suite
make smoke                               # headless run of the whole loop (exits 0 on victory)
```

Anything visual (the HD-2D art, HUD, overworld, battle screen) is verified by
**screenshot**, not by clicking through — the client paints to a canvas, so boot
the stack, load the page, and capture the frame.

### 5. Play nice with other agents

This repo is designed for many agents working at once, each in its own **git
worktree** under `.claude/worktrees/` on its own branch. The rules that keep
concurrent work from colliding — unique server ports, sharing the one local
Postgres, additive edits to global files like `balance.toml` and the wire types —
all live in [`CLAUDE.md`](CLAUDE.md) under *Working alongside other agents*. Point
your agent there before it runs anything.

---

## How the core loop is proven

The `qa/` suite is the honest proof that the whole loop works end to end — every
test drives real bot clients through the real protocol.

- **`four_players_kill_monster`** — four bots register and log in (Postgres,
  bcrypt), handshake over WebSocket, form a maze instance, walk a hero into a
  monster to trigger a fight, and trade ATB turns until the monster dies.
- **`extraction`** — the extract-or-die half: kill the monster, loot it, walk to
  the portal, channel an extraction, and confirm the loot is banked into the
  persistent Vault. Move mid-channel and it's interrupted; die and the backpack is
  lost.
- **`death_durability`** — a hero dies and its insured gear comes home with 10%
  less max durability (the durability sink).
- **`progression`** — extraction credits crafting XP; crafting the loot consumes
  materials and credits more, mutating the Vault.
- **`raid_merge`** — one party engages a monster and a second party merges into
  the same battle; both win together.
- **`auth_conformance`** — register/login/me, bcrypt-only credential storage, and
  an enumeration-safe identical error for unknown-username vs wrong-password.

---

## How it's built

MELDWORLD is a Rust workspace. The server, the shared wire types, and the Bevy
client are all Rust; the client is a *separate* workspace under `client/` so its
heavy wasm/Bevy dependencies don't weigh down the server.

```
shared/meld-proto/           wire types: envelope, C2S/S2C messages, HTTP DTOs, enums, validators
balance/balance.toml         every tunable constant (no gameplay literal lives in code)
server/crates/
  meld-balance/              typed balance.toml loader
  meld-db/                   Postgres persistence (accounts, Vault, gear, crafting skills)
  meld-api/                  HTTP API (axum): auth, players/me, vault, crafting
  meld-battle/               server-authoritative ATB engine (100 ms tick) — pure & deterministic
  meld-world/                overworld: seeded procedural areas, terrain, monsters, movement
  meld-run/                  run/instance lifecycle + battle assembly
  meld-server/               WebSocket gateway + session handshake + the game loop + HTTP mount
client/crates/meld-client/   Bevy client (native + wasm): Join → Overworld → Battle → Ended
qa/                          headless bot framework + Postgres-backed conformance tests
```

At the heart of it is one authoritative game loop
([`meld-server/src/game.rs`](server/crates/meld-server/src/game.rs)): a single
task owns all the ephemeral state, is fed events over a channel, advances the ATB
on a 100 ms tick, and fans authoritative messages back to each session. Exactly
one task touches the state — so there are no locks.
