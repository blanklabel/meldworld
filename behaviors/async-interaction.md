# Asynchronous MMO Interaction

How players who are not in combat shape the world for those who are: dropping Backpack items onto the overworld for others to pick up, dropping consumables onto a battling player's sprite so the server injects the effect into their active ATB battle, and deploying Ward items over sleeping (disconnected) allies. All of these are **ephemeral** interactions carried over the realtime protocol (CANON §S: drops, battles, presence are WebSocket domain); none of them touch persistent state until an extraction banks the result.

**Source:** GDD.md §5 (Sleep Mechanics, Protective Items), §6; CANON.md §B (Disconnect handling, Ward items), §G (Backpack, Sleeping, Ward, Battle), §S

Related specs: [economy.md](economy.md) (the taxed, persistent trade channels these drops deliberately bypass), [../edge-cases/limits.md](../edge-cases/limits.md).

---

## Backpack Item Drops (Overworld)

A player may drop any item from their ephemeral Backpack onto the overworld tile they stand on (or an adjacent tile), creating a world drop entity that other players in the same `MazeInstance` can pick up. This is the organic-cooperation channel: gearing up an ally mid-run, paying a bodyguard, or shedding weight before a risky push.

**Source:** GDD.md §6 (Backpack Dropping); CANON.md §G (Backpack, Chunk), §S (realtime channel)

### Flow

1. Player selects an item (and quantity, for stackables) in their Backpack and drops it at their position. Realtime message; the server validates the player is out of battle, alive, and owns the item.
2. The server removes the item from the dropper's Backpack and spawns a world drop entity at the tile, broadcast to all connections whose interest area (2-chunk radius) covers it.
3. Another player walks onto/adjacent to the drop and picks it up. The server moves the item into their Backpack and despawns the entity, broadcasting the removal.
4. If nobody picks it up within the despawn timer, the server deletes the drop permanently.

### Rules

