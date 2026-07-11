# Economy

The player-driven economy: player Stalls (persistent offline shops), the Bounty Board (escrowed gathering Contracts), the durability sink that keeps crafters employed, and the chits-conservation invariants that make the whole system auditable. All economy mutations are **persistent** and therefore flow through the HTTP API, executed server-side and atomically (CANON §S boundary rule, §D14). Chits is a 64-bit integer; there is no fractional chits — all tax computations round as specified below.

**Source:** GDD.md §7; CANON.md §B (Economy, Death & durability), §D (D6, D7, D10, D14), §G (Stall, Contract, Vault), §I (error codes)

Related specs: [meta-progression.md](meta-progression.md) (Mercantile/Forging levels that gate everything here), [async-interaction.md](async-interaction.md) (free-form item drops, the non-market gifting channel), [../edge-cases/limits.md](../edge-cases/limits.md).

---

## Stall Lifecycle

A `Stall` is a player shop deployed in a Hub. The owner's avatar turns into a shop sprite at the deploy location; other players tap it to open the shop UI and buy listed items. The stall remains active and sells items **while the owner is offline**.

**Source:** GDD.md §7 (Player Stalls); CANON.md §B (Economy — stall slots, placement gates, tax), §D (D14), §G (Stall)

### Flow (deploy → sell → close)

1. **Deploy.** In a Hub the player is standing in, the player deploys a stall with a name (≤ 32 chars, see [../edge-cases/limits.md](../edge-cases/limits.md)) and lists items from their Vault at integer chits prices. Listed items move into **stall escrow**: they leave the Vault and cannot be equipped, traded, dropped, or listed elsewhere while listed.
2. The owner's avatar becomes a shop sprite at the deploy position. The owner cannot move or enter the Maze while the stall is deployed from their live avatar; on logout the sprite **persists** and the stall keeps operating.
3. **Purchase.** A buyer taps the sprite, opens the shop UI, and buys a listing. The purchase executes server-side as one atomic transaction (see Purchase Flow below).
4. **Close.** The owner (online, at the stall or remotely via the stall management UI) closes the stall. All unsold listings return from escrow to the owner's Vault; accumulated proceeds were already deposited at each sale. The avatar reverts from shop sprite (or, if offline, the sprite despawns).

### Rules

| Rule | Behavior |
|------|----------|
| Placement | Any Hub the owner has `active` (Center always; Outer Hubs per [meta-progression.md](meta-progression.md)). Hubs `d ≥ 1000` require Mercantile ≥ 30; `d ≥ 3000` require Mercantile ≥ 60 → 403 `forbidden` otherwise. **[TUNABLE]** |
| Slot count | `4 + floor(mercantile_level / 10) × 2` listings max, clamped to 24. Exceeding → 400 `validation_error`. **[TUNABLE]** |
| One stall per player | A player may have at most one deployed stall at a time → 409 `conflict` on second deploy. |
| Pricing | Price per listing: integer chits ≥ 1 → 400 `validation_error` otherwise. |
| Offline persistence | Stall survives owner logout indefinitely; it does not expire. It is removed only by owner close (or account-level moderation action). |

### Purchase flow (atomic)

All steps succeed or none do; the server serializes concurrent purchases of the same listing.

1. Buyer submits purchase of listing L at price P.
2. Server verifies: listing exists and is unsold; buyer has ≥ P chits in Vault; buyer is not the owner.
3. Server debits P from buyer's Vault.
4. Server computes tax `T = round_up(P × tax_rate(owner))` where `tax_rate(owner) = max(5%, 10% − owner.mercantile_level × 0.05%)` **[TUNABLE]**. Tax is paid by the **seller**: owner receives `P − T`; `T` is destroyed (the tax sink).
5. Server credits `P − T` to owner's Vault (works while owner is offline), transfers the item from stall escrow to buyer's Vault, marks the listing sold.
6. Sale grants the owner Mercantile XP ("selling items at stalls", GDD §4.1).

### Error cases

