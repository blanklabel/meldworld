# Combat: Instanced ATB Battles

Combat never happens on the overworld canvas. Touching an enemy pulls the toucher's party into a server-side `Battle` entity, rendered as an ATB (Active Time Battle) subscreen overlay. All ATB math — gauge timers, damage, status, loot rolls — is computed server-side on a fixed tick; clients render state and submit action intents only (CANON.md §D11). Battles run in real time whether or not the player is looking at them: walking away from the device does not pause the fight.

**Source:** GDD.md §5, §6; CANON.md §B (ATB combat), §D5, §D11, §G (Battle), §S

Related: [run-lifecycle.md](./run-lifecycle.md) (what victory/defeat/flee do to the run), [disconnect-handling.md](./disconnect-handling.md) (auto-flee/auto-defend on disconnect), [world-generation.md](./world-generation.md) (`mlevel`/`stat_mult` inputs), [../interfaces/realtime-protocol.md](../interfaces/realtime-protocol.md) (`battle.*` messages).

---

## Flow: Battle Creation on Touch

**Source:** GDD.md §5; CANON.md §G (Battle), §D11

1. On the overworld, a player avatar and a monster come into contact (either may initiate movement; contact detection is server-validated).
2. The server creates a `Battle` entity containing the touching player's party-side combatants and the monster encounter group (composition, `mlevel(d)`, `stat_mult(d)` from the touch position's `distance` — see [world-generation.md](./world-generation.md)).
3. The involved players' clients are switched to the ATB subscreen overlay via `battle.*` S2C messages; the overworld avatar is marked in-battle at its position.
4. All combatant ATB gauges start at 0. The server tick loop (below) begins immediately.
5. Special creation cases:
   - Touching an enemy **already in a battle** triggers a **battle merge** (below), not a new battle.
   - A monster touching a **sleeping avatar** creates a battle in which the sleeper permanently auto-defends — see [disconnect-handling.md](./disconnect-handling.md).
   - Touching a `GatekeeperBoss` creates (or merges into) its arena battle; flee is disabled (below).

---

## Server-Authoritative ATB Loop

**Source:** CANON.md §B (ATB combat), §D11, §S

1. The server advances every active `Battle` on a **100 ms tick [TUNABLE]**.
2. Each tick, every living combatant's ATB gauge increases by `speed_stat / 400` **[TUNABLE]**. The gauge is **full at 1.0**; it does not accumulate past full.
3. When a combatant's gauge is full:
   - **Monsters/NPC combatants:** the server selects and resolves their action (AI policy is content-defined).
   - **Players:** the combatant becomes eligible to act; the client is prompted. The gauge stays full until an action resolves or the turn timeout fires.
4. After a combatant's action resolves, their gauge resets to 0 and refills.
5. Battle state deltas are pushed to all participating clients event-driven, with a 1 Hz keepalive (non-binding perf target, CANON.md §B Networking targets).
6. Clients submit **intents only** (`battle.*` C2S); the server validates legality (actor alive, gauge full, target valid, item present in `Backpack`) and computes every outcome. Client-computed results are never trusted.

---

## Actions and Resolution Ordering

**Source:** GDD.md §5; CANON.md §B (ATB combat), §D11

| Action | Requirements | Effect |
|--------|--------------|--------|
| `attack` | gauge full; living target | Weapon damage to target (server damage formula, content-defined) |
| `skill` | gauge full; class skill known at current `run_level` build; any resource costs met | Skill effect (damage/heal/status per content tables) |
| `item` | gauge full; consumable present in actor's `Backpack` | Consumes the item from the Backpack; applies its effect |
| `defend` | gauge full | Damage-reduction stance until the actor's next action (reduction value content-defined) |
| `flee` | gauge full; encounter is not a `GatekeeperBoss` | Attempts escape per the flee formula below |

Resolution ordering rules:

1. Actions resolve **serially**, one at a time, in the order the server accepts them (server receipt order for player intents; tick order for AI actors whose gauges fill on the same tick). There are no simultaneous resolutions. *(Ordering is a spec-level resolution of a canon gap — flagged in Edge Cases.)*
2. An action legal when submitted but illegal when it comes up for resolution (target already dead, actor stunned/dead) is dropped; a dropped player action leaves the gauge full and the player may re-submit.
3. `item` consumption from the `Backpack` happens at resolution time, atomically with the effect.

### Turn timeout

