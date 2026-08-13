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

## Bounties — the Den's own board

A **hunt** is posted for everyone and stands forever. A **bounty** is yours: rolled for
your hunter rank, sighted at a depth that rank has earned, standing in the world for you
alone, and withdrawn after a while whether or not you went.

**Source:** roadmap `AD-4`; registry `meld_proto::bounties`; the roll is
`meld_world::roll_bounty`; magnitudes are `[bounty]` in `balance.toml`.

### Hunter rank

| Rule | Behavior |
|------|----------|
| Its own ladder | Rank rides the **`hunting` Meld skill**, so it uses the same XP ladder every profession does — and it is raised *only* by finished board work, never by levelling a party. "How many marks have you put down" is a different question from "what level is your party", and the generator asks the first one. |
| Where it moves | XP is banked in the same transaction as the payout, at the board. A rank that moved without paying would be a rank nobody earned. |
| What it does | It is the sole input to the roll: sighting depth (`sighting_base_distance + sighting_per_rank × rank`), the mark's power, the chits, the material stack, and whether the contract pays gear at all (`reward_gear_from_rank`). |
| Titles | `rank_title` (Unblooded → Tracker → Marksworn → Houndmaster → Reaver → Apex) gates nothing; it is standing, like the orders' rank ladders. |

### What a contract is

Always a **boss fight**. There is no "fell eight of these" bounty — that is what hunts are
for.

| Rule | Behavior |
|------|----------|
| The mark | One of `FS-4`'s ten named bosses wearing a rolled **epithet** ("Ironmaw the Unburied"), so two contracts on the same boss are two creatures with two histories. |
| How hard | Promoted by the contract's own `power` (`power_base + power_per_rank × rank`) rather than the Gatekeeper constants, and **always** affixed. A deep-rank mark is worse than the door it walked past. |
| Where | Sighted at a distance, in a biome, in the open or **at the bottom of a descent** (`dungeon_chance`). The species it wears is drawn from that band's own pool, so the sprite and the ground agree. |
| A descent contract | The mark **is** what keeps the door: the first dungeon its owner descends at or past the sighted depth builds its boss from the contract instead of the authored one, and felling it finishes the contract. It is therefore never also standing in the open — one contract is one creature. |
| Yours alone | The mark carries an `owner`. It is left out of every other player's snapshot and **cannot be touched by them** — in a co-op instance the party can fight it beside you, but only you can trigger it. |
| Stood up lazily | The world stands a mark up once its frontier reaches the sighted distance, once per contract. A felled mark never comes back. |
| Tracked | A mark is always marked as its owner's quarry, so it wears the same `:quarry` tag and QUARRY plate a hunt's quarry does. |

### The window

| Rule | Behavior |
|------|----------|
| `active_slots` standing | Reading the board withdraws whatever expired and rolls replacements up to the slot count, so the offers are always live with no scheduler anywhere. |
| `window_hours` | Only a **standing** contract expires. A mark already felled is owed its reward however long the walk home takes. |
| Nothing is lost | Progress is felled-or-not, so an expiry can never eat banked work. |

### Claiming

Same rule as a hunt: **paid at the board**. The Quests column shows a finished contract as
ready and says where to take it; the payout (chits + material + a rolled piece from
`reward_gear_from_rank` up) and the rank XP land together, once, in the Vault.

### Where a player reads it

The menu's **Quests** column — which appears only once the account owns the **Hunter**
(`class_hunter`), because the board is the Den's and nothing in that menu advertises what
has not been earned. It lists the standing contracts (mark, where, how hard, what it pays,
how long is left) and everything already settled.

---

## Deliberately not in this cut

The full `AD-4` design ([`../proposals/adventure-depth.md`](../proposals/adventure-depth.md) §E)
also carries: an explicit *accept* step, bestiary ties (`CR-5`), co-op and guild hunts
(`SOC`), reputation, and hunt leaderboard points (`AD-6`). Named-creature contracts,
rotation and expiry ship with bounties above; the fixed hunts are still posted for
everyone, forever.