| Condition | Error code | HTTP status |
|-----------|------------|-------------|
| Listing already sold (lost the race) | `conflict` | 409 |
| Buyer chits < price | `insufficient_funds` | 409 |
| Buyer is the stall owner | `validation_error` | 400 |
| Listing/stall not found (closed between open and buy) | `not_found` | 404 |
| Deploy in a hub without required Mercantile level | `forbidden` | 403 |
| Listing count exceeds slot count | `validation_error` | 400 |

---

## Bounty Contracts

A `Contract` is a gathering order posted on a Hub's Bounty Board: item, quantity, chits reward, expiry (e.g. "Need 50 Iron Ore for 500c"). It lets casual players monetize short sessions and lets offline crafters source materials.

**Source:** GDD.md §7 (Bounty Board); CANON.md §B (Economy — contract escrow, tax), §D (D14), §G (Contract)

### Flow (post → accept → fulfill)

1. **Post.** The poster specifies item type, quantity, reward chits R, and an optional note (≤ 140 chars). The server debits R from the poster's Vault into **contract escrow** at posting time (409 `insufficient_funds` if short). Expiry is fixed at **7 days** from posting **[TUNABLE]**.
2. **Accept.** Any player may accept an open contract, marking it `accepted` by them. Acceptance is exclusive **[TUNABLE — exclusivity vs. open-fulfillment is a design default chosen here]** and non-binding: the acceptor may abandon, returning the contract to `open`. A contract still un-fulfilled at expiry refunds regardless of acceptance.
3. **Fulfill.** The acceptor delivers the items at a Bounty Board. The server **verifies the items**: exact item type and full quantity must be present in the deliverer's Vault (extracted goods only — Backpack items must be banked first). Partial fulfillment is rejected.
4. On successful verification, atomically: items transfer to the poster's Vault; tax `T = round_up(R × tax_rate(poster))` is destroyed (tax paid by the **poster**, same rate formula as stalls, floor 5%); the fulfiller receives `R − T` from escrow; contract → `fulfilled`. Fulfiller gains Mercantile XP ("completing player contracts", GDD §4.1).
5. **Expiry.** If not fulfilled within 7 days, the server auto-cancels: full escrow R (untaxed — tax applies only to payouts) refunds to the poster's Vault; contract → `expired`.

### States / Transitions

| From state | Event | To state | Side effects |
|------------|-------|----------|--------------|
| — | Post (escrow R debited) | `open` | 7-day expiry timer starts |
| `open` | Accept | `accepted` | Exclusive claim by acceptor |
| `accepted` | Abandon / acceptor inactivity | `open` | Claim released |
| `open` / `accepted` | Fulfill with verified items | `fulfilled` | Items → poster; `R − T` → fulfiller; `T` destroyed |
| `open` / `accepted` | Poster cancels before any fulfillment | `cancelled` | Full R refund to poster |
| `open` / `accepted` | 7 days elapse | `expired` | Full R refund to poster (automatic, server-side) |

`fulfilled`, `cancelled`, `expired` are terminal.

### Error cases

| Condition | Error code | HTTP status |
|-----------|------------|-------------|
| Post with insufficient chits for escrow | `insufficient_funds` | 409 |
| Accept a non-`open` contract | `conflict` | 409 |
| Fulfill with wrong item type or short quantity | `validation_error` | 400 |
| Fulfill a terminal contract | `conflict` | 409 |
| Poster fulfills their own contract | `validation_error` | 400 |

---

## Durability Sink Loop

The structural loop that keeps low-level and high-level economies intertwined: dying degrades permanent gear; only Forging crafters can restore it; crafters need low-tier materials that only spawn near the Center ([meta-progression.md](meta-progression.md), Resource Stratification).

**Source:** GDD.md §2.1, §7 (The Durability Sink); CANON.md §B (Death & durability), §D (D6)

### Flow

