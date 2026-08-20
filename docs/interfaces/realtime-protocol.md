# Realtime WebSocket Protocol

The realtime protocol carries all **ephemeral** state between game clients and the authoritative Rust server: movement, chunk streaming, ATB battles, item drops, presence, and run lifecycle. It never carries persistent mutations — anything that survives logout (vault, gear, chits, meld skills, stalls, contracts, leaderboards, run history) is mutated through the HTTP API or by the server itself at run end, and only its *ephemeral consequences* are visible on this channel.

**Source:** CANON.md §S (system boundaries), §I (wire conventions); GDD.md §1–§2.

> This is a forward-design spec: `**Source:**` lines cite design documents (`GDD.md §N`, `CANON.md §X`), not code. Implementations must conform to this spec; constants marked **[TUNABLE]** must be server config, not hardcoded (CANON.md preamble).
>
> **Much of it is now built, and the two have diverged.** The [Message Summary](#message-summary)
> below is reconciled against `shared/meld-proto/src/realtime.rs` and splits shipped messages from
> spec-only ones. Where this document and `meld-proto` disagree about a message that exists,
> **`meld-proto` is the contract** — it is what both sides compile against. Where a message is
> spec-only, this document is still the design of record.

## Overview

**Endpoint:** `GET /v1/realtime` — HTTP upgrade to WebSocket (`wss://`). Same host and `/v1` base path as the HTTP API.
**Transport:** WebSocket text frames, one JSON envelope per frame. Binary frames are rejected with `session.error` code `validation_error`.
**Auth model:** Session-ticket handshake. The client obtains a short-lived, single-use session ticket from the HTTP API (Bearer-authenticated; out of scope here), then presents it in `session.authenticate` as the **first** message on the socket. See [session.md](realtime-protocol/session.md#sessionauthenticate-c2s).
**Versioning policy:** The protocol is versioned by the `/v1` path segment. New message types and new optional payload fields may be added without a version bump; removing a message, renaming a field, or changing a field type is a breaking change and requires `/v2`.
**Authority model:** Server-authoritative throughout (CANON.md D11, §S). Clients send **intents**; the server validates and broadcasts **authoritative state**. A client-rendered position, gauge, or damage number is never trusted.

## Envelope Format

Every message in both directions uses this envelope (CANON.md §I):

```json
{
  "type": "battle.submit_action",
  "seq": 42,
  "ts": 1783728000000,
  "payload": {}
}
```

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| type | string (pattern: `^[a-z]+\.[a-z_]+$`) | Yes | No | — | Message name, `<domain>.<verb_phrase>`. Domains in use: `session`, `movement`, `world`, `battle`, `run`, `chat`, `onboarding`, `lobby`. (`social` is specified here but not implemented — see Message Summary.) |
| seq | integer (int64, u32 range, ≥ 1) | Yes | No | — | Per-session monotonic sequence number. Client and server each maintain their own independent counter, starting at 1 after a fresh (non-resume) authenticate. |
| ts | integer (int64, u64) | Yes | No | — | Sender wall-clock time, Unix milliseconds UTC (CANON.md §I: `u64` unix millis on the realtime protocol). |
| payload | object | Yes | No | — | Message-specific body. May be `{}` but must be present. Shape is defined per message in the detail files. |

**Source:** CANON.md §I.

### Sequencing, ordering, and acknowledgement

**Source:** CANON.md §I (`seq` is per-connection monotonic; server echoes client `seq` in acks), §B (disconnect handling — 10 s grace).

- `seq` counters are scoped to the **session** (the authenticated logical connection), not the TCP socket, so they survive reconnect-and-resume. A fresh authenticate resets both counters to 1.
- The server processes C2S messages in `seq` order. A C2S message whose `seq` is ≤ the highest already-processed client seq, or that skips backward, is rejected with `session.error` code `sequence_error` and is **not** executed — this makes blind client retries safe.
- The server echoes the triggering client `seq` in acknowledgements: every S2C message sent as a direct response to one C2S message (acks and `session.error` rejections) carries a `client_seq` payload field holding that seq. Broadcast messages (snapshots, battle events seen by other players) carry no `client_seq`. The S2C envelope `seq` is always the server's own monotonic counter, because resume replay is keyed on it.
- Most C2S intents have **no dedicated success ack**; success is observed through the resulting authoritative broadcast (e.g. a `social.drop_item` succeeds when the `world.entity_spawn` for the dropped item arrives). Failures are always explicit via `session.error`.
- Idempotency: `battle.submit_action` carries a client-generated `action_id`; a duplicate `action_id` is rejected with `duplicate_action` and does not execute twice. All other C2S messages are non-idempotent intents; the `sequence_error` rule prevents duplicate execution on retry.

### Connection lifecycle

**Source:** GDD.md §5 (disconnect & sleep mechanics); CANON.md §B (disconnect handling), §I.

1. **Connect** — client opens the WebSocket at `/v1/realtime`.
2. **Auth handshake** — client sends `session.authenticate` (with session ticket) as the first message, within 5 s **[TUNABLE]** of the socket opening. Any other first message, an invalid ticket, or handshake timeout → `session.error` (`unauthorized`) followed by socket close. Success → `session.authenticated`.
3. **Steady state** — client sends `session.heartbeat` every 5 s **[TUNABLE]**; server answers `session.heartbeat_ack`. The connection is considered **lost** on socket close or after 10 s with no C2S traffic (two missed heartbeats).
4. **Grace window** — from the moment the connection is considered lost, a **10 s** silent-reconnection grace window runs (CANON.md §B). If the client reconnects and resumes within it, nothing observable happens to other players.
5. **Disconnect rules fire** — if the grace window expires, situational rules apply: forced flee from standard battles, auto-defend in elite/Gatekeeper battles, sleeping avatar on the overworld. See [session.md — Disconnect semantics handoff](realtime-protocol/session.md#disconnect-semantics-handoff).
6. **Resume** — a reconnecting client authenticates with a `resume` block; the server replays buffered S2C messages after the client's last received server seq. See [session.md — Reconnect and resume](realtime-protocol/session.md#reconnect-and-resume-seq-replay).

## Common Rejection Message

Every rejected C2S message produces a single `session.error` S2C message (full documentation in [session.md](realtime-protocol/session.md#sessionerror-s2c)):

```json
{
  "type": "session.error",
  "seq": 118,
  "ts": 1783728000450,
  "payload": {
    "code": "invalid_state",
    "message": "Actor gauge is not full.",
    "client_seq": 42
  }
}
```

Realtime error codes (names align with the canonical HTTP error codes of CANON.md §I where the semantics match; realtime-only codes marked ✦):

| Code | Condition |
|------|-----------|
| `validation_error` | Malformed envelope or payload: missing/unknown fields, wrong types, out-of-range values, binary frame, unknown `type`. |
| `unauthorized` | First message is not `session.authenticate`; ticket invalid, expired, or already used; any message before successful auth. |
| `forbidden` | Authenticated, but the intent targets state the player may not act on (e.g. another player's backpack item). |
| `not_found` | Referenced entity/battle/run/item id does not exist in the sender's instance. |
| `invalid_state` ✦ | Message is well-formed but not legal in the current state (not in a run, not in battle, gauge not full, flee disabled on Gatekeepers, target not in battle, channel already active…). Payload `message` states the violated precondition. |
| `out_of_range` ✦ | Target entity/position is beyond the interaction range (2 tiles **[TUNABLE]**). |
| `duplicate_action` ✦ | `battle.submit_action` with an `action_id` already submitted for this battle. |
| `sequence_error` ✦ | C2S envelope `seq` is not strictly greater than the last processed client seq. |
| `resume_failed` ✦ | Resume attempted after the grace window expired or beyond the replay buffer. Client must authenticate fresh. |
| `rate_limit_exceeded` | C2S message rate exceeds per-connection limits (e.g. `movement.move_intent` above 20 Hz sim rate). Offending message dropped, not executed. |
| `internal` | Unexpected server error. The intent did not execute; safe to retry with backoff. |

**Source:** CANON.md §I (canonical error codes); realtime-specific codes are a canon gap resolved by this spec.

## Common Payload Objects

Shared object shapes referenced by the detail files.

**Source:** CANON.md §G (glossary), §B (networking targets); GDD.md §3, §5.

### Position

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| x | number (double) | Yes | No | — | East–west coordinate in tile units from the world origin (Center Hub). |
| y | number (double) | Yes | No | — | North–south coordinate in tile units from the world origin. |

`distance` thresholds always use `floor(sqrt(x² + y²))` (CANON.md §G "Distance").

### ItemStack

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| item_id | string (uuid) | Yes | No | — | Server-generated id of this item instance (UUIDv7, CANON.md §I). |
| item_kind | string | Yes | No | — | Content-table item identifier, snake_case (e.g. `health_potion`, `warding_tent`, `ripcord_scroll`). |
| quantity | integer (int32, ≥ 1) | Yes | No | — | Stack size. |
| insurance | string (enum: `blue`, `red`) | No | Yes | null | Insurance tier for gear items (CANON.md §G). `null` for non-gear items. |

### Combatant

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| combatant_id | string (uuid) | Yes | No | — | Battle-scoped actor id. |
| kind | string (enum: `player`, `monster`, `gatekeeper_boss`) | Yes | No | — | Actor category. |
| player_id | string (uuid) | Yes | Yes | — | The player, when `kind` is `player`; `null` otherwise. |
| monster_kind | string | Yes | Yes | — | Content-table monster identifier, when `kind` is not `player`; `null` for players. |
| level | integer (int32, ≥ 1) | Yes | No | — | Run level (players) or `mlevel(d) = max(1, round(d / 12.5))` (monsters) (CANON.md §B). |
| hp | integer (int32, ≥ 0) | Yes | No | — | Current hit points. |
| max_hp | integer (int32, ≥ 1) | Yes | No | — | Maximum hit points. |
| gauge | number (double, 0.0–1.0) | Yes | No | — | ATB gauge fill. Full at `1.0`; fills `speed_stat × rate_mult / gauge_fill_divisor` (5200 **[TUNABLE]**) per 100 ms server tick (CANON.md §B). |
| statuses | array of string | Yes | No | — | Active status-effect identifiers. Empty array when none. |

## Message Summary

> **Implementation status.** This table is generated against `shared/meld-proto/src/realtime.rs`,
> which is the wire contract of record — a message not in that file does not exist. The **Shipped**
> table below is every message the server actually speaks today; the **Spec-only** table is design
> this file specifies that is *not built*. Detail-file links marked *undocumented* are shipped
> messages that have no prose spec yet; read the doc comments in `realtime.rs` for their payloads.
>
> Domains on the wire are `session`, `movement`, `world`, `battle`, `run`, `chat`, `onboarding`,
> `lobby`. The `social` domain is spec-only — nothing implements it, and the async-drop features it
> carries live in [`../behaviors/async-interaction.md`](../behaviors/async-interaction.md).

### Shipped (64 messages)

| Dir | Message | Summary | Detail |
|-----|---------|---------|--------|
| C2S | `session.authenticate` | Present session ticket | [session.md](realtime-protocol/session.md) |
| S2C | `session.authenticated` | Handshake success; session parameters | [session.md](realtime-protocol/session.md) |
| C2S | `session.heartbeat` | Keepalive ping | [session.md](realtime-protocol/session.md) |
| S2C | `session.heartbeat_ack` | Keepalive pong with server time | [session.md](realtime-protocol/session.md) |
| S2C | `session.error` | Rejection of a C2S message | [session.md](realtime-protocol/session.md) |
| S2C | `session.terminated` | Server-initiated close with reason | [session.md](realtime-protocol/session.md) |
| C2S | `movement.move_intent` | Movement input sample | [movement-world.md](realtime-protocol/movement-world.md) |
| S2C | `movement.position_correction` | Authoritative position override | [movement-world.md](realtime-protocol/movement-world.md) |
| S2C | `world.snapshot` | Periodic dynamic-entity state in interest radius | [movement-world.md](realtime-protocol/movement-world.md) |
| S2C | `world.terrain_section` | One section's elevation grid + connectors + trail (D24) | — *undocumented* |
| S2C | `world.dungeon_scene` | An authored dungeon's scene to render | — *undocumented* |
| S2C | `battle.started` | Touch-triggered battle opens | [battle.md](realtime-protocol/battle.md) |
| S2C | `battle.party_joined` | A second party merges into an active battle | [battle.md](realtime-protocol/battle.md) |
| S2C | `battle.turn_ready` | Actor's gauge is full; 15 s action window opens | [battle.md](realtime-protocol/battle.md) |
| S2C | `battle.gauge_update` | ATB gauge/HP sync (event-driven + 1 Hz keepalive) | [battle.md](realtime-protocol/battle.md) |
| C2S | `battle.submit_action` | Submit attack / skill / item / defend / flee | [battle.md](realtime-protocol/battle.md) |
| S2C | `battle.telegraph_started` | A creature shouted a telegraphed ability and is channeling | — *undocumented* |
| S2C | `battle.action_resolved` | Authoritative outcome of one resolved action | [battle.md](realtime-protocol/battle.md) |
| S2C | `battle.participant_left` | One party's combatants left an ongoing battle | [battle.md](realtime-protocol/battle.md) |
| S2C | `battle.ended` | Battle resolution: victory / defeat / fled | [battle.md](realtime-protocol/battle.md) |
| S2C | `battle.watch_ended` | A WATCHED feed closed (finished / out of range / stopped) | [battle.md](realtime-protocol/battle.md) |
| C2S | `run.enter_maze` | Start the party's run; create the MazeInstance | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.started` | Authoritative run/instance state at entry | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.party` | The caller's hero roster (name/class/level/attributes/HP) | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.perks` | The caller's earned overworld class perks | — *undocumented* |
| S2C | `run.level_up` | One or more heroes levelled on this victory | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.unlocked` | Account-permanent unlocks just landed (`CL-1`) | — *undocumented* |
| S2C | `run.hunt_progress` | A posted hunt moved (`AD-4`) | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.harvest` | Begin working a resource node (MS-2) | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.cancel_harvest` | Stop an in-progress harvest channel | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.world_end_felled` | The end fight is down; the dive ends here (EW) | — *undocumented* |
| C2S | `run.psyker_hold` | A Psyker pins a creature where it stands (CL-2) | — *undocumented* |
| C2S | `run.open_chest` | Open a treasure chest in reach | — *undocumented* |
| C2S | `run.build_station` | Raise a field forge / alembic (MS-1) | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.teardown_station` | Pack up a bench you raised | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.smith_request` | Ask a station's owner for a piece of work | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.tempo_started` | The heat is open: strike on the yellow band | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.strike` | A blow, at the marker's position on the bar | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.smith_result` | What the smith did, or why they would not | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.enter_dungeon` | Descend into an authored dungeon | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.join_battle` | Opt into a fight already in progress nearby | — *undocumented* |
| C2S | `run.watch_battle` | WATCH the nearest fight in reach without joining (`SOC-3`) | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.stop_watching` | Stop watching (idempotent) | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.rename_hero` | Rename one of the caller's heroes (persistent) | — *undocumented* |
| C2S | `run.set_formation` | Put a hero in the front or back row | — *undocumented* |
| C2S | `run.begin_extraction` | Start a `portal` or `town_portal` extraction channel | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.use_item` | Drink a potion on the overworld, out of combat | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.channel_started` | A channel began (harvest / extraction / station) | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.channel_interrupted` | Channel broke before completing, with reason | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.member_result` | A member's run ended: extracted / died / abandoned | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.backpack_update` | Authoritative delta to the caller's backpack | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.pouches` | The caller's per-hero pouches, whole | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.move_item` | Move items between the shared bag and a hero's pouch | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.equip_loot` | Equip/unequip this run's not-yet-banked loot gear | — *undocumented* |
| S2C | `run.gear` | Snapshot of the caller's current run-loot gear | — *undocumented* |
| C2S | `chat.say` | Say something | — *undocumented* |
| S2C | `chat.line` | Somebody said something (echoed to the sender too) | — *undocumented* |
| C2S | `onboarding.town_seen` | Caller dismissed the town welcome tour | — *undocumented* |
| C2S | `onboarding.run_seen` | Caller dismissed the first-dive briefing | — *undocumented* |
| S2C | `onboarding.status` | What this account has already dismissed | — *undocumented* |
| C2S | `lobby.create` | Create a lobby (caller becomes host) | — *undocumented* |
| C2S | `lobby.join` | Join an existing lobby by code | — *undocumented* |
| C2S | `lobby.ready` | Toggle the caller's ready flag | — *undocumented* |
| C2S | `lobby.leave` | Leave the current lobby | — *undocumented* |
| C2S | `lobby.start` | Host only: launch the dive | — *undocumented* |
| S2C | `lobby.state` | Authoritative lobby state, broadcast on any change | — *undocumented* |
| S2C | `lobby.closed` | The lobby was disbanded | — *undocumented* |

### Spec-only — specified here, not implemented (16 messages)

| Dir | Message | Summary | Detail |
|-----|---------|---------|--------|
| S2C | `world.chunk_load` | Stream a 64×64-tile chunk entering interest radius | [movement-world.md](realtime-protocol/movement-world.md) |
| S2C | `world.chunk_unload` | Evict a chunk leaving interest radius | [movement-world.md](realtime-protocol/movement-world.md) |
| S2C | `world.entity_spawn` | Entity appears in loaded chunks | [movement-world.md](realtime-protocol/movement-world.md) |
| S2C | `world.entity_despawn` | Entity removed from loaded chunks | [movement-world.md](realtime-protocol/movement-world.md) |
| S2C | `world.presence_update` | Party-member connection / avatar state (incl. sleeping) | [movement-world.md](realtime-protocol/movement-world.md) |
| C2S | `world.deploy_ward` | Deploy a ward item over a sleeping ally | [movement-world.md](realtime-protocol/movement-world.md) |
| S2C | `world.ward_deployed` | Ward active, with expiry | [movement-world.md](realtime-protocol/movement-world.md) |
| S2C | `battle.external_effect` | Overworld item injected into the battle | [battle.md](realtime-protocol/battle.md) |
| C2S | `run.cancel_extraction` | Voluntarily cancel own channel | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `run.abandon` | Explicitly abandon the run | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `run.instance_closed` | MazeInstance shut down | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `social.drop_item` | Drop backpack items onto the overworld | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `social.pickup_item` | Pick up a ground item | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `social.item_picked_up` | Pickup ack with the items gained | [run-social.md](realtime-protocol/run-social.md) |
| C2S | `social.drop_item_on_player` | Drop a consumable onto a battling player | [run-social.md](realtime-protocol/run-social.md) |
| S2C | `social.drop_applied` | Ack: the dropped item's effect was injected | [run-social.md](realtime-protocol/run-social.md) |

## HTTP Handoffs (out of scope here, but load-bearing)

**Source:** CANON.md §S (boundary rule); GDD.md §2.

- **Session ticket issuance** — HTTP API (opaque Bearer session token, CANON.md D17) mints the single-use realtime ticket consumed by `session.authenticate`.
- **Extraction banking** — when a `run.member_result` reports `extracted`, the server banks the backpack into the player's Vault (red-chest gear becomes owned Vault gear, still `red` tier) as a server-side persistent mutation; clients observe the result via HTTP vault endpoints, never via a realtime mutation.
- **Death durability** — on `died`, blue-chest gear returns to the Hub at `max_durability × 0.9` (round down), applied server-side; visible via HTTP gear endpoints.
- **Party formation / matchmaking, stalls, contracts, leaderboards** — HTTP-owned; the realtime channel only reflects their overworld presence (e.g. stall entities in `world.chunk_load`).
