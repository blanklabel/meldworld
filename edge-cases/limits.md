# Limits

Consolidated table of every enforced numeric limit, cap, timeout, and size in MELDWORLD. This is a forward-design spec: "Enforcement" describes required observable behavior, and "Source" cites the design document (GDD.md / CANON.md section) or marks the value as **Spec-defined** (a default chosen by this spec file, pending design sign-off). All **[TUNABLE]** values must be server config, never hardcoded (CANON.md preamble).

**Source:** GDD.md §2–§8; CANON.md §B, §D, §G, §I; spec-defined defaults noted per row.

Related specs: [../behaviors/meta-progression.md](../behaviors/meta-progression.md), [../behaviors/economy.md](../behaviors/economy.md), [../behaviors/async-interaction.md](../behaviors/async-interaction.md), [../behaviors/endgame-seasons.md](../behaviors/endgame-seasons.md).

## Limits

| Resource | Limit | Enforcement | Source |
|----------|-------|-------------|--------|
| Party size | 1–4 players per `MazeInstance` | Matchmaking/party formation rejects a 5th member: 400 `validation_error` | GDD.md §5; CANON.md §D13, §G (Party) |
| Battle merge — normal encounters | Max 2 instances (up to 8 combatants) per `Battle` **[TUNABLE]** | Touching a full battle starts a separate battle instead of merging | CANON.md §D5 |
| Battle merge — Gatekeeper | Max 4 instances (up to 16 combatants) **[TUNABLE]** | Same as above; Gatekeeper HP pools sized for 8 at spawn | CANON.md §D5, §B (ATB combat) |
| Backpack capacity | 40 slots **[TUNABLE]** | Pickup/loot into a full Backpack is rejected; item stays on ground / in chest | Spec-defined (this file); Backpack per GDD.md §2.2, CANON.md §G |
| Stall listing slots | `4 + floor(mercantile_level/10) × 2`, range 4–24 **[TUNABLE]** | Listing beyond slot count: 400 `validation_error`. Note: formula max at L99 is 22; 24 clamp unreachable (design inconsistency, see Gotchas) | CANON.md §B (Economy) |
| Stall placement — hub `d ≥ 1000` | Mercantile ≥ 30 **[TUNABLE]** | Deploy rejected: 403 `forbidden` | CANON.md §B (Economy) |
| Stall placement — hub `d ≥ 3000` | Mercantile ≥ 60 **[TUNABLE]** | Deploy rejected: 403 `forbidden` | CANON.md §B (Economy) |
| Stalls per player | 1 deployed stall | Second deploy: 409 `conflict` | Spec-defined ([../behaviors/economy.md](../behaviors/economy.md)) |
| Gem sockets per gear item | 0–3, by gear rarity **[TUNABLE]** (default mapping: common 0, uncommon 1, rare 2, epic+ 3 — rarity bands are content-table) | Socketing into a full item: 409 `conflict` | Spec-defined (this file); gems per GDD.md §4.1, CANON.md §G (Gem) |
| Hub tax rate | `10% − 0.05% × mercantile_level`, floor 5% **[TUNABLE]** | Applied server-side to stall sales & contract payouts; floor never binds at L ≤ 99 (min actual: 5.05%) | GDD.md §4.1, §7; CANON.md §D7, §B (Economy) |
| Meld Skill levels | 1–99 per skill (`forging`, `mercantile`, `alchemy`) | XP past L99 is discarded | GDD.md §4.1; CANON.md §G (MeldSkill) |
| Death durability loss | −10% of current max durability per death, floor 0 **[TUNABLE]** | `max_durability ← floor(max × 0.9)`; gear at 0 is unequippable until repaired | CANON.md §D6, §B (Death & durability) |
| Repair cap (Forging L) | `base_max × (0.5 + L/198)`; L99 → 100% **[TUNABLE]** | Repairs above cap rejected/clamped by crafting UI + server: 400 `validation_error` | CANON.md §B (Death & durability) |
| Contract expiry | 7 days from posting **[TUNABLE]** | Server auto-cancels and refunds full escrow to poster | CANON.md §B (Economy — contract escrow) |
| Contract note length | ≤ 140 characters | 400 `validation_error` | Spec-defined (this file) |
| Stall name length | ≤ 32 characters | 400 `validation_error` | Spec-defined (this file) |
| Player name | 3–20 chars, pattern `^[a-zA-Z0-9_]+$` | 400 `validation_error` at registration/rename | Spec-defined (this file) |
| Build templates per player | 8 **[TUNABLE]** | Save beyond cap: 400 `validation_error` | Spec-defined ([../behaviors/meta-progression.md](../behaviors/meta-progression.md)) |
| Overworld drop despawn | 5 min **[TUNABLE]** | Server deletes the drop entity permanently | Spec-defined ([../behaviors/async-interaction.md](../behaviors/async-interaction.md)); dropping per GDD.md §6 |
| Ward: `warding_tent` | 30 min invisibility to monster pathfinding **[TUNABLE]** | Ward expires server-side | CANON.md §B (Disconnect handling); GDD.md §5 |
| Ward: `sanctuary_campfire` | 10 min invisibility + slow HP regen aura **[TUNABLE]** | Ward expires server-side | CANON.md §B (Disconnect handling); GDD.md §5 |
| Disconnect grace window | 10 s silent reconnection | Disconnect rules (forced flee / auto-defend / sleeping) fire only after grace elapses | CANON.md §B (Disconnect handling) |
| Escape-item extraction channel | 10 s, interruptible **[TUNABLE]** | Taking damage / moving cancels the channel; must restart | CANON.md §D15 |
| ATB turn timeout | 15 s with a full gauge and no action → auto-defend | Server forces defend action | CANON.md §B (ATB combat) |
| ATB server tick | 100 ms | Gauge fill per tick: `speed_stat / 400` (full at 1.0) **[TUNABLE]** | CANON.md §B (ATB combat) |
| Flee success | Base 60%, −10% per tier above party tier, min 5%; Gatekeepers: disabled **[TUNABLE]** | Server-rolled | CANON.md §B (ATB combat) |
| Instance idle close | 60 min with all members disconnected | Instance closes; sleeping avatars auto-abandon (Backpack lost as death, **no** durability loss) | CANON.md §B (Disconnect handling) |
| Chunk size | 64×64 tiles **[TUNABLE]** | World streamed in chunk units | CANON.md §G (Chunk) |
| Interest radius | 2 chunks | Entities outside radius not synced to client | CANON.md §B (Networking targets) |
| Overworld sim tick | 20 Hz (non-binding perf goal) | — | CANON.md §B (Networking targets) |
| Snapshot broadcast | 10 Hz (non-binding perf goal) | — | CANON.md §B (Networking targets) |
| Battle updates | Event-driven + 1 Hz keepalive (non-binding perf goal) | — | CANON.md §B (Networking targets) |
| Season length | 13 weeks exactly, rolling UTC boundary (structural) | Board archived read-only; titles to top 100 instances; infinite-zone board reset; Vault/hubs/Meld Skills NOT wiped | CANON.md §D8, §B (Sessions & seasons) |
| Curated hubs | 11, at `d = 0, 500, …, 5000` (structural) | No hubs beyond 5000; infinite scaling | CANON.md §B (Hubs & run levels) |
| Red-chest gear spawn floor | Cannot spawn below `d = 300` **[TUNABLE]** | Loot tables exclude it | CANON.md §B (Distance → difficulty) |
| Tier-1 material spawn band | Only at `d < 300` **[TUNABLE]** | Loot tables exclude elsewhere (bands mirror biome tiers) | GDD.md §4 (Resource Stratification); band value spec-defined ([../behaviors/meta-progression.md](../behaviors/meta-progression.md)) |
| Chits value range | Non-negative `int64`; no fractional chits | Operations driving a balance negative: 409 `insufficient_funds` | CANON.md §D10 |
| HTTP rate limit | 10 req/s per token **[TUNABLE]** | 429 `rate_limit_exceeded` with `Retry-After` header | Spec-defined (this file); error code per CANON.md §I |
| Realtime rate limit | 30 msgs/s per connection **[TUNABLE]** | Excess messages dropped with a rate-limit notice; repeated abuse → connection closed | Spec-defined (this file) |

