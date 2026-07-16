# MELDWORLD

An instanced, asynchronous MMO roguelite with ATB combat — built all-in-Rust
per [`GDD.md`](GDD.md) → [`CANON.md`](CANON.md) → [`spec`](interfaces/) (behaviors +
interfaces), executed against [`BUILD-PLAN.md`](BUILD-PLAN.md).

This branch is the **architecture spike**: the real server stack on the canonical
workspace layout, proven end-to-end to the point where **four players join a
server and kill one creature**. The Bevy client (T4/T5), Postgres-backed
economy/meta/endgame, chunk streaming, and Gatekeepers are the next slices — see
[`SPIKE-NOTES.md`](SPIKE-NOTES.md) for exactly what is implemented vs deferred.

## Quick start

Prereqs: a Rust toolchain and a local Postgres (`initdb`/`pg_ctl`/`createdb` on
PATH), or Docker.

Run the end-to-end proof (boots a throwaway Postgres, runs the QA suite):

```sh
qa/scripts/local_pg.sh          # → cargo test -p meld-qa
```

Run the server against a database and talk to it:

```sh
# Option A: Docker Postgres
docker compose up -d
MELD_DATABASE_URL=postgres://meld:meld@localhost:5432/meldworld cargo run -p meld-server

# Option B: your own Postgres
MELD_DATABASE_URL=postgres://user@localhost:5432/meldworld cargo run -p meld-server
```

The server exposes the HTTP API and realtime WebSocket on `MELD_ADDR`
(default `0.0.0.0:8080`; `PORT` also honored).

## The core loop, proven

`qa/tests/four_players_kill_monster.rs` drives four headless bot clients through
the real wire protocol — no shortcuts, no client-side combat math:

1. `POST /v1/auth/register` + `POST /v1/auth/login` → session token + realtime ticket (Postgres, bcrypt).
2. WebSocket `session.authenticate` handshake → `session.authenticated`.
3. `run.enter_maze` forms one `MazeInstance`; all four get `run.started`.
4. `movement.move_intent` walks a hero into the monster → server-detected touch → `battle.started`.
5. On each `battle.turn_ready`, bots submit `battle.submit_action { attack }`.
6. The 100 ms ATB engine resolves damage; the monster dies → `battle.ended { outcome: victory }`.

`qa/tests/extraction.rs` proves the **extract-or-die** half: a bot kills the
monster (loot → backpack), walks to the extraction portal, channels an extraction
(`run.begin_extraction` → `run.channel_started` → `run.member_result`), and the
loot is **banked into the persistent Vault** — verified over `GET /v1/vault`
(Postgres). Move mid-channel and it's interrupted; die and the backpack is lost.

More Postgres-backed conformance tests cover the RPG systems:

- `qa/tests/death_durability.rs` — a passive bot dies; its equipped blue-chest gear loses 10% max durability (the durability sink, CANON D6).
- `qa/tests/progression.rs` — extraction credits **Alchemy** XP; crafting the loot consumes it and credits **Forging** XP, mutating the Vault (`/v1/meld-skills`, `/v1/crafting/craft`).
- `qa/tests/raid_merge.rs` — an anchor party engages the monster and a second party **merges into the battle** (`battle.party_joined`); both win together.

`qa/tests/auth_conformance.rs` covers the auth acceptance criteria (BUILD-PLAN
M1.1/M1.8/M1.9): register/login/me, bcrypt-only credential storage, and the
enumeration-safe identical error for unknown-username vs wrong-password.

## Play it

The quickest way — a `Makefile` wraps the Postgres + server + client wiring so
you don't have to remember any of it:

```sh
make play         # boot everything, then open http://localhost:9080 in a browser
make play-native  # same, but opens the native desktop window instead
make help         # list every task (test, server, smoke, stop, …)
```

`make play` boots a throwaway Postgres, starts the server, and serves the wasm
client (first run compiles the wasm bundle — give it a minute). Open
**http://localhost:9080**, click the page, and press **ENTER** to play — or open
**http://localhost:9080/?autoplay** to watch it play itself.

The all-Bevy client (CANON D16) implements the core gameplay loop as screens:
**Join** (Enter to auth as a guest) → **Overworld** (WASD to march east through
the procedurally-generated biome areas; walk into a creature to fight; walk to a
cyan portal and press **E** to extract) → **Battle** (ATB HUD — HP + gauge bars
from the server) → **Ended** (extracted / defeat). It's server-authoritative:
the client sends intents and renders whatever the server reports, never
computing combat. Solo is winnable but tense; a full party wins comfortably.

**Headless verification** (no window — drives the whole loop through the client's
own network layer against a real server; exits 0 on victory):

```sh
make smoke
```

### Browser details

The same client compiles to WebAssembly (networking via `ewebsock`/`ehttp` works
on native *and* web); `make play` uses `trunk`, which serves the wasm client on
port 9080 and proxies `/v1` + the realtime socket to the server on
`$MELD_ADDR` (default `127.0.0.1:18090`). Needs `trunk` (`cargo install trunk`)
and the wasm target (`rustup target add wasm32-unknown-unknown`).

`?autoplay` self-drives the loop against the server; `?demo` runs an offline
render demo (no server) — handy for screenshots. The wasm build needs rustup's
toolchain (the wrapper sets that up); a `?server=<url>` param points the client
at a server on another origin (the server sends permissive CORS for the HTTP
API). Requires the wasm target: `rustup target add wasm32-unknown-unknown`.

## Workspace layout (BUILD-PLAN §1)

```
shared/meld-proto/           wire types: envelope, C2S/S2C messages, HTTP DTOs, enums, validators
balance/balance.toml         every [TUNABLE] constant (no gameplay literal lives in code)
server/crates/
  meld-balance/              typed balance.toml loader
  meld-db/                   Postgres persistence (accounts + bcrypt credentials)
  meld-api/                  HTTP API (axum): auth, players/me, realtime-ticket mint
  meld-battle/               server-authoritative ATB engine (100 ms tick, gauges)
  meld-world/                overworld: seeded arena, monster, movement, touch detection
  meld-run/                  run/instance lifecycle + battle assembly
  meld-server/               WS gateway + session handshake + the game loop + HTTP mount
qa/                          headless bot framework + conformance/integration tests (T6)
```

The authoritative game loop ([`meld-server/src/game.rs`](server/crates/meld-server/src/game.rs))
is the Rust descendant of the original Go `GameHub`: one task owns all ephemeral
state and is fed events over an mpsc channel, fanning authoritative messages back
per session — no locks (CANON.md §S).

## Testing

```sh
cargo test --workspace          # unit tests (no DB needed)
qa/scripts/local_pg.sh          # + the DB-backed QA suite
cargo clippy --workspace --all-targets   # clean: 0 warnings
```
