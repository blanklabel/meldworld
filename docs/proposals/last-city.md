# The Central Hub — **Last City** — design + server plan

> **Name decided: the hub is "Last City."** This supersedes the "The Weld" working
> name used throughout the rest of this doc; wherever it says "The Weld," read
> "Last City." The district names and the whole design below stand. Tracked as
> epic **LC** in [`../ROADMAP.md`](../ROADMAP.md).
>
> Status: **M0 SHIPPED (spike); M1–M3 proposed.** Grounds the persistent social +
> economic core the loop was missing. Written against the real code: the client
> `Screen` state machine ([client `main.rs`](../../client/crates/meld-client/src/main.rs)),
> the single-owner authoritative loop ([`meld-server/game.rs`](../../server/crates/meld-server/src/game.rs)),
> the persistent Vault/economy HTTP surface ([`meld-api`](../../server/crates/meld-api/)),
> and the already-specced economy ([behaviors/economy.md](../behaviors/economy.md)).
> This reframes the GDD "Center Hub" (GDD §2.1, §4, §7) as **the New Weird last
> city** and closes the biggest hole in the extract-or-die loop.
>
> ## M0 implementation status (shipped)
> - **The loop closes.** New `Screen::City` ("The Weld") is the post-auth home and
>   the return target after every run. `Connected → City`; The Threshold dives
>   (`[ENTER]` solo / `[C]` co-op via the lobby); `Ended` demoted to a summary that
>   routes **back to City** (`[ENTER]`) instead of quitting.
> - **Server: `release_from_run`** ([game.rs](../../server/crates/meld-server/src/game.rs)) —
>   extraction + death now clear the session's `in_instance` flag and drop the run,
>   so a second `enter_maze` succeeds. Before this, re-diving was rejected with "A
>   run is already active for you." — the loop could never close.
> - **The Vault-Deep** reads live `GET /v1/vault` (chits / materials / gear), so you
>   watch your haul land after an extract. `[V]` expands the full list.
> - **Tests:** new `qa/tests/redive.rs` (dive → extract → **dive again**) proves the
>   loop closes; full qa suite still green. Screenshot-verified via `?city` / `MELD_CITY`
>   (connect + park in the hub for inspection).
> - **Walkable city (shipped on top of M0).** `Screen::City` is now a walkable HD-2D
>   plaza, not a menu: you steer a hero avatar (WASD, camera-relative) around The Weld
>   and interact with the district you're standing in (`[E]`). Buildings/props are
>   Kenney CC0 kits — **Fantasy Town** (stalls, fountain, the Threshold arch, lanterns),
>   **Graveyard** (crypts as district buildings, gravestones, fire-baskets, dweller
>   NPCs), **Pirate** (the beached wreck + dock the city is salvaged from). Reuses the
>   overworld HD-2D machinery (camera, `CharSprite` walk animation, GLB scene loading);
>   the city is client-local (no server sim — matches the presence-only design).
>   Screenshot-verified via `?city` / `MELD_CITY`. See `assets/ATTRIBUTIONS.md`.
> - **Deferred to M1+:** functional Market/Forge/Bounty/Drill/Vanguard interiors (the
>   buildings stand but only Threshold + Vault act), the town presence loop + Commons
>   crowd (real other players), floating world-space building labels (the contextual
>   HUD prompt names the district you're standing in for now), and a distinct plaza
>   floor (the hub currently sits on the grass commons).

## The problem: the loop has no home

The core loop is **extract-or-die**, but today it just *ends*. The client flow is
`Join → Lobby → Overworld → Battle → Ended`, and [`ended_ui`](../../client/crates/meld-client/src/main.rs)
is a terminal screen — "EXTRACTED — banked 3 items + 120 chits to your Vault …
Press ESC to exit." You bank loot into a persistent Vault (real rows in Postgres),
earn chits, level Meld skills — and then there is **nowhere to be**. No place to
spend chits, restock, craft, sell your harvest, read a bounty, or see another
soul. The persistent half of the game (GDD §2.1) is specced but roomless.

Every downstream system already assumes this room exists:

- [economy.md](../behaviors/economy.md) — Stalls "deployed **in a Hub**," Bounty
  Contracts "posted on **a Hub's** Bounty Board," Mercantile taxes, the durability
  sink that "keeps crafters employed."
- GDD §4 — the Training Ground, Outer Hub unlocks, resource stratification.
- CANON D16 — "hub UIs (Vault, Training Ground, Stall shop, Bounty Board,
  leaderboards)" — explicitly Bevy UI, HD-2D art.
