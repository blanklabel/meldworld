# Battle Messages

> Parent: [interfaces/realtime-protocol](../realtime-protocol.md)

Instanced ATB battle sync. All combat math is server-side (CANON.md D11): the server runs a 100 ms ATB tick, fills each combatant's gauge by `speed_stat / 400` per tick (full at 1.0), resolves actions, and broadcasts results. Clients render the Bevy battle UI (CANON.md D16) and submit action intents only. Battle S2C traffic is event-driven plus a 1 Hz `battle.gauge_update` keepalive (CANON.md §B, networking targets).

Key constants (CANON.md §B, ATB combat — all **[TUNABLE]** unless marked structural):

| Constant | Value |
|----------|-------|
| ATB server tick | 100 ms |
| Gauge fill per tick | `speed_stat / 400` (gauge full at 1.0) |
| Turn timeout | 15 s after gauge full without an action → auto-defend |
| Flee success | base 60%, −10% per tier the encounter is above the party's level tier, floor 5%; **disabled** vs Gatekeepers |
| Merge cap | 2 instances (8 combatants) for standard/elite; 4 instances (16) for Gatekeepers (CANON.md D5) |
| Disconnect grace | 10 s before disconnect rules fire |
| Forced flee (standard, on disconnect) | always succeeds (structural) |