An actor whose gauge is full and who submits no action within **15 s [TUNABLE]** automatically performs `defend` (gauge resets to 0). This repeats indefinitely — an unresponsive player perpetually auto-defends while enemies keep acting.

**Source:** CANON.md §B (ATB combat: turn timeout)

---

## Flee

**Source:** CANON.md §B (ATB combat: flee); GDD.md §5

```
flee_chance = clamp(0.60 − 0.10 × max(0, encounter_tier − party_tier), 0.05, 0.60)
```

- Base **60%**, minus **10% per tier** the encounter is above the party's level tier, never below **5%** **[TUNABLE]**.
- `encounter_tier` = `tier(d)` of the encounter's spawn distance. `party_tier` is the tier equivalent of the party's level: `floor(avg_run_level / 8)` (derived by inverting `mlevel(d) = d/12.5` through `tier(d) = floor(d/100)`; spec-level derivation — flagged in Edge Cases).
- **Gatekeepers: flee is disabled** — the `flee` action is not offered/accepted in a `GatekeeperBoss` battle.
- A successful flee removes the fleeing player's side from the battle and returns their avatars to the overworld (see Outcomes). A failed flee consumes the actor's turn (gauge resets to 0).
- The **forced flee** applied to a disconnected party in a standard encounter bypasses this formula and always succeeds (structural) — see [disconnect-handling.md](./disconnect-handling.md).

---

## Battle Merge (Expandable Party / Raid)

**Source:** GDD.md §5; CANON.md §B (ATB combat: battle merge), §D5, §D13

1. If a player touches an enemy that is already in a `Battle` (including a `GatekeeperBoss`), their party **joins that battle** — the battle merges parties; the `MazeInstance`s themselves are never merged (CANON.md §D13).
2. Merge caps **[TUNABLE]** (CANON.md §D5):

   | Encounter type | Max instances in one battle | Max player combatants |
   |----------------|-----------------------------|-----------------------|
   | Normal | 2 | 8 |
   | `GatekeeperBoss` | 4 | 16 |

   A touch that would exceed the cap does not merge (the toucher bounces off; no new battle is created against an already-engaged enemy).
3. Every joining combatant **enters at gauge 0**.
4. **No mid-fight rescale:** enemy stats, HP, and composition do not change when parties join or leave.
5. Gatekeeper HP pools are **sized for 8 combatants at spawn**, regardless of how many actually fight.
6. The battle UI expands to show all active parties; battle state is synced to every participating client.

---

## "Walking Away Doesn't Pause"

**Source:** GDD.md §5; CANON.md §B (ATB combat)

1. A `Battle` advances on the server tick regardless of player attention, input, app focus, or connectivity.
2. Enemy gauges fill and enemy actions resolve normally while a player is unresponsive; the player's own full gauge converts to `defend` every 15 s (turn timeout).
3. There is no pause mechanic anywhere in maze combat. The only ways a battle stops affecting a player are its terminal outcomes (victory, defeat, flee) or the disconnect rules in [disconnect-handling.md](./disconnect-handling.md).

---

## Flow: External Heal Injection (Overworld → Battle)

**Source:** GDD.md §6; CANON.md §S

1. Player B, on the overworld and **not** in the battle, drops a consumable (e.g. a health potion) from their `Backpack` onto Player A's in-battle avatar sprite.
2. The server validates: item exists in B's `Backpack`; A is in an active battle in the same instance; drop position coincides with A's avatar.
3. The item is consumed from B's `Backpack` and its effect is applied to A **inside the ATB battle immediately** (next tick), without consuming any combatant's turn or gauge.
4. All battle participants see the effect via battle state sync. Ordinary backpack drops (not onto a battle sprite) simply place the item on the overworld for pickup (GDD.md §6) and do not interact with battles.

---

## Battle Outcomes

**Source:** GDD.md §2.2, §5; CANON.md §B (ATB combat, Death & durability), §G

