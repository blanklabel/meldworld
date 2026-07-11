# Architecture Spike — Implemented vs Deferred

Today's goal (per direction): an all-Rust architecture spike on the **real
stack**, to the point where **4 players join a server and kill one creature**.
This is a thin, honest vertical slice of BUILD-PLAN M0–M2 — enough of every layer
to prove the architecture end-to-end, with everything else explicitly deferred.

## Implemented (proven by tests)

| Area | What | Where | Spec / milestone |
|------|------|-------|------------------|
| Shared protocol | Realtime envelope `{type,seq,ts,payload}`, session/movement/battle/run messages, HTTP DTOs, enums, validators; golden round-trip tests | `shared/meld-proto` | CANON §I; T1; M0.2/M0.3 |
| Tunables | Every constant the slice reads lives in `balance/balance.toml` with a typed loader; no gameplay literal in code | `balance/`, `meld-balance` | Working agreement #2; M0.4 |
| Persistence | Postgres; accounts + bcrypt (cost 12) credentials; idempotent migration on boot | `meld-db` | CANON D18; M1.8 |
| Auth | `POST /v1/auth/register` + `/login` (session token + single-use realtime ticket), `GET /v1/players/me`; enumeration-safe login | `meld-api` | CANON D17; M1.1/M1.8/M1.9 |
| Realtime gateway | `/v1/realtime` WS upgrade, ticket handshake → `session.authenticated`, per-session seq, heartbeat | `meld-server/gateway.rs` | realtime session.md; M1.2 |
| ATB engine | 100 ms tick, gauge fill `speed/400`, attack/defend/flee, 15 s auto-defend, duplicate-action rejection, victory/defeat; deterministic + unit-tested | `meld-battle` | combat-atb.md; M2.3/M2.4 |
| World | Distance→difficulty formulas (`tier`/`mlevel`/`stat_mult`), a bounded Forest arena, server-authoritative movement, touch-to-battle | `meld-world` | world-generation.md; M2.2 |
| Run lifecycle | `run.enter_maze` → one MazeInstance + party; base-run-level; backpack; XP/level-up; victory loot, defeat → died | `meld-run`, `meld-server/game.rs` | run-lifecycle.md; M2.1 |
| Game loop | Single-owner authoritative loop (Go `GameHub` descendant): sessions + instance, mpsc in, per-session fan-out, no locks | `meld-server/game.rs` | CANON §S |
| QA | Headless bot clients; 4-player kill conformance + auth conformance, over real HTTP + WS | `qa/` | T6; M1/M2 subset |

`cargo test --workspace` is green; `cargo clippy --workspace --all-targets` is 0 warnings.

## Deliberately deferred (next slices)

- **Bevy client (T4/T5)** — overworld rendering, HD-2D pipeline, ATB/hub/menu UI.
  The spike proves the server with bot clients (the BUILD-PLAN conformance model);
  a human-facing Bevy client is the next major slice.
- **Chunk streaming & full world gen** — `world.chunk_load/unload`, biomes past
  Forest, chokepoints, Gatekeeper arenas, infinite scaling past d=5000. The slice
  uses one bounded in-memory arena; the scaling *formulas* are already implemented.
- **Battle merge / raid** (`battle.party_joined`, 8/16 caps), skills, items,
  status effects, external heal injection.
- **Disconnect handling** — 10 s grace, resume/seq-replay, forced-flee vs
  auto-defend, sleeping avatars, wards, 60-min instance close.
- **Extraction & banking** — portals, ripcord channel, Backpack→Vault on
  extract, death durability −10%, the whole Vault/gear/meld/economy HTTP surface
  and its Postgres schema.
- **Endgame** — leaderboards, seasons, prestige drops.
- **Matchmaking** — the slice forms the party from the connected players at the
  first `run.enter_maze` (a documented simplification of the hub/party HTTP flow).

## Slice simplifications to unwind later

- One global `MazeInstance` at a time; party = connected players (≤4) at first
  `run.enter_maze`. Real matchmaking/party formation is HTTP-owned (D13).
- The overworld arena is bounded and non-streamed; the monster is placed at a
  fixed Forest distance. Movement is integrated per-intent (touch checked on
  receipt) rather than on a separate 20 Hz sim loop.
- `session_id`/resume are carried but resume/seq-replay is not implemented yet.
- Naia is named in the GDD, but the spec's actual realtime *contract* is the
  hand-rolled WebSocket envelope (realtime-protocol.md), which is what this
  implements — faithful to the wire spec without pulling in Naia's replication model.
