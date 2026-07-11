# Endgame & Seasons

The permanent competitive endgame: the Vanguard Board (seasonal leaderboard of the deepest distance reached by an instance in a single run), the infinite zone past the final curated hub at `d = 5000` (exponential monster scaling and Prestige aura cosmetics), and the 13-week season lifecycle that archives the board and grants titles without ever wiping persistent player progression.

**Source:** GDD.md §8; CANON.md §B (Distance → difficulty, Sessions & seasons), §D (D3, D8), §G (VanguardBoard, Season, Distance)

Related specs: [meta-progression.md](meta-progression.md) (what persists across seasons), [../edge-cases/limits.md](../edge-cases/limits.md).

---

## Vanguard Board

A global, per-season, real-time leaderboard ranking the **maximum distance achieved by a `MazeInstance` during a single run** in the current season. Entries are per-instance (crediting all party members), not per-player.

**Source:** GDD.md §8 (The Vanguard Board); CANON.md §D (D3), §G (VanguardBoard, Distance)

### Ranking rules

| Rule | Behavior |
|------|----------|
| Metric | Max `distance` (Euclidean from world origin, `floor`ed to integer — CANON §G) reached by **any member** of the instance during one run. Only movement while the run is live counts. |
| Scope | One board per `Season`. Only distances achieved within the season's 13-week window count; a run straddling the season boundary credits each moment's distance to the season it occurred in. |
| Entry identity | The instance's best run this season: `(instance members, max_distance, achieved_at)`. A party's later, shallower runs do not create additional entries; a deeper run replaces its earlier entry. |
| Survival requirement | None — the distance stands whether the run ends in `extracted`, `died`, or `abandoned`. The board rewards depth reached, not survival **[TUNABLE — GDD/CANON do not condition the record on extraction; chosen by this spec]**. |
| Tie-break | Equal `max_distance` ranks by **earliest `achieved_at`** (the server timestamp of the movement update that first reached that distance). First to the frontier wins the tie. |
| Real-time updates | The server updates the board the moment an instance's max distance increases (movement is server-validated, so the authority already sees every step). Leaderboard reads are served over HTTP (persistent state, CANON §S); clients viewing the board receive live re-rank updates. |
| Anti-forgery | Distance comes exclusively from server-side movement validation (CANON §S: server owns movement validation); there is no client-submitted score path. |

---

## The Infinite Zone (d > 5000)

Past the final curated Outer Hub at `d = 5000` there are no more hubs, no Gatekeepers, and no curated biome tables — the server keeps procedurally generating world forever, with exponentially scaling monsters and Prestige cosmetic drops.

**Source:** GDD.md §8 (Infinite Scaling); CANON.md §B (Distance → difficulty — structural exponential branch; Hubs & run levels)

### Monster scaling

| Range | Stat multiplier |
|-------|-----------------|
| `d ≤ 5000` | `stat_mult(d) = (1 + d/500)^1.25` **[TUNABLE]** |
| `d > 5000` | `stat_mult(d) = stat_mult(5000) × 1.5^((d − 5000)/500)` — exponential, **structural** (the exponential form is canon; the 1.5 base is **[TUNABLE]**) |

The two branches meet continuously at `d = 5000` (`stat_mult(5000) = 11^1.25 ≈ 20.0`). Monster level continues as `mlevel(d) = max(1, round(d / 12.5))` **[TUNABLE]** with no cap.

### Prestige aura drops

Monsters in the infinite zone drop "Prestige" cosmetic **aura items** — pure cosmetics (no stats) that prove how deep a player has pushed.

**Source:** GDD.md §8; tier banding consolidated by this spec.

| Rule | Behavior |
|------|----------|
| Drop zone | Infinite zone only (`d > 5000`). |
| Distance-banded tiers | Aura tier is banded by the 500-distance band of the kill: tier `k = floor((d − 5000) / 500) + 1` (tier 1 at 5000–5499, tier 2 at 5500–5999, …). Higher tiers are visually distinct upgrades. **[TUNABLE — band width and visual mapping are content-table]** |
| Extraction | Auras drop into the Backpack like all loot: **they must be extracted to keep**. Deep-zone deaths delete un-banked auras with the Backpack. |
| Persistence | Extracted auras are account cosmetics on the `Player`; NOT wiped at season end. Tradeability: **[TUNABLE — not specified in GDD/CANON; default: tradeable at stalls like other items]**. |

---

## Season Lifecycle

Seasons are 13-week epochs (rolling UTC boundary, structural) that keep the Vanguard Board fresh. A season wipe touches **leaderboards only** — persistent player progression is never reset.

**Source:** GDD.md §8 (Seasonal Wipes); CANON.md §B (Sessions & seasons — structural), §D (D8)

### Flow

1. A `Season` opens with an empty Vanguard Board at the UTC instant the previous season ends (13 weeks exactly, back-to-back; no off-season gap).
2. During the season, the board updates in real time as described above.
3. At season end (the 13-week UTC boundary), the server atomically:
   a. **Archives the Vanguard Board read-only** — the season's final standings are immortalized and permanently queryable, never mutated again.
   b. **Grants cosmetic titles to the top 100 instances** — every `Player` who was a member of a top-100 instance receives the season's unique Hub cosmetic title on their account.
   c. **Resets the infinite-zone leaderboard** — the new season's Vanguard Board starts empty.
4. Runs live across the boundary continue uninterrupted; distances reached after the boundary post to the new season's board.

### What is and is NOT wiped (structural)

| State | At season end |
|-------|---------------|
| Vanguard Board (current season) | Archived read-only; new board starts empty |
| Season titles | Granted to top 100 instances' members; permanent thereafter |
| **Vault** (chits, materials, gear, gems) | **NOT wiped** |
| **Hub unlocks / rebuilt Outer Hubs** | **NOT wiped** |
| **Meld Skills** (forging/mercantile/alchemy levels) | **NOT wiped** |
| Class unlocks, cosmetics (incl. Prestige auras, prior titles) | **NOT wiped** |
| Stalls, open contracts | **NOT wiped** — economy state is season-agnostic ([economy.md](economy.md)) |

### Error cases

| Condition | Error code | HTTP status |
|-----------|------------|-------------|
| Querying a season that does not exist | `not_found` | 404 |
| Any write to an archived season's board | `conflict` | 409 (archives are read-only) |

## Notes

- "Seasonal Wipes" (GDD §8) is a misnomer the implementation must not take literally: only the infinite-zone leaderboard resets. CANON §B marks the not-wiped list structural.
- Title grants go to *instances* (top 100 instances → all their members). A player in two top-100 instances receives the title once (titles are idempotent account cosmetics).
- The tie-break by earliest `achieved_at` is resolved by this spec per the design brief; GDD/CANON specify the metric (D3) but not the tie order. Timestamps are server-side (u64 unix millis on the realtime protocol, CANON §I), so ties at millisecond resolution are effectively impossible; if they occur, rank by instance ID as a final deterministic key.
