# Conditions — what holds you, what it does, and what lifts it

An **affliction** does not wear off. A **boon** does. That asymmetry is the whole design: waiting
out a debuff by standing still is not a decision, while a buff that never faded would make the
opening turns of a fight the entire fight.

The classification lives in [`meld_proto::statuses`](../../shared/meld-proto/src/statuses.rs) and
both sides read it. An unrecognised condition counts as a **boon** on purpose — a new boon
mistaken for an affliction becomes permanent and breaks every fight, while the reverse merely
keeps the old timer.

## Afflictions

| Condition | In battle | On the road | Family |
|---|---|---|---|
| `poison` | Damage each turn, a fraction of your own max HP | **Bites per step.** Floors at 1 HP — it grinds, it does not finish | Venom |
| `burn` | As poison, fire-typed | As poison | Venom |
| `slowed` / `web` / `chill` / `bind` | Gauge fills slower | **Drags a march** to `bindings_move_mult` of your speed | Bindings |
| `paralyzed` | **Cannot act.** The gauge fills and the turn is spent standing there. Each held turn you try to break it, on **Will** | — | Bindings |
| `marked` | Everyone hits you harder | — | Senses |
| `distracted` | You swing wide | **Reverses the movement controls** (keyboard/stick; a tap destination is left alone) | Senses |
| `blinded` | You swing wide | **The server stops sending you creatures.** Most of the screen is black | Senses |
| `dread` | **Cannot act on ENEMIES.** Defend, drink, mend yourself or an ally, even swing at one — all still yours | — | Mind |
| `confused` | **Your order is replaced by a random one**: a random action from your own kit, at a random living combatant, friend or foe | — | Mind |
| `frenzied` | **Control is taken away** — it attacks on its own | — | Mind |

## Boons (these still expire, deliberately)

`hasted`, `barrier`, `regen`, `evasion`, `insight`.

## What lifts an affliction

A cure answers a **condition**, not a checklist. A poultice draws venom out and has nothing to
say about being blinded.

| Answer | Lifts |
|---|---|
| Keeper **Poultice** (rung 5) | Venom |
| Resonant **Sanctuary** (rung 35) | Mind |
| **Clarity Draught** | Mind |
| **Panacea** (tier 3) | Everything, and priced like it |
| **A physical hit** | `dread`, `confused` — on a creature as much as a hero |
| **Any healing** | `frenzied` |
| **Will** | `paralyzed`, a small chance each held turn |

The last three matter most: they are the answers that are not a bottle. A martial party with no
mender can slap a frightened ally back into the fight, and the same works on the creature it is
hitting.

## Rules worth knowing before you touch this

- **A wholly paralysed party is a DEFEAT.** Paralysis skips the turn, so without it nobody can
  act while the creatures work through them — the unbounded soft-lock a gauge *cap* used to
  cause. Paralysis is also the one affliction with a way out that is not a cure, and its break
  chance is capped below 1.0 so curing it never becomes pointless.
- **Confusion does NOT roll `Item`.** Drinking your last Panacea by accident spends something you
  cannot get back — a different and much crueller mechanic than swinging at the wrong person.
- **Blindness is enforced server-side.** People will hack the client, so a blinded party is not
  *sent* the creatures. `check_touch` still runs off server positions: you walk into what you
  cannot see and the fight starts anyway. Any future "you cannot see X" belongs in that cull,
  never in the renderer alone.
- **Afflictions are run-scoped**, carried on `hero_afflictions` beside `hero_hp`, so walking away
  from what poisoned you is not a cure.
- Magnitudes are `[affliction]` and `[battle]` `[TUNABLE]`s. `base_ward` is 0 for martials and 1
  for casters because ward is subtracted FLAT: at 4 it floored an early creature ability from 4
  damage to 1 and made a starting party nearly immune to the elemental half of the game.
