# Proposal — Gear identity: class-locked kit, ephemeral clarity, and builds

> Status: **proposed** (design of record for roadmap `GR-5`, `GR-6`, and the
> `AD-1`/`AD-3` build layer). Folds into [`CANON.md`](../CANON.md) §G + a D-number
> when the first slice lands. Extends [`Epic GR`](../ROADMAP.md) and
> [`gear-item-models.md`](../interfaces/data-models/gear-item-models.md).

Today every piece of gear fits every hero, `gear.class_key` is written `''`
(unrestricted) on every row, and a drop's only interesting quality is that its
numbers are bigger. Three consequences:

- **Classes don't read as classes.** A Resonant healer swinging the same blade as
  the Explorer next to it has no silhouette of its own.
- **"Red" means nothing to a player.** The wire says `red`; the fiction says
  Red-Chest; neither says *this vanishes when the run ends*.
- **There are no builds.** Gear is a single scalar ladder, so there is no
  decision to make and nothing to plan a party around.

This proposal fixes all three, in that order.

---

## 1. Class-locked equipment

Every equippable item declares a **family**. Every class declares which families
it may wear. The server rejects an illegal equip; nothing about the check lives
on the client (CANON §S).

### Weapons

| Family | Hands | Slot | Worn by |
|--------|-------|------|---------|
| `sword` | 1 | main | Explorer |
| `shield` | 1 | off | Explorer, Iron Hull |
| `spear` | **2** | main (+reserves off) | Explorer |
| `staff` | **2** | main (+reserves off) | Resonant |
| `globe` | **2** | main (+reserves off) | Psyker |
| `gauntlet` | 1 | main | Iron Hull |
| `dagger` | 1 | main **or** off | Shifter |
| `parry_blade` | 1 | off | Shifter |

Which gives each class a recognizable hand:

- **Explorer** — sword + shield, or a spear that takes both hands. The martial
  baseline is the only class with a real weapon *choice* (defensive vs reach).
- **Resonant** — staff only, two-handed. A healer's hands are full.
- **Psyker** — globe only, two-handed: the Foci are channeled through it.
- **Iron Hull** — gauntlet + shield. The tankiest class is also the only one that
  cannot reach past its own arms.
- **Shifter** — dagger main-hand, with **two legal off-hands**: a second dagger
  (dual-wield, aggressive) or a parrying blade (defensive, leans on the class's
  innate dodge and Flicker). The one class whose off-hand is a build decision.

**Two-handed rule.** A 2-hand weapon occupies `main_hand` and *reserves*
`off_hand`. Equipping one with a filled off-hand is a `409` (the client offers to
unequip); the reserved off-hand renders as occupied-by-weapon, not as empty. This
makes "spear or sword+shield" a genuine trade rather than a strict upgrade.

### Armor

Armor pieces (`head`, `chest`, `legs`) declare a **weight**, and each class
allows a *set* of weights — so most drops are useful to more than one hero,
which is the point of weight classes rather than per-class armor:

| Class | Allowed weights |
|-------|-----------------|
| Iron Hull | `heavy`, `medium` |
| Explorer | `medium`, `light` |
| Shifter | `light` |
| Resonant | `robe`, `light` |
| Psyker | `robe` |

Plus **signature pieces**: rare, class-exclusive armor that names one class
(`class_key` set), ignores the weight table, and carries a class-flavored keyword
affix — a Psyker's crown that adds a Focus slot, an Iron Hull cuirass that
converts a fraction of incoming damage into Barrier. These are the armor arm of
`AD-1`'s uniques: the pieces you *chase* because only one class can wear them and
they change how that class plays.

### Accessories

Accessories stay unrestricted (both accessory slots, any class). Every loot table
needs a family that is never a dead drop, and accessories are it.

### Enforcement points

1. **Equip** (`PUT`/`POST` equip): illegal family, illegal weight, wrong
   `class_key`, or a two-handed conflict → `409 conflict` with a code that says
   which rule failed. The class rule is checked **before** the hands rule, so a
   shield offered to a Resonant is answered "your class cannot wield that", not
   "your hands are full" — the more specific answer wins. This needs a persisted
   hero class (`GR-7`).
2. **Derivation** (`meld-run::party_fighters`): already ignores gear whose
   `class_key` mismatches the hero — the guardrail exists; this widens it to
   family/weight so a legacy row can never silently buff the wrong hero.
3. **Loot generation**: rolls a family appropriate to *some* class, and stamps
   `class_key` only for signature pieces. A drop that fits nobody in your party
   is still worth extracting — it sells, and it fits your next party.

---

## 2. "Red" becomes "Ephemeral"

`Insurance::Red` is renamed **`Ephemeral`** and `Insurance::Blue` becomes
**`Insured`** on the wire and in every player-facing string. The chest-colour
fiction (Blue-Chest / Red-Chest, CANON §G) stays in the lore; it stops being the
*label* a player has to decode. A serde alias keeps `red`/`blue` payloads
parsing so this is not a breaking wire change.

Player-facing copy, everywhere gear is listed:

- **Insured** — "Comes home with you. Degrades on death."
- **Ephemeral** — "**Vanishes when the run ends** — win or lose. Use it now."

The Equip/Items rows show the word plus a colour cue, and hovering (or
press-and-hold on touch) reveals the sentence. This is the `GR-3` ephemeral-item
class made legible, and the tooltip is the point: a player should never lose an
item they didn't know was temporary.

---

## 3. Gear that makes builds, not bigger numbers

The chase can't be `+1 atk` forever. Past a distance band, drops start rolling
**qualities** instead of only magnitudes:

- **Damage types** (`AD-3`) — gear rolls an element, and monsters carry
  weak/resist/immune profiles. The `gear.damage_modifiers` JSON column already
  exists and is already summed into `GearBonus.modifiers`; this turns it from
  dormant plumbing into a reason to carry a second weapon.
- **On-hit statuses** — burn / chill / stagger / bleed, rolled as an affix, each
  reusing a status the ATB engine already models (Barrier, Regen, Evasion,
  gauge-drain) rather than inventing new state.
- **Keyword affixes** (`AD-1`) — affixes that twist a *class mechanic*: banks
  Adrenaline on a block, a Focus that fires twice, a Transfuse that costs no HP.
  These are what make two same-tier items genuinely different.
- **Synergy affixes** (`AD-2`) — affixes that reference allies ("+X while a
  Resonant is in the party"), which is how an *individual* drop becomes a **party
  build** decision.

**Tier gating.** Bands come from distance, as everything else does
(`tier(d)=floor(d/100)`): stat affixes from tier 0, damage types and on-hit
statuses from a `[TUNABLE]` tier floor, keyword affixes above that, synergy
affixes and signature pieces deepest. So the early game stays legible for a new
player (`P1-3`) and the deep game is where builds bloom.

---

## Why this order

Class-locking first, because it is the cheapest of the three (the `class_key`
hook and the slot categories already exist) and because it *creates the surface*
the other two need: a Shifter off-hand choice and a class-exclusive signature
slot are only meaningful once families exist. Ephemeral clarity next, because it
is small, is a correctness/trust fix, and unblocks `GR-3`. Then the build layer,
which is the big one — and the one that wants the other two underneath it.

## Open questions

- Do signature pieces drop from ordinary loot tables, or only from Gatekeepers /
  authored dungeon bosses (`FS-4`, `DG-3b`)? Boss-only reads better as a chase.
- Should a class be able to *learn* a family it doesn't start with (a `CL-1`
  class-unlock-style progression), or are families permanent identity? This
  proposal assumes permanent.