1. A player dies in the Maze. Blue-chest gear (`insurance: blue`) is returned safely to the Hub, but each piece loses max durability: `max_durability ← floor(max_durability × 0.9)` (−10% of **current** max per death), floor 0. **[TUNABLE]**
2. Gear at 0 max durability is **unequippable** until repaired.
3. A Forging-L crafter repairs the gear, restoring max durability up to the cap `base_max × (0.5 + L/198)` (L99 → 100% of original `base_max`). Repair prices are player-set (stall/contract/direct market); repair material costs are content-table **[TUNABLE]**.
4. Repairing grants Forging XP; sourcing repair materials drives contracts and stalls near the Center Hub.

Because the loss is multiplicative on current max and repair caps below 100% for L < 99, gear repaired by low-level crafters ratchets downward over many deaths — this is intentional demand for high-level Forging.

---

## Chits Conservation Invariants

Chits is a conserved 64-bit integer quantity. Every chits mutation is classified as a **source** (creates chits), a **sink** (destroys chits), or a **transfer** (conserves chits). The tables below are exhaustive: any operation not listed must not change total chits in circulation.

**Source:** GDD.md §2.1, §7; CANON.md §B (Economy), §D (D7, D10, D14); classification consolidated by this spec.

### Sources (chits created)

| # | Source | Where |
|---|--------|-------|
| S1 | Monster kill chits drops | Battle loot rolls (server-authoritative), banked on extraction |
| S2 | World loot drops (chests, containers) | Overworld loot rolls, banked on extraction |

Chits found during a run lives in the Backpack: it is only **minted into the persistent economy at extraction**; dying deletes it with the Backpack (so a death of un-extracted chits is a non-event for the persistent supply — it never entered circulation).

### Sinks (chits destroyed)

| # | Sink | Amount |
|---|------|--------|
| K1 | Hub tax on stall sales | `round_up(price × tax_rate(seller))`, rate `max(5%, 10% − 0.05% × mercantile_level)` |
| K2 | Hub tax on contract payouts | `round_up(reward × tax_rate(poster))`, same rate formula |

The tax is the **only** chits sink. NPCs do not sell gear or services for chits (GDD §7: "NPCs do not sell the best gear. The community runs the world."); any future NPC chits cost is a new sink and must be added to this table.

### Transfers (chits conserved)

| # | Transfer | From → To |
|---|----------|-----------|
| T1 | Stall purchase | Buyer Vault → (seller Vault `P − T`) + (sink K1 `T`) |
| T2 | Contract escrow at posting | Poster Vault → contract escrow |
| T3 | Contract payout | Escrow → (fulfiller Vault `R − T`) + (sink K2 `T`) |
| T4 | Contract refund (expiry/cancel) | Escrow → poster Vault, in full |
| T5 | Camp rebuild cost, if chits-denominated | Player Vault → destroyed — **note:** this would be a third sink; flagged as a design decision needed before implementation (see [meta-progression.md](meta-progression.md) Notes) |

### Invariants

- **I1:** `Σ(vault chits) + Σ(contract escrow) + Σ(backpack chits in live runs)` changes only by S1+S2 (up) and K1+K2 (down).
- **I2:** Every purchase/payout is atomic; no observable state exists where chits left one party without arriving (net of tax) at the other.
- **I3:** Escrowed chits (stall items are item-escrow; contract rewards are chits-escrow) is owned by no player and is unspendable until release.
- **I4:** Tax is computed with `round_up` so `T ≥ 1` on any taxed transaction of `P ≥ 1` — the sink can never round to zero **[TUNABLE — rounding direction chosen by this spec; CANON specifies only the rate]**.
- **I5:** Chits amounts are non-negative `int64` everywhere; any operation that would drive a balance negative fails with 409 `insufficient_funds` and mutates nothing.

## Notes

- Player-to-player free giving exists outside the market via overworld item drops ([async-interaction.md](async-interaction.md)) — items only, never chits, so drops cannot bypass the tax sink.
- Contract fulfillment verifies items from the **Vault**, not the Backpack; a gatherer must survive extraction before a contract can pay out. This is deliberate: contracts inherit the extract-or-die tension.