- `game.rs` already stamps every run with `departure_hub_distance = 0 // Center Hub`.

So this is not a new pillar — it is **the missing floor** under pillars the spec
already stands up. This doc designs it and, crucially, answers the one question
the GDD hand-waves: how does a locked, 4-player instanced game host *a city*.

---

## The fiction: **The Weld**

The world is an infinite radial plane that grows outward from a single origin
(distance 0). Difficulty, monster level, and loot scale purely with distance —
which means **the origin is the one still, safe point in a hostile universe that
gets worse in every direction forever.** That geography *is* the story: the last
city is not a capital you sortie from, it is the **last calm** — the eye, the
drain, the seam the whole weird world flows back into.

**The Weld** (working name) is that seam. The name is the game's own — *MELD*world;
everything you extract *melds* back here, and the city is welded together from the
salvage of a thousand dead runs. It is New Weird in the Miéville/VanderMeer register:
bio-industrial and grown-not-built, uncanny but *warm* — the one place the wrongness
of the plane is held at bay, lit by lantern-fungus and forge-glow, loud with a
hundred strangers haggling over monster parts.

> **Aesthetic north star (for HD-2D art):** pixel sprites in a 3D-lit scene
> (CANON D16). Vertical, stacked, accreted — catwalks and stilts over an "origin
> wound" you never quite see the bottom of. Chitin-and-brass architecture: shells,
> carapaces, and salvaged maze-metal fused into buildings. Colour: warm interior
> pools (amber forge, teal vault-light, spore-green commons) against the cold
> indifferent dark of the plane at the city's edge. Sound: market murmur, forge
> ring, a low tidal hum from the wound. Not grim — *thronged and alive.*
>
> **Name is a one-line veto.** Alternates if "The Weld" doesn't land: **The
> Confluence** (everything flows in), **Nadir** (the low still point), **The
> Cinder** (last warm ember), **Fathom** (the drain). Pick one; the rest of this
> doc is name-agnostic.

Fictionally this is the **Center Hub** of GDD §4; "Outer Hubs" (unlocked at deep
Gatekeepers) are later, smaller Welds — this doc designs the first and central one.

---

## Where it sits in the loop

The city replaces the terminal `Ended` screen as the loop's hub-and-spoke center.
You are *always* in the city between dives; a dive is a spoke you leave and return
to.

```
                         ┌─────────────────────────────┐
                         │        THE WELD (city)       │
   register / login ───► │  persistent · safe · social  │ ◄─── extract (bank → Vault)
                         │  Vault · Market · Forge ·     │ ◄─── die  (blue gear returns,
                         │  Bounties · Training · Board  │        backpack lost)
                         └──────────────┬──────────────┘
                                        │  step through The Threshold
                                        ▼   (form/join party — the current Lobby)
                                 ┌──────────────┐
                                 │  THE MAZE    │  ephemeral · 4-player instance
                                 │  overworld + │  (unchanged: single-owner loop,
                                 │  ATB battle  │   no locks — CANON §S)
                                 └──────────────┘
```

Concretely, in the client `Screen` enum ([main.rs:378](../../client/crates/meld-client/src/main.rs)):

- **`Join`** stays (auth).
- **New `Screen::City`** becomes the post-auth default and the return target.
  `Lobby` folds into a *building inside the city* (The Threshold), not a separate
  full-screen state.
- **`Ended` is demoted** to a brief result *toast/summary* that then routes **back
  to `City`** ("banked 3 items + 120 chits") instead of `AppExit`. Death routes to
  the same place (your blue gear is waiting in the Vault, per GDD §2.2).

Nothing about the maze changes. The city is a **new, parallel state**, not a
modification of the authoritative combat loop.

---

## The districts (each = one existing system, given a room)

The city is a small HD-2D walkable scene (like an area, but combat-free and
persistent) with labelled buildings. Walk to a building, press the interact key,
open its Bevy UI. Every district maps 1:1 to a system that already exists or is
already specced — the city is the **front door** to them, not new mechanics.

