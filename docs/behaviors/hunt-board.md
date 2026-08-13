# The Hunt Board

Directed combat goals posted in Last City: named things to go and do that pay out when
you come home. The Hunt Board is the mid-game's answer to "why go out today" — between the
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
what a run costs you is your Backpack, not your standing with a board.

---

## Finding the quarry

A directed goal is only directed if a player can act on it, so a hunt says **where** and
the world **points at it**.

| Rule | Behavior |
|------|----------|
| The board says where | Each row carries a `where_to_look` line derived server-side from the tables the world generates from: which biomes hold a creature (`meld_world::biomes_of_creature`) and the depth `[biome_gate]` opens them at; the depth a Gatekeeper first mounts a seam; the depth Elites start being promoted at. It is never written down a second time, so it cannot disagree with where the creature actually spawns. |
| A Gatekeeper is guaranteed | One stands in the **pass at every biome border**, centred on the guaranteed clear path, on every run including the tutorial (`world-generation.md`; FS-4). A `fell_class: gatekeeper` hunt is therefore never a matter of luck — it is a matter of walking to the next border. |
| The quarry is marked | While a hunt is unfinished and unclaimed, matching creatures are **force-included in that player's own snapshot** and tagged `mob:<kind>:<faction>:quarry`. Force-included the way the portal and a crafter's node-sense are — never by widening the shared interest cull, which would show everyone everything. |
| A Hunter tracks further | Anyone holding a hunt senses its quarry at `[hunt] quarry_sense_radius`; a party with a **Hunter** senses it at `quarry_sense_hunter_radius` — the guild's trade is finding the thing before it finds you. |
| Marking stops when the work does | A hunt at or past its target stops marking, claimed or not: the thing left to do is walk home and be paid. |
| Per-viewer, not per-world | The tag rides each player's own copy of the snapshot row, so the same creature is not a quarry to the teammate beside them. |

---

## Claiming the reward

The reward is **not** granted on completion: it is taken at the board in Last City, so
finishing a hunt is a reason to come home.

| Rule | Behavior |
|------|----------|
| Once per account | A hunt pays exactly once. The claim stamp and the payout are one transaction, so concurrent presses cannot double-pay. |
| Unearned | A claim below the target is refused, and the refusal names the progress and the objective. |
| Payout | `[hunt] reward_chits_base × reward_chits_growth_per_tier ^ tier` chits, plus a stack of the hunt's material (`reward_material_qty + reward_material_qty_per_tier × tier`). Both land in the **Vault**, not the Backpack — you are already home. |
| Gear on the deep hunts | A hunt with `reward_gear` also hands over a **rolled piece**: insured, at the hunt's own tier, from the ordinary affix pool, for a class the claimant actually fields (GR-7's recorded roster) in a slot that class can use. Rolled through the same generator the Forge uses (`meld_world::rolled_gear`) and inserted in the same transaction as the payout. Only tier ≥ 3 hunts pay it, so the board is a ladder; and never from the epic pool, so a champion remains the better *source* of a great item — the board's promise is reliability, not superiority. |
| Advertised by the server | The reward on the row is computed server-side and rides the wire, so a retuned `[hunt]` retunes what the board promises. |

---

## Deliberately not in this cut

The full `AD-4` design ([`../proposals/adventure-depth.md`](../proposals/adventure-depth.md) §E)
also carries: named-creature hunts off `FS-4`'s ten named bosses, rotation and expiry, an
explicit *accept* step, bestiary ties (`CR-5`), co-op and guild hunts (`SOC`), reputation,
and hunt leaderboard points (`AD-6`). None of those exist yet. A hunt today is posted for
everyone, forever, and is claimed by whoever finishes it.
