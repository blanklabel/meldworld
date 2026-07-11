# Session Messages

> Parent: [interfaces/realtime-protocol](../realtime-protocol.md)

Auth handshake, heartbeat/keepalive, reconnect-and-resume, and disconnect semantics for the realtime WebSocket connection. Envelope format, sequencing rules, and the error-code table live in the [index](../realtime-protocol.md).

### `session.authenticate` (C2S)

Presents a session ticket to authenticate the socket; optionally resumes a recently dropped session.

**Source:** GDD.md §5 (disconnect mechanics); CANON.md §I (auth/wire conventions), §B (10 s grace window).
**Direction:** C2S — MUST be the first message on a new socket, within 5 s **[TUNABLE]** of the socket opening.
**Idempotency:** Non-idempotent. A second `session.authenticate` on an already-authenticated socket is rejected with `invalid_state`.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| ticket | string | Yes | No | — | Single-use, short-lived session ticket minted by the HTTP API. Consumed on first presentation, whether auth succeeds or fails. |
| resume | object | No | Yes | null | Resume block. When present and valid, the previous session is resumed instead of a fresh one being created. When omitted or `null`, a fresh session starts. |
| resume.session_id | string (uuid) | Yes | No | — | The `session_id` from the previous `session.authenticated`. |
| resume.last_server_seq | integer (int64, u32 range, ≥ 0) | Yes | No | — | Highest S2C envelope `seq` the client fully processed. `0` if none. |

**Server validation**

- Ticket must be valid, unexpired, unused, and belong to the connecting player → else `unauthorized`, then socket close.
- `resume.session_id` must reference a session for the **same player** that is inside its 10 s grace window → else `resume_failed` (fresh authenticate required; the run-side disconnect consequences that already fired are not rolled back).
- `resume.last_server_seq` must be within the server's replay buffer → else `resume_failed`.
- If the player has a live connected session elsewhere, the new connection wins: the old socket receives `session.terminated` (`replaced_by_new_connection`) and this authenticate proceeds as a resume of that session's state.

**Results in**