| District (fiction) | System it fronts | Backing spec / code | Status today |
|---|---|---|---|
| **The Vault-Deep** — teal-lit strongroom grown into the wound wall | Vault: chits, materials, blue/red gear, gems, equip/unequip, repair | [economy.md], data-models Vault/GearItem; `GET /v1/vault`, `/vault/gear` exist | API partly built, **no UI** |
| **The Market Tiers** — stacked stalls, owners' avatars frozen mid-haggle | Player **Stalls** (offline shops) | [economy.md] "Stall Lifecycle" (deploy → sell → close, atomic purchase, tax sink) | **specced, unbuilt** |
| **The Forge & the Alembic** — forge-glow + spore-green alchemy lab | **Forging** / **Alchemy** Meld crafting (gear from materials, gems, durability repair) | GDD §4.1; [meta-progression.md]; forging/alchemy XP already persists | XP tracked, **no crafting UI** |
| **The Bounty Board** — a bristling notice-wall of escrowed orders | **Contracts** (gathering bounties, chit rewards, expiry) | [economy.md] "Bounty Contracts" | **specced, unbuilt** |
| **The Drill Yard** — training ground under the city lanterns | **Training Ground**: build templates, Base Level allocation | GDD §4 | **unbuilt** |
| **The Vanguard Wall** — names carved by the wound's light | Leaderboards / Vanguard Board (seasonal) | [endgame-seasons.md]; CANON §seasons | **unbuilt** |
| **The Threshold** — the gate onto the plane; a party rallying-point | Party formation + **the current Lobby** (create/join by code, ready, start) | `lobby.*` messages already exist ([net.rs](../../client/crates/meld-client/src/net.rs)) | **built** (as a screen) |
| **The Commons** — the plaza: fungus-lantern square where everyone lands | **Social**: presence of 100s, proximity chat, emotes, hanging out | *new* (see scale section) | **new** |

The Commons is the only genuinely new thing. Everything else is a room wrapped
around a system the spec hierarchy already commits to — which is exactly why the
city is high-leverage: **it makes the extract-or-die loop mean something and
unblocks the entire economy at once.**

---

## The hard part: how a 4-player game hosts a city of hundreds

This is the design's crux and where the GDD hand-waves. MELDWORLD's whole
performance story (memory: game-loop-perf; CANON §S) is that **exactly one Tokio
task owns all ephemeral state, so there are no locks.** A maze `ActiveInstance`
holds ≤4 players. A naïve "shared city instance with 300 players" would either
break that model (locks, contention) or melt the single task. It must not.

**Design decision: the city is presence-only, and it is a *different* loop.**

Three moves keep the no-locks maze model untouched while letting the city feel
crowded:

1. **All state that matters is already persistent → it already goes through HTTP,
   not the game loop.** Per CANON §S and [economy.md], *every* economic mutation
   (buy a listing, deploy a stall, post/fulfil a contract, craft, repair, equip,
   allocate a build) is a **persistent** change and therefore executes
   **server-side, atomically, over the HTTP API** — never through the realtime
   authoritative loop. This is already the rule the economy spec is written to.
   So the city needs **no authoritative game-loop state at all** for its actual
   functionality. Stalls sell while you're offline *because they're rows*, not
   because a task is simulating them.

2. **The realtime layer in the city carries only presence + chat + emotes** —
   soft, lossy, non-authoritative signal. Nobody's HP is on the line; a dropped
   position packet in the Commons costs nothing. This is a separate, simpler loop
   from the maze's 100 ms authoritative tick — call it the **town loop** — and it
   can broadcast presence at a lazy cadence with the same
   `broadcast()`/`Arc<RawValue>` serialize-once discipline already used for maze
   snapshots (memory: game-loop-perf).

