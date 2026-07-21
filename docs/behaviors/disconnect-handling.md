# Disconnect Handling

Meldworld is a mobile-first game where losing cell service mid-run is expected, not exceptional. The server intercepts every connection drop and applies situational rules so a disconnect is survivable but never free: a short silent grace window, context-dependent battle resolution (forced flee vs. auto-defend), a vulnerable `sleeping` avatar left on the overworld, consumable wards that hide sleepers from monsters, seamless reconnection, and a 60-minute all-disconnected backstop that abandons the run. All disconnect logic is server-side; the client cannot influence it after the connection is gone.

**Source:** GDD.md §5; CANON.md §B (Disconnect handling), §G (Sleeping, WardItem), §S

Related: [run-lifecycle.md](./run-lifecycle.md) (abandon/death consequences), [combat-atb.md](./combat-atb.md) (auto-defend, forced flee mechanics), [../interfaces/realtime-protocol.md](../interfaces/realtime-protocol.md) (`session.*` connection lifecycle messages).

---

## Flow: Detecting a Disconnect (Grace Window)

**Source:** CANON.md §B (Disconnect handling: grace window); GDD.md §5

1. The server detects a dropped realtime connection (socket close, missed heartbeats).
2. A **10 s grace window [TUNABLE]** begins. During the grace window nothing observable changes: the avatar stays where it is, battles continue exactly as if the player were merely unresponsive (enemy keeps acting; the player's full gauge auto-defends on the 15 s turn timeout — see [combat-atb.md](./combat-atb.md)).
3. If the player reconnects within the grace window, the session resumes silently; no disconnect rule fires and other players see nothing (beyond any missed inputs).
4. If the grace window expires, the player is formally **disconnected** and the rules below fire based on the player's situation at that moment.

---

## In-Battle Disconnect Rules

**Source:** GDD.md §5; CANON.md §B (Disconnect handling: standard/elite)

Rules apply per party side when the grace window expires while the player is in a `Battle`:

| Encounter type | Rule | Detail |
|----------------|------|--------|
| Standard encounter | **Forced flee** | The disconnected party is forced to "flee" the subscreen. The forced flee **always succeeds** (structural — it bypasses the flee formula entirely, including the 5% floor case). The battle ends for that side; the avatar transitions to `sleeping` on the overworld. |
| Elite encounter | **Auto-defend** | No auto-flee. The disconnected party's combatants enter an auto-defend state: every time a gauge fills, the combatant performs `defend`. This continues until the battle resolves (victory/defeat) or the player reconnects and retakes control. |
| `GatekeeperBoss` | **Auto-defend** | Same as elite — Gatekeeper progress (and shared boss attempts with merged parties) is never thrown away by a disconnect. Flee remains disabled ([combat-atb.md](./combat-atb.md)). |

Consequences:

- Auto-defend can still end in **defeat**: if the boss wipes the auto-defending side, those players' runs end in `died` with full death penalties ([run-lifecycle.md](./run-lifecycle.md)) — auto-defend prevents throwing away the attempt, it does not grant immunity.
- Auto-defend can end in **victory**: the disconnected player still receives loot into their `Backpack` and XP as a battle participant.
- After an in-battle resolution (forced flee, or auto-defend battle ending) while still disconnected, the avatar transitions to `sleeping` on the overworld.
- Elite vs. standard classification is a property of the encounter (content tables); `GatekeeperBoss` is always critical.

---

## Flow: The Sleeping Avatar

**Source:** GDD.md §5; CANON.md §B (Disconnect handling: sleeping avatar), §G (Sleeping)

1. Once out of battle, the disconnected avatar is left `sleeping` on the world map at its current position, visible to other players.
2. Sleeping is **not safe**: the avatar remains attackable. If a roaming monster's pathfinding reaches and touches the sleeping avatar, the server creates a `Battle` in which the sleeper participates alone (unless allies merge in) and **auto-defends every turn** — identical mechanics to the auto-defend state above, for the whole battle regardless of encounter type (a sleeper cannot flee).
3. If that battle ends in defeat, the sleeper's run ends in `died` with full death penalties: Backpack and run levels deleted, blue gear returned at `max_durability × 0.9` ([run-lifecycle.md](./run-lifecycle.md)).
4. Allies can defend a sleeper by touching the attacking monster (battle merge, [combat-atb.md](./combat-atb.md)) or by external heal injection onto the sleeper's battle sprite.
5. The sleeping avatar persists on the overworld until the player reconnects or the `MazeInstance` closes.

---

## Ward Items

Active players can protect sleeping allies by deploying consumable `WardItem`s over them on the map.

**Source:** GDD.md §5; CANON.md §B (Disconnect handling: ward items), §G (Ward)

| Item (`item_kind`) | Duration | Effect |
|--------------------|----------|--------|
| `warding_tent` | 30 min **[TUNABLE]** | Avatars under the ward are **invisible to monster pathfinding** — roaming monsters neither path toward nor touch them, so no battle can start against a warded sleeper. |
| `sanctuary_campfire` | 10 min **[TUNABLE]** | Same pathfinding invisibility, plus a **slow HP regen aura** on avatars in its area. |

Rules:

1. Deploying a ward consumes the item from the deployer's `Backpack` and places an area effect on the overworld at the target position (typically over a sleeping ally).
2. Wards affect monster **pathfinding/aggro only**; they do not end a battle already in progress when deployed.
3. Ward effects apply to any avatar within the area (sleeping or awake); their protective purpose is for sleepers, who cannot move out of danger.
4. When the duration expires, the ward despawns and the sleeper is attackable again. Wards are instance-scoped overlay state, discarded at instance close.

---

## Flow: Reconnect / Resume

**Source:** GDD.md §5; CANON.md §B (Disconnect handling), §I (session domain)

1. The player re-authenticates and re-establishes the realtime connection (`session.*` messages — see [../interfaces/realtime-protocol.md](../interfaces/realtime-protocol.md)).
2. The server resumes the existing run in place — reconnection never resets, relocates, or re-rolls anything:
   - **Sleeping on the overworld:** the avatar wakes at its current position under whatever conditions exist there (including an active ward); control returns immediately.
   - **In an auto-defend battle (elite/Gatekeeper, or sleeper-ambush):** the player rejoins the live battle at its current state and retakes manual control from the next gauge fill; auto-defend stops.
   - **During the 10 s grace window:** silent resume, nothing ever fired.
3. The server replays/refreshes required state to the client (current chunks, battle state, backpack, run_level); the run continues as `active`.
4. If the run reached a terminal state while disconnected (killed while sleeping → `died`; 60-min auto-abandon → `abandoned`), the reconnecting player is returned to the Hub and shown the outcome; there is nothing to resume.

---

## 60-Minute All-Disconnected Auto-Abandon

**Source:** CANON.md §B (Disconnect handling: instance close); GDD.md §2.2

1. The server tracks connectivity per `MazeInstance`. When **all** members are disconnected simultaneously, an instance-wide timer runs; any single member reconnecting cancels and resets it.
2. After **60 continuous minutes [TUNABLE]** with all members disconnected, the instance closes and every still-`active` run transitions to `abandoned` ([run-lifecycle.md](./run-lifecycle.md)); sleeping avatars are removed with the instance.
3. Auto-abandon **counts as death for the Backpack**: Backpack contents and accumulated run levels are deleted, exactly as in a death.
4. **BUT: no durability loss.** Blue-chest gear is returned to the Hub at its current `max_durability`, *without* the `× 0.9` death penalty. This is an explicit, deliberate difference from normal death — a connectivity failure costs the run's loot but never degrades permanent gear. (CANON.md §B: "counts as death for backpack, no durability loss".)
5. If even one member remains connected, no timer runs and disconnected members can sleep indefinitely — until killed, warded, reconnected, or all members eventually go terminal/disconnected.

---

## Invariants

1. **Grace is silent:** within the 10 s window, no observable state change occurs and no disconnect rule fires.
2. **Forced flee never fails:** the standard-encounter forced flee is structural — it succeeds unconditionally, unlike the voluntary flee roll.
3. **No auto-flee from critical fights:** elite and `GatekeeperBoss` battles never end via disconnect; they end only by victory or defeat (or, for the disconnected side, reconnect-and-fight-on).
4. **Sleeping ≠ safe:** an unwarded sleeping avatar is always attackable; only wards (and instance close) remove it from monster pathfinding.
5. **Sleepers only defend:** a sleeping avatar's battle participation is auto-defend, every turn, in every encounter type; it never attacks, flees, or uses items.
6. **Reconnect is lossless in kind:** reconnection restores the player to exactly the state the server evolved to (never a checkpoint or rollback); disconnects are handled forward-only.
7. **Auto-abandon ≠ death for gear:** the 60-min auto-abandon deletes the Backpack and run levels but never applies durability loss — the only backpack-deleting path with intact gear.
8. **Server-only:** all disconnect detection, timers, forced actions, and abandonment are server-side (CANON.md §S); a client cannot fake, block, or trigger them.

---

## Edge Cases

- **Disconnect during the extraction channel:** the channel is interrupted (it requires an active, stationary player); after grace expiry the avatar sleeps un-extracted. Nothing banks.
- **Disconnect during a battle merge (multi-party fight):** rules apply to the disconnected side only; other parties fight on. A standard-encounter forced flee removes just the disconnected side; in Gatekeeper fights the disconnected side auto-defends while allied sides keep attacking.
- **Reconnect exactly as auto-defend battle resolves:** resolution wins ties — if the battle reached victory/defeat before the resume completes, the player receives the outcome, not control.
- **Ward deployed after a monster already touched the sleeper:** too late — the battle exists; wards only prevent future pathfinding acquisition.
- **Ward expiry with monster adjacent:** the sleeper becomes visible to pathfinding immediately on expiry; there is no post-ward grace.
- **Stacked/overlapping wards:** canon does not define stacking; this spec resolves that effects do not stack — the longest-remaining invisibility applies, and `sanctuary_campfire` regen applies while within any active campfire area. Flagged for design confirmation.
- **All members disconnected but one avatar is mid-battle (auto-defending a Gatekeeper):** the 60-min timer still runs; if the battle is still unresolved at expiry, the instance closes and the run is abandoned (backpack deleted, no durability loss). Battle state is discarded with the instance.
- **Serial disconnects that never overlap:** the 60-min timer requires *all* members disconnected *simultaneously*; a party trading off connectivity never triggers it.
- **Solo player:** disconnecting alone starts the all-disconnected timer immediately after grace expiry; a solo sleeper has at most 60 min (minus battle time) to reconnect before auto-abandon.
- **Killed while sleeping, then reconnect:** run is already `died`; death penalties (including durability ×0.9) stand — the no-durability-loss rule applies only to the auto-abandon path, not to deaths that happen while disconnected.
- **App backgrounded but socket alive:** not a disconnect — the player is merely unresponsive; the 15 s auto-defend turn timeout governs ([combat-atb.md](./combat-atb.md)), and no sleeping state occurs.