| Rule | Behavior |
|------|----------|
| Visibility | Visible to all members of the same `MazeInstance` (instances are isolated worlds — drops never cross instances, including merged-battle guests, who are in the battle but not in the instance's overworld). Rendered to any client whose interest radius (2 chunks) covers the drop. |
| Who can pick up | Any live, non-battling player in the instance, **including the dropper**. First-come, first-served: pickup is atomic; the second pickup attempt fails silently (drop no longer exists). No reservation, ownership window, or party-priority. **[TUNABLE]** |
| Despawn timer | **5 minutes** from drop **[TUNABLE]**. Deletion is permanent — despawned items are destroyed, not returned. |
| Capacity | Pickup fails (client-visible "backpack full" rejection over realtime, no state change) if the receiver's Backpack is at capacity (40 slots **[TUNABLE]**, see [../edge-cases/limits.md](../edge-cases/limits.md)). The drop remains on the ground. |
| What can be dropped | Backpack items only: consumables, materials, red-chest gear, chits-as-item is **not** droppable (chits is a counter, not an item — keeps the tax sink un-bypassable, see [economy.md](economy.md)). Blue-chest (equipped/insured) gear is Vault-side and cannot be dropped in the maze. |
| Instance close | All outstanding drops are deleted when the instance closes. Drops are never persisted. |
| While battling / sleeping | A player in an active Battle or in the `sleeping` state cannot drop or pick up. |

---

## Consumable Injection into Active Battles

A player on the overworld can drop a consumable **onto the battle sprite of a player who is currently inside an ATB subscreen**. The server intercepts the drop and injects the item's effect directly into that player's active `Battle` — e.g. dropping a health potion on Player A's sprite instantly heals Player A inside their battle.

**Source:** GDD.md §6 (Real-Time Influence); CANON.md §D (D11 — server-authoritative combat), §G (Battle), §S

### Flow

1. Player B (out of combat, same instance) targets Player A's overworld battle sprite with a consumable from B's Backpack.
2. The server validates: A is in an active Battle in the same instance; the item is an **injectable** type; B is within interaction range (adjacent to the sprite **[TUNABLE]**).
3. The server consumes the item from B's Backpack and applies the item's effect to A inside the Battle immediately — as a server-initiated battle event, not an action on anyone's ATB gauge (it costs A no turn and does not wait for a gauge).
4. Both A's battle UI and B's overworld client receive confirmation events. A sees the effect with attribution ("B used Potion on you").

### Rules

| Rule | Behavior |
|------|----------|
| Injectable item types | **Healing and curative consumables only** (HP restore, status-cure, revive) **[TUNABLE]**. Buff, offensive, and utility items are rejected — no state change, item retained. |
| No hostile injections | Nothing can be injected that targets the enemies or harms/debuffs the battling player. There is no mechanism to affect a battle negatively from the overworld. (Anti-grief, see below.) |
| Timing | Effect applies at the next server battle tick (≤ 100 ms). If the battle ended between send and processing, the injection fails and the item is **retained** by B. |
| Dead target | If A is KO'd in-battle, only revive-class items are accepted; others are rejected with item retained. |
| Who can inject | Any member of the same MazeInstance; party membership not required. Rate-limited per the realtime message limits in [../edge-cases/limits.md](../edge-cases/limits.md). |

---

## Ward Deployment over Sleeping Allies

When a disconnected player's avatar is left `sleeping` on the overworld (after the 10 s reconnect grace window and any forced-flee/auto-defend resolution), it is attackable by roaming monsters. Active players can protect it by deploying a consumable `WardItem` from their Backpack onto the sleeping avatar's tile.

**Source:** GDD.md §5 (The "Sleeping" State, Protective Items); CANON.md §B (Disconnect handling — Ward items), §G (Ward, Sleeping)

### Flow

1. Player B targets a sleeping avatar in their instance with a `WardItem` (`warding_tent` or `sanctuary_campfire`).
2. The server consumes the item from B's Backpack and attaches the ward to the sleeping avatar, broadcasting the visual to nearby clients.
3. For the ward's duration, the sleeping avatar is **invisible to monster pathfinding** — roaming monsters cannot target or collide-attack it.
4. The ward expires after its duration, or is discarded early if the sleeper reconnects and moves, or the instance closes.

### Ward types

| Ward | Duration | Effect |
|------|----------|--------|
| `warding_tent` | 30 min **[TUNABLE]** | Invisibility to monster pathfinding |
| `sanctuary_campfire` | 10 min **[TUNABLE]** | Invisibility to monster pathfinding + slow HP regen aura on the sleeper |

### Rules

| Rule | Behavior |
|------|----------|
| Stacking | One active ward per sleeping avatar; deploying a second replaces the first (remaining duration of the first is lost) **[TUNABLE — replacement vs. rejection chosen by this spec]**. |
| Valid targets | Sleeping avatars in the same instance. Wards can also be deployed on a connected player's own tile as pre-emptive shelter **[TUNABLE — GDD describes only the sleeping-ally use]**. |
| Mid-attack | A ward deployed while a monster is already engaged with the sleeper does not cancel the in-progress Battle; it prevents new acquisitions only. |
| Instance close | Sleeping avatars persist until the instance closes (all members extracted/died/abandoned, or 60 min with all members disconnected → auto-abandon: Backpack lost as a death, **no durability loss**). Wards do not extend instance lifetime. |

---

## Anti-Grief Rules

**Source:** GDD.md §6; CANON.md §D (D11); consolidated by this spec.

- **No hostile injections.** The overworld→battle channel is help-only: healing/curative items only, targeting the battling *player* only. There is no item, action, or message by which an outside player can damage, debuff, or otherwise hinder someone's battle.
- **Drops are first-come.** World drops carry no ownership; dropping an item is an unconditional gift to the instance. Clients must present drops as irrevocable ("scam" disputes are out of scope by design).
- **No drop-blocking.** Drop entities have no collision; they cannot be used to obstruct chokepoints.
- **Wards are strictly protective.** Wards cannot be placed on monsters, hostile players do not exist (no PvP anywhere), and there is no "reverse ward" that reveals a hidden sleeper.
- **Consumption honesty.** Injected/deployed items are consumed only on successful application; every rejection path retains the item.
- **Rate limits** on realtime messages (30 msgs/s per connection **[TUNABLE]**, see [../edge-cases/limits.md](../edge-cases/limits.md)) bound drop/pickup spam.

## Notes

- Merged battles complicate sprite targeting: a merged Battle can contain players from up to 4 instances (Gatekeepers), but each battler's overworld sprite exists only in their own instance. Injection therefore reaches only battlers from the injector's instance, even though the Battle itself spans instances.
- GDD §6 frames dropping as usable for "paying bodyguards" — but since chits is not droppable (this spec), payment must be in items or via taxed hub channels. Flagged as a mild tension with GDD flavor text; resolved in favor of chits-conservation ([economy.md](economy.md) invariant I1).