## Undocumented Behaviors

- **Run Level has no cap** — it grows without bound during a run via `xp_to_next(L) = 80 × L^1.6` (CANON.md §B). Only the *base* level is hub-derived.
- **Auto-abandon is durability-free**: the 60-min idle close counts as death for the Backpack but explicitly skips the −10% durability loss (CANON.md §B, Disconnect handling) — gentler than an actual death.
- **Forced flee on standard-encounter disconnect always succeeds** (structural), bypassing the normal flee percentages (CANON.md §B).

## Known Gotchas

- **Stall slot cap unreachable:** `4 + floor(99/10) × 2 = 22`, but CANON states "max 24". The 24 clamp only matters if the level cap or formula changes. Implement formula + clamp as written; flagged to design.
- **Tax floor unreachable:** min tax by formula at L99 is 5.05%; the 5% floor never binds at levels 1–99. Implement the clamp anyway.
- **Two "tier" notions coexist:** loot tier `tier(d) = floor(d/100)` (CANON §B) vs. biome bands (Forest 0–100, Desert 100–300, …). Material stratification follows **biome** bands (tier-1 materials to `d < 300`), not `floor(d/100)`.
- **Merged-battle sizing:** Gatekeeper HP pools are sized for 8 combatants at spawn even though a merge can reach 16 (CANON §B, §D5) — a 16-player Gatekeeper fight is intentionally easier per capita.
- **Distance thresholds use `floor`ed Euclidean distance** (CANON §G) — all band checks (`d < 300`, `d ≥ 1000`, gatekeepers at `500k − 1`) compare against the floored integer.