- Success → `session.authenticated`; on resume, immediately followed by the seq replay (see [Reconnect and resume](#reconnect-and-resume-seq-replay)) and a `world.presence_update` to party members if the avatar had gone `sleeping`.
- Failure → `session.error` (`unauthorized` | `resume_failed` | `validation_error`), and for `unauthorized` the server closes the socket.

**Example — fresh session**

```json
{"type": "session.authenticate", "seq": 1, "ts": 1783728000000, "payload": {"ticket": "rt-4f9a1c2e-example-ticket", "resume": null}}
```

**Example — resume**

```json
{"type": "session.authenticate", "seq": 1, "ts": 1783728004200, "payload": {"ticket": "rt-8b2d3f4a-example-ticket", "resume": {"session_id": "0197a3b2-4c1d-7e2f-9a0b-3c4d5e6f7a8b", "last_server_seq": 8341}}}
```

---

### `session.authenticated` (S2C)

Confirms the handshake and delivers session parameters.

**Source:** CANON.md §I; CANON.md §B (disconnect handling — grace window).
**Direction:** S2C — direct ack of `session.authenticate` (carries `client_seq`).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | No | — | Echo of the authenticate envelope `seq` (CANON.md §I ack rule). |
| session_id | string (uuid) | Yes | No | — | Logical session id (UUIDv7). Presented in a future `resume` block. |
| player_id | string (uuid) | Yes | No | — | The authenticated player. |
| resumed | boolean | Yes | No | — | Whether an existing session was resumed. `false` for a fresh session (seq counters reset to 1). |
| heartbeat_interval_ms | integer (int32) | Yes | No | — | Interval at which the client must send `session.heartbeat`. Default configuration `5000` **[TUNABLE]**. |
| grace_window_ms | integer (int32) | Yes | No | — | Silent-reconnection grace before disconnect rules fire. `10000` (CANON.md §B). |
| server_ts | integer (int64, u64) | Yes | No | — | Server wall-clock, Unix millis, for client clock offset estimation. |
| last_client_seq | integer (int64, u32 range, ≥ 0) | Yes | No | — | Highest C2S `seq` the server processed for this session. `0` on fresh sessions. On resume, the client resends nothing at or below this and continues its counter above it. |

**Example**

```json
{"type": "session.authenticated", "seq": 1, "ts": 1783728000031, "payload": {"client_seq": 1, "session_id": "0197a3b2-4c1d-7e2f-9a0b-3c4d5e6f7a8b", "player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "resumed": false, "heartbeat_interval_ms": 5000, "grace_window_ms": 10000, "server_ts": 1783728000030, "last_client_seq": 0}}
```

---

### `session.heartbeat` (C2S)

Keepalive ping proving the client is still connected.

**Source:** GDD.md §5 (server intercepts connection drops); CANON.md §B (10 s grace). The 5 s interval is a canon gap resolved by this spec **[TUNABLE]**.
**Direction:** C2S — sent every `heartbeat_interval_ms` while authenticated. Any C2S traffic resets the silence timer; heartbeats are only mandatory when otherwise idle.
**Idempotency:** Safe; carries no state.

**Payload** — empty object `{}`.

**Server validation** — none beyond auth. Before auth → `unauthorized`.

**Results in** — `session.heartbeat_ack`. Two consecutive missed heartbeats (10 s of C2S silence) mark the connection **lost** and start the 10 s grace window.

**Example**

```json
{"type": "session.heartbeat", "seq": 57, "ts": 1783728030000, "payload": {}}
```

---

### `session.heartbeat_ack` (S2C)

Keepalive pong with server time.

**Source:** CANON.md §I (ack echoes client seq); §B.
**Direction:** S2C — direct ack of `session.heartbeat`.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | No | — | Echo of the heartbeat envelope `seq`. |
| server_ts | integer (int64, u64) | Yes | No | — | Server wall-clock, Unix millis, for RTT/offset estimation. |

**Example**

```json
{"type": "session.heartbeat_ack", "seq": 903, "ts": 1783728030018, "payload": {"client_seq": 57, "server_ts": 1783728030018}}
```

---

### `session.error` (S2C)

The single rejection message for any failed C2S intent, in every domain.

**Source:** CANON.md §I (error-code naming, ack seq echo); code table in the [index](../realtime-protocol.md#common-rejection-message).
**Direction:** S2C — direct response to the rejected C2S message. Never sent unsolicited.
**Side effects:** None — a rejected intent is guaranteed not to have executed (rejections happen-before any state change).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| code | string (enum: `validation_error`, `unauthorized`, `forbidden`, `not_found`, `invalid_state`, `out_of_range`, `duplicate_action`, `sequence_error`, `resume_failed`, `rate_limit_exceeded`, `internal`) | Yes | No | — | Machine-readable rejection code. Stable — safe to match on. |
| message | string | Yes | No | — | Human-readable description of the violated rule. May change without notice — do not parse. |
| client_seq | integer (int64, u32 range) | Yes | Yes | — | Envelope `seq` of the rejected C2S message. `null` only when the message was so malformed its `seq` could not be read. |

**Example**

```json
{"type": "session.error", "seq": 1204, "ts": 1783728041500, "payload": {"code": "duplicate_action", "message": "action_id 0197a4c8-90ab-7cde-8f01-23456789abcd was already submitted for this battle.", "client_seq": 88}}
```

---

### `session.terminated` (S2C)

Server-initiated close notice; the socket is closed immediately after it is sent.

**Source:** GDD.md §5; CANON.md §B (instance close after 60 min all-disconnected), §D13.
**Direction:** S2C — unsolicited.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| reason | string (enum: `replaced_by_new_connection`, `auth_timeout`, `idle_timeout`, `server_shutdown`, `protocol_violation`) | Yes | No | — | Why the server is closing the connection. |
| resumable | boolean | Yes | No | — | Whether a `resume` reconnect within the grace window is honored. `true` for `server_shutdown`; `false` for `replaced_by_new_connection`, `auth_timeout`, and `protocol_violation`. |

**Example**

```json
{"type": "session.terminated", "seq": 2210, "ts": 1783729990000, "payload": {"reason": "replaced_by_new_connection", "resumable": false}}
```

---

## Reconnect and resume (seq replay)

**Source:** CANON.md §B (grace window: 10 s silent reconnection before disconnect rules fire), §I (per-connection monotonic seq).

1. Connection is marked **lost** (socket close, or 10 s C2S silence). The 10 s grace timer starts. Nothing is broadcast yet — other players see the avatar continue standing (or its battle continue) unchanged.
2. Client reconnects, obtains a fresh ticket over HTTP, and sends `session.authenticate` with a `resume` block.
3. On success (`session.authenticated` with `resumed: true`), the server **replays** every buffered S2C message with envelope `seq > resume.last_server_seq`, in original order, with their **original** `seq` and `ts` values, before sending any new messages. The replay buffer must cover at least the grace window plus replay time; default retention 512 messages or 30 s, whichever is larger **[TUNABLE]**.
4. The client resumes its own C2S counter above `last_client_seq` from the ack. Intents it sent that were never processed can be re-issued with new seqs.
5. If the grace window has expired, resume fails (`resume_failed`); the client must authenticate fresh, and rejoins whatever state its avatar is now in (e.g. `sleeping` on the overworld, or auto-defending in a Gatekeeper battle — the server then wakes the avatar / returns battle control, broadcasting the corresponding `world.presence_update`).
6. State reconciliation after replay is guaranteed by the server: the first post-replay messages are a full `world.snapshot`, `world.chunk_load` for every chunk in interest radius the client no longer holds, and (if in battle) a full `battle.gauge_update`.

## Disconnect semantics handoff

What happens when the 10 s grace window expires depends on where the avatar is. The rules are owned by the battle and world domains; this section is the routing table.

**Source:** GDD.md §5 (disconnect & sleep mechanics); CANON.md §B (disconnect handling).

| Avatar situation at grace expiry | Rule | Documented in |
|----------------------------------|------|---------------|
| In a **standard** encounter | Forced flee — always succeeds (structural). Party is force-fled from the subscreen. | [battle.md — Disconnect handling](battle.md#disconnect-handling-in-battle) |
| In an **elite / Gatekeeper** encounter | No flee. Combatant enters auto-defend until the battle ends or the player reconnects. | [battle.md — Disconnect handling](battle.md#disconnect-handling-in-battle) |
| Out of battle, in a run | Avatar placed **sleeping** on the overworld at its last authoritative position — attackable, not safe. | [movement-world.md — `world.presence_update`](movement-world.md#worldpresence_update-s2c) |
| All instance members disconnected for 60 min | Instance closes; sleeping avatars auto-abandon (counts as death for the backpack, **no** durability loss). | [run-social.md — `run.instance_closed`](run-social.md#runinstance_closed-s2c) |
| In a Hub (no run) | Session ends; nothing ephemeral to protect. Stalls persist independently (HTTP-owned). | — |
