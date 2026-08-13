# The Hunt Board

Directed combat goals posted in Last City: named things to go and do that pay out when
you come home. The Hunt Board is the mid-game's answer to "why dive today" — between the
Vanguard Board's single axis (go deeper) and the end-world bosses (`EW`, unbuilt), it is
the only spine that names a *specific* objective.

**Source:** [`../proposals/adventure-depth.md`](../proposals/adventure-depth.md) §E
(AD-4); roadmap [`../ROADMAP.md`](../ROADMAP.md) Phase 1 ③. Chits minted here are
economy source **S4** ([economy.md](economy.md)).

Related specs: [economy.md](economy.md) (the payout as a faucet),
[endgame-seasons.md](endgame-seasons.md) (the Vanguard Board it sits beside),
[../interfaces/http-api/hunts.md](../interfaces/http-api/hunts.md) (the wire surface).

> **A hunt is combat-facing.** The economy's *gathering* **Contracts** (`EC-1`,
> [economy.md](economy.md) "Bounty Contracts") are player-posted orders for materials.
> Both are read at the same district in Last City; they are different systems and do not
> share state.

---

## What is posted

The board is a **fixed roster of hunts** defined in one registry
(`meld_proto::hunts::HUNTS`), read by both sides: the server credits progress against it
and the client draws its rows from it, so the board cannot advertise a condition the
server does not check.

Each hunt declares a **goal**, a **tier** (biome band 0–4, shallow → deep) and the
material its reward is paid in. It is **not** seasonal and does not expire.

| Goal | Complete when |
|------|---------------|
| `fell` | `count` creatures of one `monster_kind` have been felled |
| `fell_class` | `count` creatures of one **encounter class** (`elite`, `gatekeeper`) have been felled |
| `depth` | the player has stood at `distance` or deeper |
| `extract_from` | a run ended in a successful extraction having reached `distance` or deeper |
| `clear_dungeon` | `count` dungeon bosses have been felled |

Progress is a single integer per (account, hunt), capped at the goal's target.

---

## Crediting progress

| Rule | Behavior |
|------|----------|
| Authority | Every credit is read off **server-owned state** — the felled creature's own kind and encounter class, the server-validated avatar position, the run's own distance record. There is no client-submitted progress path (CANON §S). |
| Kill credit | On a won battle, **every participating player** is credited for **every creature in the encounter** — a co-op joiner earns the hunt too, on the same terms as the XP split. |
| Depth credit | Posted off the same new-deepest-tile high-water mark as the Vanguard Board, so it is asked once per new record rather than on every step. |
| Extraction credit | Credited when the run banks, against the run's deepest distance. A death credits nothing: the hall pays on evidence. |
| Dungeon credit | Credited to every member of a won dungeon **boss** battle. |
| Announcement | The player is told as it happens (`run.hunt_progress`), and once more when the credit **completes** the hunt. Completion fires exactly once, however many kills land in the same tick. |
| A claimed hunt is inert | Progress on a hunt that has already paid out is not recorded. |

Progress persists per account and survives death, disconnection and the end of a run —
what a dive costs you is your Backpack, not your standing with a board.

---

## Claiming the reward

The reward is **not** granted on completion: it is taken at the board in Last City, so
finishing a hunt is a reason to come home.

| Rule | Behavior |
|------|----------|
| Once per account | A hunt pays exactly once. The claim stamp and the payout are one transaction, so concurrent presses cannot double-pay. |
| Unearned | A claim below the target is refused, and the refusal names the progress and the objective. |
| Payout | `[hunt] reward_chits_base × reward_chits_growth_per_tier ^ tier` chits, plus a stack of the hunt's material (`reward_material_qty + reward_material_qty_per_tier × tier`). Both land in the **Vault**, not the Backpack — you are already home. |
| Advertised by the server | The reward on the row is computed server-side and rides the wire, so a retuned `[hunt]` retunes what the board promises. |

---

## Deliberately not in this cut

The full `AD-4` design ([`../proposals/adventure-depth.md`](../proposals/adventure-depth.md) §E)
also carries: named-creature hunts off `FS-4`'s bosses, rotation and expiry, an explicit
*accept* step, bestiary ties (`CR-5`), co-op and guild hunts (`SOC`), reputation, and
hunt leaderboard points (`AD-6`). None of those exist yet. A hunt today is posted for
everyone, forever, and is claimed by whoever finishes it.