3. **Shard the Commons into "wards" with a soft cap.** A single city has N visual
   shards (wards); each new arrival joins the least-full ward under a soft cap
   (e.g. 40–60). You *see* your ward's ~50 avatars and chat with them; the
   Market/Vault/etc. are shared and global (they're just HTTP). "Hundreds of
   players" is true at the city level and at the market level; the *rendered
   crowd* around you is one ward. This is the standard MMO town-shard pattern and
   it keeps per-broadcast fan-out bounded.

> **Net:** the maze loop (`game.rs`) is **not touched**. The city is a new,
> lighter presence service + the existing HTTP economy surface + a new Bevy scene.
> The one architecturally-honest new thing is the town presence loop, and it is
> strictly easier than the combat loop it sits beside.

**Trade / chat safety.** All trades are **escrowed and atomic** (already specced:
stall escrow, contract escrow) — there is no free-form player-to-player trade
window to exploit, and the async gifting channel ([async-interaction.md]) stays as
the only direct hand-off. Chat is proximity/ward-scoped text with server-side rate
limiting and a report path; it never carries authority.

---

## Wire + HTTP surface (additive — respects the concurrency rules)

Per [AGENTS.md] "coordinate on global files — prefer additive edits," every change
below is **new** enum variants / messages / endpoints, never a rename.

**HTTP (persistent — the real economy; mostly already specced in economy.md):**
`POST /v1/stalls` (deploy), `GET /v1/hubs/:d/stalls`, `POST /v1/stalls/:id/buy`,
`DELETE /v1/stalls/:id`; `GET/POST /v1/hubs/:d/contracts`,
`POST /v1/contracts/:id/fulfill`; `POST /v1/craft/forge`, `POST /v1/craft/alchemy`,
`POST /v1/gear/:id/repair`; `GET/PUT /v1/build-templates`. (Note the axum-0.7
`:param` form — memory: axum-route-params; `{param}` silently 404s.)

**Realtime (town loop only — presence/chat, non-authoritative):** new
`town.*` messages paralleling the existing `lobby.*`: `town.enter { hub_distance }`
→ `town.ward_state { ward_id, members: [(player_id, name, class, pos)] }`,
`town.move { pos }` → broadcast `town.presence`, `town.chat { text }` →
`town.message`, `town.emote { kind }`. All ride the existing envelope
`{type, seq, ts, payload}` (CANON §I).

**Client:** new `Screen::City` + district building entities + one Bevy UI panel
per district (reusing the battle/menu UI patterns already in `main.rs`).

---

## Build plan (phased; M0 is spike-sized)

Ordered so the loop *closes* first (highest value), then the economy fills in.

- **M0 — Close the loop (the floor).** Add `Screen::City`: a static HD-2D plaza
  scene with labelled building props. Route `Ended` → summary toast → `City`
  instead of exit; make `City` the post-auth default; enter the maze via **The
  Threshold** building (wraps the existing Lobby). **The Vault-Deep** UI reads the
  live `GET /v1/vault` you already have. *Outcome: you dive, extract, walk back
  into a city, see your banked loot, and dive again — the loop is whole.* No new
  server loop yet; single-player-visible city.

- **M1 — The Market + The Commons (social).** Stand up the **town presence loop**
  (ward sharding, `town.*` messages), render other players' avatars in the
  Commons, proximity chat + emotes. Build **Stalls** end-to-end per [economy.md]
  (deploy from Vault, atomic buy, tax sink). *Outcome: sell your harvest; see the
  crowd.*

- **M2 — Crafting + Bounties.** The Forge/Alembic UIs (Forging/Alchemy consume
  materials → gear/gems/repair) and the Bounty Board (post/fulfil escrowed
  contracts). *Outcome: chits and materials have sinks; crafters are employed.*

- **M3 — Training + Vanguard + polish.** Drill Yard build templates, the Vanguard
  Wall leaderboard, emote/cosmetic pass, ambient HD-2D life (NPC vendors, spore
  lanterns, the wound). Fold into CANON with a §/D-number and a
  `behaviors/central-hub.md`, mirroring how verticality will graduate.

- **Deferred (explicitly):** Outer Hubs (deep-distance unlocks, GDD §4); cross-hub
  stall placement gates (Mercantile ≥ 30/60, already in economy.md); guild/party
  persistence; voice; anything seasonal beyond a read-only board.

---

## Open decisions (yours to call)

1. **Name** — "The Weld," or one of the alternates above.
2. **Crowd model** — ward soft-cap size (rendered crowd vs. broadcast cost); is a
   global shared Market list enough, or do stalls also need to be *walkable* in a
   ward (heavier)? Recommendation: **global list first**, walkable Market later.
3. **M0 scope** — do you want M0 to ship *only* the loop-closing city + Vault
   (fastest path to "the loop feels whole"), or M0 + Market in one push?
   Recommendation: **loop first**, then Market — the loop-closing is the single
   highest-value change and is small.