| Outcome | Trigger | Battle effect | Run-lifecycle consequence ([run-lifecycle.md](./run-lifecycle.md)) |
|---------|---------|---------------|--------------------------------------------------------------------|
| Victory | All enemy combatants defeated | Battle ends; server rolls loot (banded by `tier(d)`, red floor `d ≥ 300`) into participants' `Backpack`s; XP awarded; `ClassEmblem` drops on Gatekeeper kills; per-instance Gatekeeper clear flag set | Runs stay `active`; run-level-ups applied |
| Defeat | All combatants on a player side defeated | That side's players are removed; battle ends when no player side remains (monsters persist) | Each defeated player's run → `died` (backpack + run levels deleted, blue gear at ×0.9 durability) |
| Flee (voluntary) | Successful `flee` roll | Fleeing side removed; avatars return to overworld near the encounter; no loot, no XP for the fled encounter | Run stays `active` |
| Forced flee (disconnect, standard encounter) | Disconnect grace expiry | Always succeeds (structural); avatar left `sleeping` on overworld | Run stays `active`; see [disconnect-handling.md](./disconnect-handling.md) |
| Partial outcome (merged battles) | One party wiped/fled while another fights on | Battle continues for remaining parties; outcomes applied per side as they occur | Per-player consequences as above |

---

## XP and Run-Level-Up During Runs

**Source:** CANON.md §B (Hubs & run levels: XP); GDD.md §2.2, §4

1. Victory awards combat XP to each surviving participant (per-monster XP values are content-defined **[TUNABLE]**).
2. XP accumulates against `xp_to_next(L) = 80 × L^1.6` **[TUNABLE]**; on crossing the threshold, `run_level` increments (multiple levels per battle possible). There is **no run level cap**.
3. `run_level` gains are ephemeral: they exist only within the current run and are deleted at any terminal transition (even `extracted` — extraction banks the Backpack, never the level).
4. Combat XP is entirely separate from `MeldSkill` XP (which is credited only in hubs and on extraction).

---

## Invariants

1. **Server authority:** every gauge value, damage number, status application, flee roll, loot roll, and XP award is computed server-side; clients submit intents and render results.
2. **Real-time only:** battles never pause; gauge fill is a pure function of ticks elapsed and `speed_stat`.
3. **One action per full gauge:** an actor can have at most one action resolve per gauge fill; the gauge resets to 0 on resolution (including auto-defend).
4. **No mid-fight rescale:** enemy stats and HP never change due to merge, leave, disconnect, or party size after spawn.
5. **Merge caps are hard:** a battle never exceeds 2 instances / 8 players (normal) or 4 instances / 16 players (Gatekeeper) [TUNABLE values, hard once configured].
6. **Gatekeeper no-flee:** `flee` is never accepted in a `GatekeeperBoss` battle — including disconnect handling (auto-defend, never auto-flee).
7. **Loot flows to Backpack only:** battle rewards never write to the `Vault` directly ([run-lifecycle.md](./run-lifecycle.md) invariant 2).
8. **Battles are ephemeral:** a `Battle` lives in instance memory and is discarded with the instance; no battle state persists.

---

## Edge Cases

- **Resolution ordering is a canon gap:** CANON defines gauge mechanics but not what happens when several gauges fill on one tick; this spec mandates serial resolution in server-acceptance order. Flagged for design confirmation.
- **`party_tier` definition is derived:** CANON says "per tier the encounter is above party level tier" without defining a level→tier mapping; this spec uses `floor(avg_run_level / 8)`. Flagged for design confirmation.
- **Target dies before action resolves:** action is dropped; player actions may be re-submitted (gauge remains full), so a dropped action never silently wastes a turn.
- **Item drop race:** if B's heal lands the same tick A's side is defeated, defeat wins (terminal outcomes resolve before external injections queued later in the tick); the item is not consumed if the effect cannot apply.
- **Merge attempt above cap:** no error subscreen; the toucher simply does not enter the battle and remains on the overworld.
- **All players in a merged battle disconnect:** disconnect rules apply per side; a Gatekeeper battle can proceed with every player side auto-defending until wipe or reconnect ([disconnect-handling.md](./disconnect-handling.md)).
- **Speed extremes:** `speed_stat ≥ 400` fills the gauge in a single tick; implementations must still enforce one resolution per fill (no multi-action per tick from overshoot).
- **15 s timeout vs. flee:** auto-defend (timeout) is not a flee; an AFK player in a standard encounter stays in battle indefinitely until the enemy kills them or an ally intervenes — only a *disconnect* triggers forced flee.
- **Fled-encounter monster state:** after a voluntary/forced flee, the monster remains on the overworld (with its current HP — no rescale/reset rule in canon; flagged as a content decision).
- **Zero-player battle:** if every player side flees or wipes, the `Battle` entity ends; monsters return to overworld behavior.