Shared payload object `Combatant` is defined in the [index](../realtime-protocol.md#common-payload-objects). While in battle the avatar remains on the overworld with `avatar_state: "in_battle"` and the enemy keeps attacking even if the player stops interacting (GDD.md §5).

### `battle.started` (S2C)

A battle subscreen opened. There is **no C2S battle-start message**: battles are triggered server-side when the server's collision detection sees a player avatar touch a monster (or a roaming monster touch a sleeping avatar).

**Source:** GDD.md §5 (touching an enemy pulls the player into the ATB overlay; monsters can attack sleeping avatars); CANON.md §G (`Battle`), D11.
**Direction:** S2C — sent to every connected member of the party being pulled in. (Members already in another battle are not pulled in; the toucher's whole party joins.)

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| battle_id | string (uuid) | Yes | No | — | Server-side battle entity id, cited by all subsequent battle messages. |
| encounter_class | string (enum: `standard`, `elite`, `gatekeeper`) | Yes | No | — | Drives flee availability (`gatekeeper`: flee disabled) and disconnect rules (see [Disconnect handling](#disconnect-handling-in-battle)). |
| allies | array of Combatant (1–16 items) | Yes | No | — | Player-side combatants, in turn-display order. Player gauges start at 0.0. |
| enemies | array of Combatant (min 1 item) | Yes | No | — | Monster-side combatants. Gatekeeper HP pools are sized for 8 combatants at spawn and never rescale mid-fight (CANON.md §B). |
| your_combatant_id | string (uuid) | Yes | No | — | The recipient's own combatant, for input routing. |
| triggered_by | string (uuid) | Yes | Yes | — | Player whose touch started the battle; `null` when a monster touched a sleeping avatar. |

**Example**

```json
{"type": "battle.started", "seq": 4001, "ts": 1783728100000, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "encounter_class": "standard", "allies": [{"combatant_id": "0197a600-00aa-7abc-9def-0123456789ab", "kind": "player", "player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "monster_kind": null, "level": 12, "hp": 340, "max_hp": 340, "gauge": 0.0, "statuses": []}], "enemies": [{"combatant_id": "0197a600-00bb-7abc-9def-0123456789ab", "kind": "monster", "player_id": null, "monster_kind": "dune_stalker", "level": 14, "hp": 410, "max_hp": 410, "gauge": 0.35, "statuses": []}], "your_combatant_id": "0197a600-00aa-7abc-9def-0123456789ab", "triggered_by": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c"}}
```

---

### `battle.party_joined` (S2C)

A second (or later) party merged into an already-active battle by touching the same enemy — the raid-merge mechanic.

**Source:** GDD.md §5 (the Expandable Party); CANON.md D5 (merge caps), §B (joining party inserted at gauge 0; enemy stats do not rescale mid-fight).
**Direction:** S2C — broadcast to every combatant already in the battle **and** to the joining party (the joiners receive `battle.started` for this battle immediately before it, carrying full state; existing combatants receive only this delta).

**Server behavior**

- Merge trigger is server-detected touch, like battle start. Merging combines **battles**, not MazeInstances (CANON.md D13) — each party keeps its own instance, backpacks, and run levels.
- Joining combatants are inserted at `gauge: 0.0` (structural).
- Cap: a standard/elite battle holds at most 2 instances (8 combatants); a Gatekeeper battle at most 4 instances (16) **[TUNABLE]**. A touch that would exceed the cap does **not** merge: the toucher is nudged back via `movement.position_correction` and no battle message is sent.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| battle_id | string (uuid) | Yes | No | — | The battle joined. |
| joining_instance_id | string (uuid) | Yes | No | — | MazeInstance of the joining party. |
| joining_allies | array of Combatant (1–4 items) | Yes | No | — | The new player-side combatants, gauges at 0.0. |

**Example**

```json
{"type": "battle.party_joined", "seq": 4102, "ts": 1783728112000, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "joining_instance_id": "0197a5f0-9999-7abc-9def-0123456789ab", "joining_allies": [{"combatant_id": "0197a600-00cc-7abc-9def-0123456789ab", "kind": "player", "player_id": "0197a2f0-33cc-7ddd-9eee-0f1a2b3c4d5e", "monster_kind": null, "level": 15, "hp": 402, "max_hp": 402, "gauge": 0.0, "statuses": []}]}}
```

---

### `battle.submit_action` (C2S)

Submits the acting player's chosen action once their gauge is full.

**Source:** GDD.md §5 (ATB combat); CANON.md §B (ATB combat: gauge, timeout, flee), D11 (clients submit intents only).
**Direction:** C2S — legal only between a `battle.turn_ready` for the sender's combatant and the resolution of that turn.
**Idempotency:** Idempotent-by-rejection — `action_id` is a client-generated UUID unique per submission; resubmitting an `action_id` already accepted for this battle is rejected with `duplicate_action` and does not execute twice. A retry after a dropped connection should reuse the same `action_id`: if the original was accepted the retry safely fails.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| battle_id | string (uuid) | Yes | No | — | The battle being acted in. |
| action_id | string (uuid) | Yes | No | — | Client-generated idempotency key for this submission. |
| action | string (enum: `attack`, `skill`, `item`, `defend`, `flee`) | Yes | No | — | The chosen action. Determines which of the following fields are required. |
| skill_kind | string | No | Yes | null | Content-table skill identifier. Required when `action` is `skill`; must be `null` otherwise. |
| item_id | string (uuid) | No | Yes | null | Backpack item instance to consume. Required when `action` is `item`; must be `null` otherwise. |
| target_ids | array of string (uuid) | No | Yes | null | Target combatant ids. Required for `attack`, single-target `skill`s, and targeted `item`s; must be `null` for `defend` and `flee`. Multi-target skills list every target. |

**Server validation** (checked in order; first failure wins)

1. Not authenticated → `unauthorized`.
2. Unknown `battle_id`, or sender has no combatant in it → `not_found`.
3. Duplicate `action_id` → `duplicate_action`.
4. Sender's combatant gauge not full, or an action already accepted this turn → `invalid_state` ("Actor gauge is not full." / "Action already submitted for this turn.").
5. `action`/field mismatch (e.g. `item` without `item_id`) → `validation_error`.
6. `action: "flee"` in a Gatekeeper battle → `invalid_state` ("Flee is disabled against Gatekeepers.").
7. `item_id` not in the sender's backpack, or not usable in battle → `not_found` / `validation_error`.
8. Any `target_ids` entry not a living combatant in this battle → `not_found`. (A target that dies between submission and resolution is retargeted by server rule to the next living enemy, not rejected.)

**Results in** — acceptance is signalled by the resulting `battle.action_resolved` broadcast (carrying this `action_id`); there is no separate ack. `flee` resolves as a party-wide roll: success = base 60% − 10% per tier (`tier(d) = floor(d/100)`) the encounter is above the party's level tier, floor 5% **[TUNABLE]** — on success the sender's whole party leaves via `battle.participant_left` (and `battle.ended` with `outcome: "fled"` if no allies remain); on failure the turn is consumed and `battle.action_resolved` reports `flee_success: false`. `item` actions consume the item atomically with resolution (`run.backpack_update`).

**Example**

```json
{"type": "battle.submit_action", "seq": 310, "ts": 1783728115000, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "action_id": "0197a601-1234-7abc-9def-0123456789ab", "action": "attack", "skill_kind": null, "item_id": null, "target_ids": ["0197a600-00bb-7abc-9def-0123456789ab"]}}
```

---

### `battle.turn_ready` (S2C)

A combatant's gauge reached 1.0; if it is a player, their 15 s action window opens.

**Source:** CANON.md §B (ATB tick 100 ms; turn timeout 15 s auto-defend).
**Direction:** S2C — broadcast to all battle participants whenever any combatant's gauge fills (player or monster; monster turns resolve server-side immediately after).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| battle_id | string (uuid) | Yes | No | — | The battle. |
| combatant_id | string (uuid) | Yes | No | — | Whose gauge filled. |
| timeout_at | integer (int64, u64) | Yes | Yes | — | Unix millis when the auto-defend timeout fires (now + 15 000 ms **[TUNABLE]**). `null` for monster combatants (monsters act without a window). |

**Example**

```json
{"type": "battle.turn_ready", "seq": 4210, "ts": 1783728114900, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "combatant_id": "0197a600-00aa-7abc-9def-0123456789ab", "timeout_at": 1783728129900}}
```

---

### `battle.gauge_update` (S2C)

Authoritative gauge (and HP/status drift) sync for all combatants.

**Source:** CANON.md §B (gauge fill `speed_stat / 400` per 100 ms tick; battle updates event-driven + 1 Hz keepalive).
**Direction:** S2C — broadcast to all battle participants at least once per second (keepalive), and immediately after any event that changes gauges non-linearly (action resolution, merge, status application). Clients interpolate gauge fill between updates using each combatant's known fill rate; this message corrects drift.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| battle_id | string (uuid) | Yes | No | — | The battle. |
| server_tick | integer (int64) | Yes | No | — | ATB tick number (100 ms cadence) this state was sampled at. Clients drop updates older than the newest received. |
| combatants | array of object | Yes | No | — | One entry per living-or-KO'd combatant. |
| combatants[].combatant_id | string (uuid) | Yes | No | — | The combatant. |
| combatants[].gauge | number (double, 0.0–1.0) | Yes | No | — | Authoritative gauge fill. |
| combatants[].hp | integer (int32, ≥ 0) | Yes | No | — | Authoritative current HP. |
| combatants[].statuses | array of string | Yes | No | — | Active status identifiers. |

**Example**

```json
{"type": "battle.gauge_update", "seq": 4220, "ts": 1783728116000, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "server_tick": 160, "combatants": [{"combatant_id": "0197a600-00aa-7abc-9def-0123456789ab", "gauge": 0.12, "hp": 340, "statuses": []}, {"combatant_id": "0197a600-00bb-7abc-9def-0123456789ab", "gauge": 0.80, "hp": 331, "statuses": []}]}}
```

---

### `battle.action_resolved` (S2C)

The authoritative outcome of one resolved action — player-submitted, monster AI, or auto-defend.

**Source:** GDD.md §5; CANON.md D11 (server computes all outcomes), §B (turn timeout auto-defend; flee math).
**Direction:** S2C — broadcast to all battle participants, in resolution order (envelope `seq` order is the authoritative action order).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| battle_id | string (uuid) | Yes | No | — | The battle. |
| action_id | string (uuid) | Yes | Yes | — | The client-submitted idempotency key; `null` for monster actions and server-initiated auto-defends. |
| actor_id | string (uuid) | Yes | No | — | The acting combatant. Gauge resets to 0.0 on resolution. |
| action | string (enum: `attack`, `skill`, `item`, `defend`, `flee`) | Yes | No | — | The resolved action. |
| auto | boolean | Yes | No | — | `true` when the server acted for the player: 15 s turn timeout, or disconnect auto-defend. Always `action: "defend"` when `true`. |
| flee_success | boolean | Yes | Yes | — | For `flee` actions, whether the roll succeeded; `null` otherwise. Success is followed by `battle.participant_left` for the fleeing party. |
| effects | array of object | Yes | No | — | Per-target results, empty for `defend` and failed `flee`. |
| effects[].target_id | string (uuid) | Yes | No | — | Affected combatant. |
| effects[].kind | string (enum: `damage`, `heal`, `status_applied`, `status_removed`, `ko`, `revive`) | Yes | No | — | Effect type. `ko` accompanies the HP-zeroing effect on the same target. |
| effects[].amount | integer (int32, ≥ 0) | Yes | Yes | — | HP delta for `damage`/`heal`; `null` for status effects and `ko`. |
| effects[].status | string | Yes | Yes | — | Status identifier for `status_applied`/`status_removed`; `null` otherwise. |
| effects[].hp_after | integer (int32, ≥ 0) | Yes | No | — | Target's authoritative HP after the effect — clients render this, never client-side math. |

**Example — auto-defend on turn timeout**

```json
{"type": "battle.action_resolved", "seq": 4290, "ts": 1783728130000, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "action_id": null, "actor_id": "0197a600-00aa-7abc-9def-0123456789ab", "action": "defend", "auto": true, "flee_success": null, "effects": []}}
```

**Example — attack**

```json
{"type": "battle.action_resolved", "seq": 4230, "ts": 1783728115120, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "action_id": "0197a601-1234-7abc-9def-0123456789ab", "actor_id": "0197a600-00aa-7abc-9def-0123456789ab", "action": "attack", "auto": false, "flee_success": null, "effects": [{"target_id": "0197a600-00bb-7abc-9def-0123456789ab", "kind": "damage", "amount": 96, "status": null, "hp_after": 235}]}}
```

---

### `battle.external_effect` (S2C)

An effect injected into the battle from the overworld — e.g. a bystander dropped a health potion onto a battling player's sprite.

**Source:** GDD.md §6 (real-time influence: drop a health potion on Player A's active battle sprite; server intercepts and instantly heals inside the ATB subscreen); CANON.md §S.
**Direction:** S2C — broadcast to all battle participants when a valid [`social.drop_item_on_player`](run-social.md#socialdrop_item_on_player-c2s) targets one of them. Applied immediately on receipt server-side; it does not consume the target's gauge or turn.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| battle_id | string (uuid) | Yes | No | — | The battle. |
| target_id | string (uuid) | Yes | No | — | The combatant who received the effect. |
| source_player_id | string (uuid) | Yes | No | — | The overworld player who dropped the item. Not a battle participant. |
| item_kind | string | Yes | No | — | The consumed item's content identifier (e.g. `health_potion`). |
| effects | array of object | Yes | No | — | Same shape as `battle.action_resolved.effects` (target_id, kind, amount, status, hp_after). |

**Example**

```json
{"type": "battle.external_effect", "seq": 4302, "ts": 1783728118000, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "target_id": "0197a600-00aa-7abc-9def-0123456789ab", "source_player_id": "0197a2f0-33cc-7ddd-9eee-0f1a2b3c4d5e", "item_kind": "health_potion", "effects": [{"target_id": "0197a600-00aa-7abc-9def-0123456789ab", "kind": "heal", "amount": 120, "status": null, "hp_after": 340}]}}
```

---

### `battle.participant_left` (S2C)

One party's combatants left an ongoing battle (voluntary flee success, or disconnect forced flee) while the battle continues for others.

**Source:** GDD.md §5 (auto-flee on disconnect; walking away doesn't pause the fight); CANON.md §B (forced flee always succeeds — structural).
**Direction:** S2C — broadcast to remaining battle participants and to the leavers.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| battle_id | string (uuid) | Yes | No | — | The battle. |
| combatant_ids | array of string (uuid) (min 1 item) | Yes | No | — | The combatants who left (a whole party — flee is party-scoped). |
| reason | string (enum: `fled`, `forced_flee`) | Yes | No | — | `fled` = successful flee roll; `forced_flee` = disconnect rule in a standard encounter (grace expired). |

**Server behavior** — leavers' avatars return to control at their pre-battle overworld positions (a `movement.position_correction` follows for connected leavers; disconnected leavers go `sleeping` there via `world.presence_update`). If no player combatants remain, the battle ends silently server-side (monsters return to roaming; no `battle.ended` is sent to leavers beyond their own `outcome: "fled"` copy).

**Example**

```json
{"type": "battle.participant_left", "seq": 4400, "ts": 1783728140000, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "combatant_ids": ["0197a600-00cc-7abc-9def-0123456789ab"], "reason": "forced_flee"}}
```

---

### `battle.ended` (S2C)

Terminal resolution of the battle for the recipient's party.

**Source:** GDD.md §2 (die → backpack/run-level wipe), §5; CANON.md §B (death & durability), §G (`Battle`, `GatekeeperBoss`), GDD.md §4 (Gatekeeper drops).
**Direction:** S2C — sent to every participant (each party receives its own party-scoped rewards view) when the battle reaches a terminal state: all enemies defeated (`victory`), all player combatants KO'd (`defeat`), or the recipient's party fled (`fled` — sent to the fleeing party alongside `battle.participant_left`).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| battle_id | string (uuid) | Yes | No | — | The battle. |
| outcome | string (enum: `victory`, `defeat`, `fled`) | Yes | No | — | Terminal result for the recipient's party. |
| xp_awards | array of object | Yes | No | — | Per-player XP and run-level gains (empty on `defeat`/`fled`). Fields: `player_id` string (uuid); `xp` integer (int64, ≥ 0); `run_level_after` integer (int32, ≥ 1). Run-level XP follows `xp_to_next(L) = 80 × L^1.6` (CANON.md §B); no run-level cap. |
| loot | array of ItemStack | Yes | No | — | Items added to the recipient's own backpack (loot rolls are server-side; per-player, not shared). Mirrored by a `run.backpack_update`. Empty on `defeat`/`fled`. |
| class_emblem_drops | array of object | Yes | No | — | Gatekeeper victories only, else empty. Fields: `player_id` string (uuid); `emblem_kind` string (e.g. `emblem_of_the_dragoon`). The account-level class unlock itself is a **persistent** mutation applied server-side and visible via the HTTP API — this field is a notification. |
| gatekeeper_cleared | boolean | Yes | No | — | `true` when a Gatekeeper victory sets the per-instance clear flag opening the chokepoint (CANON.md §B); the arena terrain change arrives via re-sent `world.chunk_load`. |

**Server behavior on `defeat`** — each KO'd player's run ends as `died`: backpack and run level deleted; blue-chest gear returned to the Hub at `max_durability × 0.9` (round down, floor 0; gear at 0 is unequippable until repaired) — a persistent server-side mutation (CANON.md §B, D6). A [`run.member_result`](run-social.md#runmember_result-s2c) with `result: "died"` follows immediately.

**Example — victory**

```json
{"type": "battle.ended", "seq": 4500, "ts": 1783728150000, "payload": {"battle_id": "0197a600-0001-7abc-9def-0123456789ab", "outcome": "victory", "xp_awards": [{"player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "xp": 420, "run_level_after": 13}], "loot": [{"item_id": "0197a602-8888-7abc-9def-0123456789ab", "item_kind": "iron_ore", "quantity": 3, "insurance": null}], "class_emblem_drops": [], "gatekeeper_cleared": false}}
```

---

## Disconnect handling in battle

Applied when a battling player's 10 s grace window expires (CANON.md §B; GDD.md §5). Situational, keyed on `encounter_class`:

| encounter_class | Rule | Wire effect |
|-----------------|------|-------------|
| `standard` | **Forced flee** — the disconnected player's party is force-fled; always succeeds (structural), no roll, no flee-chance math. | `battle.participant_left` (`reason: "forced_flee"`) to remaining combatants; if the whole battle was that one party, the battle simply ends for the monsters. Disconnected avatars then sleep at their pre-battle positions (`world.presence_update`). Connected party members of the disconnected player are force-fled with the party. |
| `elite`, `gatekeeper` | **Auto-defend** — no forced flee (protects boss-attempt progression). Each disconnected combatant automatically resolves `defend` (`battle.action_resolved` with `auto: true`) every time its gauge fills, until the battle ends or the player reconnects and resumes control. | Repeated auto `battle.action_resolved`; no participant departure. On reconnect within the same battle, the player resumes from the next `battle.turn_ready`. |

The 15 s turn-timeout auto-defend is independent of disconnects: a **connected** player who simply doesn't act also auto-defends when `timeout_at` passes.
