# Co-op Groups & Guilds — design + server plan

> **Status: PROPOSED (design only — nothing built).** This is the design doc the
> ROADMAP epic **SOC** (`SOC-1` co-op groups, `SOC-2` guilds) and **MON-2** point
> at but defer. It graduates the two unchecked roadmap items into a full spec:
> data models, HTTP + realtime surface, balance tunables, a phased build plan, and
> the CANON deltas to fold in when the design hardens. Written against the real
> stack: the single-owner authoritative loop and the ephemeral **Lobby** /
> **LobbyMember** ([`meld-server/game.rs`](../../server/crates/meld-server/src/game.rs)),
> the persistent Vault/economy HTTP surface ([`meld-api`](../../server/crates/meld-api/)),
> the already-specced economy ([`behaviors/economy.md`](../behaviors/economy.md)),
> and the **Last City** hub + town presence loop ([`proposals/last-city.md`](last-city.md)).
> Tracked as epic **SOC** in [`../ROADMAP.md`](../ROADMAP.md).

> **Terminology (matches the ROADMAP SOC note).** In this codebase **"party"**
> already means one player's team of up to four *heroes* (mixed classes) inside a
> single `MazeInstance` (CANON §G, D5). The systems here are about grouping
> *players*, so — to avoid overloading "party" — this doc uses:
>
> - **Co-op group** (`CoopGroup`) — a **transient** team of players who dive
>   together. The durable evolution of today's ephemeral **Lobby**: it survives
>   between dives instead of dissolving at each maze exit. (`SOC-1`.)
> - **Guild** (`Guild`) — a **persistent** player organization with membership,
>   ranks, a shared vault, identity (name/tag/flag), chat, and a home in the Last
>   City. Account-level and long-lived, like the Vault. (`SOC-2`.)
>
> A co-op group and a guild are independent: a group can be all-guildmates,
> cross-guild, or guildless; a guild member dives in whatever group they like.

---

## The problem: the loop is social-shaped but solitary

The extract-or-die loop already *assumes* other people — the shared instance
(CANON D5, D13), the raid-merge (D5), the player-run economy ([`economy.md`](../behaviors/economy.md)),
the Commons crowd and proximity chat ([`last-city.md`](last-city.md) M1). But two
load-bearing pieces of "playing with people" don't exist:

1. **Grouping is throwaway.** You form up through the ephemeral **Lobby** (join
   code → `run.join_battle` → the Threshold). It dissolves the moment the run ends.
   There's no "my usual crew" that persists from dive to dive, no way to see where
   your friends are, no durable roster.
2. **There's no organization to belong to.** No guilds — no membership, no shared
   stash, no identity you carry into the world, no reason to log in for *people*
   rather than loot. Every downstream retention system (a guild line on the Vanguard
   board, the premium guild instance MON-2, guild bounties) is written against a
   guild that isn't specced.

This doc designs both, and answers the concurrency question the same way
[`last-city.md`](last-city.md) does: **everything durable is persistent state and
therefore goes through the HTTP API, atomically — never through the no-locks maze
loop.**

---

## Design principles (inherited, non-negotiable)

These fall straight out of CANON §S and the Last City design; every rule below is a
consequence, not a new invention.

1. **Persistent state → HTTP, atomic, server-authoritative.** Guild membership,
   ranks, the guild vault, the audit log, heraldry, guild XP — all **persistent
   rows**. Every mutation executes server-side over the HTTP API as one atomic
   transaction (CANON §S, §D14; the rule [`economy.md`](../behaviors/economy.md) is
   already written to). The authoritative maze loop ([`game.rs`](../../server/crates/meld-server/src/game.rs))
   is **not touched.**
2. **Realtime layer carries only soft signal** — presence, chat, emotes, flag
   display. Non-authoritative, lossy-tolerant; a dropped guild-chat packet costs
   nothing. This rides the **town loop** ([`last-city.md`](last-city.md) §"the hard
   part"), the same lighter presence service the Commons uses, with the
   `broadcast()` / `Arc<RawValue>` serialize-once discipline (memory: game-loop-perf).
3. **No free-form anything that can't be moderated.** No free-text trade windows
   (trades stay escrowed/atomic), no arbitrary-image flag uploads (heraldry is
   composed from a bounded server catalog). Chat is rate-limited with a report path
   and never carries authority.
4. **Additive wire/HTTP only.** Per [`AGENTS.md`](../../AGENTS.md), every change
   here is a **new** enum variant / message / endpoint — never a rename. New
   `guild.*` and `group.*` realtime messages parallel the existing `lobby.*` /
   `town.*`; new `/v1/guilds/*` endpoints sit beside the economy surface. (axum-0.7
   `:param` form — memory: axum-route-params; `{param}` silently 404s.)

---

## Part A — Co-op Groups (`SOC-1`)

### What it is

A **`CoopGroup`** is a named, leader-owned team of players that **outlives a single
dive**. It is the ephemeral Lobby made durable: form it once, and it survives
`city → maze → extract → city → maze` until it's disbanded or everyone logs off.
It is **not** persisted to Postgres — it lives in server memory on the **town loop**
alongside presence (like `Lobby`/`LobbyMember` today), because nothing about it must
survive a full logout. What it adds over the Lobby is **lifetime + presence + a
chat channel**.

### Forming and managing a group

You can build a group from **either** the Last City **or** the overworld — grouping
is a town-loop concern, available everywhere the town/presence layer runs.

| Action | How | Realtime message |
|---|---|---|
| **Create** | Any player creates a group (becomes leader) | `group.create` → `group.state` |
| **Invite** | Leader/any member (setting-gated) invites an **online** player by name | `group.invite { player }` → target gets `group.invite_received` |
| **Accept / decline** | Invitee responds | `group.accept` / `group.decline` → `group.state` |
| **Leave** | Any member leaves | `group.leave` → `group.state` |
| **Kick** | Leader removes a member | `group.kick { player }` → `group.state` |
| **Transfer lead** | Leader hands off (or auto on leader-leave: oldest member) | `group.transfer { player }` |
| **Disband** | Leader disbands, or last member leaves | `group.disband` |

`group.state { group_id, leader, members: [(player_id, name, class, guild_tag,
flag_id, presence)] }` is broadcast to all members on every change. `presence` is
one of `city`, `overworld { distance }`, `in_battle`, `offline`, so the group panel
shows where everyone is at a glance.

### Presence — see your crew

- **In the Last City:** group members show on the city minimap (UX-1) with a group
  marker and render with a group-tint nameplate in the Commons.
- **In the overworld:** group members in the *same instance* already appear in the
  snapshot; group members **not** in your instance surface as a lightweight roster
  entry (`presence.overworld { distance }`) so you know they're out on a dive. No
  cross-instance rendering — just roster state.

### Diving together — how a group maps onto the instance cap

This is the question `SOC-1` flags (CANON D5: an instance is ≤ 4 players; a merged
battle holds 2 instances / 8 combatants normally, 4 / 16 for Gatekeepers). Resolved
in two phases:

- **Phase 1 — group ≤ 4 = one instance.** The leader presses **dive**; the group
  fills exactly one `MazeInstance` (the current Lobby-start path, upgraded to read
  the durable group instead of an ad-hoc lobby). This is the whole of `SOC-1`'s
  first ship and it maps cleanly onto the existing cap.
- **Phase 2 — raid groups (5–16) via merge, deferred.** A group larger than 4
  forms **linked instances** on the **same `run_seed`** (world-gen is deterministic
  from the seed — `section_seed(run_seed, n)`, [`world-generation.md`](../behaviors/world-generation.md)),
  so the crew walks the *same* world in ≤ 4-player pods, and battles **auto-merge**
  by the existing raid-merge (D5: 2 instances for standard, 4 for Gatekeepers). This
  reuses the merge that already exists rather than lifting the 4-player instance cap.
  Deferred until the base group ships.

> **Group cap** = `coop_group_max_size` **[TUNABLE]**, default **4** in Phase 1,
> raised to `raid_group_max_size` (default **8**, Gatekeeper raids **16**) when
> Phase 2 lands.

### Group chat

A private channel for the group: `group.chat { text }` → `group.message { from,
name, guild_tag, flag_id, text, ts }`, delivered only to current members over the
town loop. Ephemeral (no backlog persistence — it's a transient team). Sender's
guild tag + flag id ride on every message so crests render inline (see Part D).

---

## Part B — Guilds (`SOC-2`)

A **`Guild`** is a persistent, account-level player organization. Everything in this
part is **persistent HTTP state.**

### B.1 Registering a guild — The Charterhouse

Guilds are chartered at a new Last City district, **The Charterhouse** (the guild
registry — a brass-and-vellum office grown into the wound wall, ledgers of every
charter ever bound). It slots into the district table in [`last-city.md`](last-city.md)
exactly like the others: walk to the building, press interact, open its Bevy UI.

**Founding flow:**

1. A player without a guild opens The Charterhouse and submits: **name**, **tag**,
   and an initial **flag** (heraldry — Part D).
2. Server validates (below), debits the **founding cost**
   `guild_founding_cost_chits` **[TUNABLE]** (default **5,000 c**) from the
   founder's Vault — an anti-spam sink — and creates the guild with the founder as
   **Leader** (rank 0).
3. `POST /v1/guilds` returns the new guild; the founder is now its sole member.

**Validation & uniqueness:**

| Field | Rule |
|---|---|
| `name` | 3–24 chars, `^[A-Za-z0-9 ]+$`, **globally unique** (case-insensitive), profanity-filtered → 409 `conflict` / 400 `validation_error` |
| `tag` | 2–5 chars, `^[A-Za-z0-9]+$`, **globally unique** (case-insensitive) — the short badge shown on nameplates → 409 `conflict` |
| `flag` | A valid `Heraldry` composed from the server catalog (Part D) → 400 `validation_error` |
| Founder eligibility | Must not already be in a guild (one guild per player) → 409 `conflict`; must have ≥ founding cost → 409 `insufficient_funds` |

Founding cost and both uniqueness checks are enforced **atomically** with creation
(the name/tag reservation and the chit debit succeed or fail together).

### B.2 Membership — invites, applications, roster

**One guild per player** (a hard invariant, like one deployed stall). Two join
paths, both server-atomic:

- **Invite** (guild → player): a member with `invite` permission invites an online
  or offline player → `GuildInvite` row (`pending`). The target accepts/declines
  from their guild UI (or a login notification). Accept adds them at the default
  **Recruit** rank.
- **Application** (player → guild): a guild may be `open` (auto-join),
  `application` (creates a `GuildApplication` an officer approves), or `invite_only`
  (`join_policy` setting). Applications carry an optional ≤ 140-char note.

**Roster** (`GET /v1/guilds/:id` for members): every member with rank, `joined_at`,
`contribution_xp`, and **live presence** (`city` / `overworld { distance }` /
`in_battle` / `offline` + `last_seen`) merged in from the town loop. The Charterhouse
UI shows online-first, sortable by rank / contribution / last-seen.

**Leaving & removal:**

| Action | Who | Endpoint |
|---|---|---|
| Leave | any member (not the Leader unless last) | `DELETE /v1/guilds/:id/members/:player` (self) |
| Kick | member with `kick`, only lower rank | `DELETE /v1/guilds/:id/members/:player` |
| Promote / demote | member with `manage_ranks`, only below own rank | `PATCH /v1/guilds/:id/members/:player { rank }` |
| Transfer leadership | Leader only | `POST /v1/guilds/:id/transfer { player }` |
| Disband | Leader only, **guild vault must be empty** | `DELETE /v1/guilds/:id` |

**Succession (dead-guild safety).** A guild whose Leader is offline > `guild_leader_
succession_days` **[TUNABLE]** (default **30 d**) can be claimed by the highest-ranked
active Officer via `POST /v1/guilds/:id/transfer` (self-claim, allowed only past the
threshold). Prevents guilds stranded by an inactive founder — a standard modern
expectation.

### B.3 Ranks & permissions

Each guild has an ordered list of **`GuildRank`s** (rank 0 = Leader, immutable and
all-powerful). Default ladder, editable by `manage_ranks`:

| Rank | Default permissions |
|---|---|
| **Leader** (0) | everything; sole holder; only rank that can disband / transfer |
| **Officer** (1) | invite, kick (lower), promote/demote (below self), edit_flag, edit_motd, manage_bounties, vault_deposit, vault_withdraw (high limit) |
| **Member** (2) | invite (if guild setting allows), vault_deposit, vault_withdraw (modest limit) |
| **Recruit** (3) | vault_deposit only; **no withdraw** (default) |

Permissions are a **bitset** (`GuildPermission`): `invite`, `kick`, `manage_ranks`,
`edit_flag`, `edit_motd`, `edit_settings`, `vault_deposit`, `vault_withdraw`,
`manage_bounties`, `manage_hall`, `disband`. Every permission check is enforced
**server-side** on the HTTP handler — the client greys disallowed actions, but the
server is the authority. Withdraw is further constrained by per-rank daily limits
(B.5).

Rank config: `GET/PUT /v1/guilds/:id/ranks`.

### B.4 Identity — name, tag, flag, MOTD

- **MOTD** (message of the day): ≤ 280 chars, set by `edit_motd`, shown on login and
  in The Charterhouse. `PATCH /v1/guilds/:id { motd }`.
- **Name / tag:** set at founding; renaming costs `guild_rename_cost_chits`
  **[TUNABLE]** and re-checks uniqueness (anti-squat churn control).
- **Flag:** see Part D.
- **Settings:** `join_policy`, member-invite toggle, default recruit rank, etc.
  (`edit_settings`).

### B.5 The Guild Vault — shared storage with theft-proof accounting

The headline of "so they can easily share." A **`GuildVault`** is a guild-owned
bucket structurally identical to a player `Vault` (CANON §G) — **chits, materials,
and gear** — but owned by the guild and accessed by any member with the right
permission. Same persistence class, same atomicity rules as the player Vault.

**Deposit / withdraw** are transfers between a member's personal Vault and the Guild
Vault:

- `POST /v1/guilds/:id/vault/deposit { chits?, items? }` — moves chits/items from
  the caller's Vault into the Guild Vault. Any member with `vault_deposit`.
- `POST /v1/guilds/:id/vault/withdraw { chits?, items? }` — the reverse, gated by
  `vault_withdraw` **and** the caller's **per-rank daily limit**.

**Per-rank daily withdraw limits** are the anti-drain control (a rogue member can't
empty the stash):

| Limit (per `GuildRank`) | Meaning |
|---|---|
| `withdraw_limit_chits_per_day` | max chits withdrawn per rolling UTC day **[TUNABLE]** |
| `withdraw_limit_items_per_day` | max item count withdrawn per rolling UTC day **[TUNABLE]** |

Usage accrues on `GuildMember` (`daily_withdraw_used_chits` / `_items`), resets at
the UTC day boundary, and a withdraw that would exceed the limit fails **atomically**
with 403 `forbidden` (nothing moves). Leader (rank 0) is unlimited.

**No tax.** Guild-vault deposit/withdraw is an ownership transfer between two buckets
the same player-base owns, **not** a market transaction — so, unlike stalls and
contracts, it carries **no hub tax** (there'd be no seller/buyer to pay it). This
extends the chits-conservation invariant (below).

**Chits conservation delta** (extends [`economy.md`](../behaviors/economy.md) §I1):
the conserved total becomes

```
Σ(vault chits) + Σ(guild vault chits) + Σ(contract escrow) + Σ(backpack chits in live runs)
```

— guild-vault deposit/withdraw is a **transfer** (new rows T6/T7 in the ledger
tables), never a source or sink. Add them to the economy transfer table when this
folds in.

### B.6 The audit log — who did what with the vault

Every guild-vault mutation writes an **immutable, append-only `GuildVaultLog`
entry** — the accountability spine that makes a shared vault safe to trust. It
parallels the economy `LedgerEntry` (CANON §G).

| Field | Meaning |
|---|---|
| `id` | UUIDv7 |
| `guild_id` | the guild |
| `actor_player_id` + `actor_name` | **who** (name snapshotted so it survives a rename/leave) |
| `action` | `deposit` \| `withdraw` |
| `chits_delta` | signed chits moved (0 if item-only) |
| `item_ref` + `quantity` | which material/gear and how many (null for chits-only) |
| `chits_balance_after` | guild-vault chit balance after the entry (running audit) |
| `ts` | server timestamp (ISO 8601) |

`GET /v1/guilds/:id/vault/log` returns the log **newest-first, paginated**,
filterable by `actor`, `action`, and time range — rendered as a scrollable ledger in
The Charterhouse (any member can read; it's the trust mechanism). Entries are
**never mutated or deleted** (moderation actions append a compensating entry rather
than editing history). Future guild-vault actions (craft-from-vault, guild-bounty
payouts) append their own action kinds to the same log.

### B.7 Guild progression — XP, level, perks

Guilds level up from member activity — the "why keep the guild alive" hook every
modern MMO ships.

- **Guild XP** accrues from member milestones (server-credited, atomic): successful
  **extractions**, **new distance records**, **bounty completions**, boss kills.
  Each also credits the member's `contribution_xp` (a roster leaderboard).
- **Guild level** on the doubling curve family already used for runs
  (`guild_xp_to_next(L) = guild_xp_base × guild_xp_growth^(L-1)` **[TUNABLE]**,
  capped at `guild_max_level`).
- **Level unlocks** (all **[TUNABLE]**):

| Unlock | Scales with level |
|---|---|
| **Member cap** | `guild_base_member_cap + guild_member_cap_per_level × L` (default base 20, +5/level) |
| **Guild vault slots** | `guild_vault_base_slots + guild_vault_slots_per_level × L` |
| **Heraldry catalog** | higher tiers unlock more charges / patterns / a second charge |
| **Guild Hall** (later) | a claimable hall plot in the city at level *N* (ties MON-2) |

### B.8 Guild bounties (later — folds into the economy)

An **internal** bounty board: a guild posts a gathering order visible only to
members, **escrowed from the Guild Vault** instead of a personal Vault, so officers
can direct crafting effort ("bring 200 Iron Ore"). Mechanically a `Contract`
([`economy.md`](../behaviors/economy.md)) scoped to `guild_id` and paid from the
Guild Vault; payout appends a `GuildVaultLog` entry. **Deferred** — listed so the
data model reserves room for it. `GET/POST /v1/guilds/:id/bounties`, `POST
/v1/guilds/:id/bounties/:bid/fulfill`.

### B.9 Guild line on the Vanguard Board (later)

The seasonal Vanguard Board ([`endgame-seasons.md`](../behaviors/endgame-seasons.md))
gains a **guild aggregate** line — e.g. the guild's best member distance, or summed
top-N — so guilds compete seasonally, not just individuals. Read-only; deferred with
the rest of the seasonal work.

---

## Part C — Chat

Three scopes, one envelope, one delivery discipline. All chat rides the **town loop**
(soft signal, CANON §I envelope `{type, seq, ts, payload}`), is **server-side
rate-limited** (`chat_rate_limit_msgs_per_10s` **[TUNABLE]**), profanity-filterable,
and has a **report path** (`chat.report { message_id }`). Chat **never carries
authority.**

| Channel | Message → broadcast | Scope | History |
|---|---|---|---|
| **City / ward** | `town.chat` → `town.message` (from [`last-city.md`](last-city.md)) | your Commons ward (~50 players) | none (ambient) |
| **Group** | `group.chat` → `group.message` | current co-op group | none (transient) |
| **Guild** | `guild.chat` → `guild.message` | **all online guild members**, keyed by `guild_id` (not ward-scoped) | **persisted backlog** |

**Guild chat is the durable one.** Delivery is realtime to online members, but a
**rolling backlog** of the last `guild_chat_history` **[TUNABLE]** messages (default
**200**) is persisted so a member sees recent conversation on login:
`GET /v1/guilds/:id/chat?before=<ts>` returns paginated history. Old messages age out
past the cap. (This is the one chat channel modern players expect to have memory —
ward and group chat stay ephemeral.)

**Every chat message carries the sender's identity token** — `from` (player id),
`name`, `guild_tag`, `flag_id` — so the client renders the sender's **guild crest +
tag inline** next to their name in *any* channel (city, group, or guild). That's how
"display those guild flags … in guild chats and Last City chats" is satisfied: the
flag id is a field on the message, and the client draws the small heraldry badge from
its cached catalog render (Part D).

---

## Part D — Guild flags (heraldry): creation & display

### D.1 Composed, not uploaded — the `Heraldry` model

A guild flag is **not** an uploaded image (unmoderatable, a hate-symbol vector).
It's a **`Heraldry`** value composed from a **bounded server catalog**, exactly like
the emblem editors in Guild Wars 2 / Chivalry / For Honor — expressive but
enumerable, so every possible flag is renderable client-side and reviewable
server-side.

| Field | Meaning | Source |
|---|---|---|
| `field_pattern` | background division (solid, per-pale, per-fess, quarterly, chevron, saltire, bordure…) | catalog enum |
| `field_color_a`, `field_color_b` | the two field tinctures | catalog palette |
| `charge` | the central emblem (beast, blade, eye, spore, anchor, wound-sigil…) | catalog enum |
| `charge_color` | charge tincture | catalog palette |
| `border` | optional edge treatment | catalog enum |

`GET /v1/heraldry/catalog` returns the allowed patterns / charges / palette (and
which entries the guild's level has unlocked, B.7). The client's **flag editor**
(in The Charterhouse) composes a `Heraldry` from the catalog and previews it live;
`PUT /v1/guilds/:id/flag { heraldry }` validates every field against the catalog
(400 `validation_error` on any out-of-catalog value) and stores it. Because a flag
is a handful of small enums, it's tiny on the wire and **deterministically rendered**
by any client — the server ships ids, the client draws pixels.

**Moderation.** Bounded catalog → no arbitrary imagery; combinations flagged by
report go to account-level moderation, which can force-reset a guild's flag (append
a `GuildVaultLog`-style audit note). Founding cost + rename cost throttle churn.

### D.2 Where flags display

The flag is identity you carry everywhere. It rides existing presence/chat surfaces
as **additive fields** — no new "show a flag" message is needed:

- **In the overworld & Last City (over avatars):** the avatar presence snapshot
  gains additive `guild_tag: Option<String>` + `flag_id: Option<Uuid>` (rides
  `AvatarState` / `SnapshotEntity`, in the spirit of the `key:value` status-token
  convention — CLAUDE.md "extending combatant state without a proto change"). The
  client draws a small heraldry banner above the avatar and the `[TAG]` on the
  nameplate. `flag_id` lets the client fetch-and-cache each guild's `Heraldry` once
  (`GET /v1/guilds/:id/flag`) and reuse the render.
- **Over a guild's Hall** (later, B.7/MON-2): a guild that owns a city hall plot
  flies its flag over it — a persistent, walk-past piece of identity in the Commons.
- **In chat (all channels):** the `flag_id` + `guild_tag` on every `*.message`
  render the crest inline beside the name (Part C).
- **In rosters & the Vanguard Board:** the guild profile and any guild line show the
  full flag.

> **Wire discipline:** `guild_tag` + `flag_id` are **two small optional fields**
> added to presence snapshots and chat messages — additive, cheap, and the flag
> itself is fetched once per guild and cached, never streamed per-frame.

---

## Data models (additive — CANON §W-style block)

Persistent (Postgres, mutated only via HTTP), unless marked ephemeral.

| Model | Summary |
|---|---|
| `Guild` | id, `name` (unique), `tag` (unique), `heraldry` (embedded flag), `motd`, `level`, `xp`, `join_policy`, `leader_id`, `founded_at`, settings |
| `GuildMember` | guild_id, player_id, rank, `joined_at`, `contribution_xp`, `daily_withdraw_used_chits`/`_items` (+ reset day) — **one row per player, unique on player_id** (one guild per player) |
| `GuildRank` | guild_id, rank (order), `name`, `permissions` (bitset), `withdraw_limit_chits_per_day`, `withdraw_limit_items_per_day` |
| `GuildVault` | guild_id → chits + materials + gear (guild-owned bucket; mirrors player `Vault`) |
| `GuildVaultLog` | **immutable, append-only** audit entry (B.6): actor, action, chits/item delta, balance-after, ts |
| `GuildInvite` | guild_id, player_id, inviter_id, status (`pending`/`accepted`/`declined`), ts |
| `GuildApplication` | guild_id, player_id, note, status, ts |
| `Heraldry` | embedded: `field_pattern`, `field_color_a/b`, `charge`, `charge_color`, `border` (catalog enums/ids) |
| `GuildBounty` *(later)* | a `Contract` scoped to guild_id, escrowed from `GuildVault` |
| `CoopGroup` *(ephemeral — town loop memory)* | id, leader, members[], created_at, `shared_run_seed?` (set on dive); like `Lobby`/`LobbyMember`, but survives across runs |

Detail files would live under [`../interfaces/data-models/`](../interfaces/data-models/)
(e.g. `guild-models.md`) when this graduates, mirroring `economy-models.md`.

---

## HTTP surface (persistent — all guild state)

All under `/v1/`, axum-0.7 `:param` form (memory: axum-route-params). Permission-gated
server-side per B.3; every mutation atomic.

```
POST   /v1/guilds                         found a guild (name, tag, flag; debits cost)
GET    /v1/guilds/:id                      profile (roster + presence if a member)
GET    /v1/guilds/mine                     the caller's guild (or 404)
PATCH  /v1/guilds/:id                       motd / settings / rename (perm-gated)
DELETE /v1/guilds/:id                       disband (Leader; vault must be empty)

POST   /v1/guilds/:id/invites              invite a player            (invite)
POST   /v1/guilds/:id/applications         apply to join
POST   /v1/guild-invites/:iid/accept       accept an invite
POST   /v1/guild-invites/:iid/decline      decline
POST   /v1/guilds/:id/applications/:aid/approve   approve an application (invite/kick)
DELETE /v1/guilds/:id/members/:player       leave (self) or kick        (kick)
PATCH  /v1/guilds/:id/members/:player       set rank                    (manage_ranks)
POST   /v1/guilds/:id/transfer             transfer / succession-claim leadership

GET/PUT /v1/guilds/:id/ranks               rank ladder + permissions   (manage_ranks)

GET    /v1/guilds/:id/vault                 guild vault contents        (member)
POST   /v1/guilds/:id/vault/deposit         deposit chits/items         (vault_deposit)
POST   /v1/guilds/:id/vault/withdraw        withdraw (rank daily limit) (vault_withdraw)
GET    /v1/guilds/:id/vault/log             audit log (paginated/filter)(member)

GET    /v1/heraldry/catalog                 allowed patterns/charges/palette
GET    /v1/guilds/:id/flag                  a guild's Heraldry (cache key = flag_id)
PUT    /v1/guilds/:id/flag                  set the flag                (edit_flag)

GET    /v1/guilds/:id/chat?before=<ts>      guild chat backlog (paginated)
GET/POST /v1/guilds/:id/bounties            guild bounties  (later)
POST   /v1/guilds/:id/bounties/:bid/fulfill                (later)
```

## Realtime surface (town loop — soft signal, additive)

New `group.*` and `guild.*` messages parallel the existing `lobby.*` / `town.*`; all
ride the `{type, seq, ts, payload}` envelope (CANON §I).

**Co-op group (transient):**
`group.create`, `group.invite { player }` → `group.invite_received`,
`group.accept` / `group.decline`, `group.leave`, `group.kick { player }`,
`group.transfer { player }`, `group.disband`, `group.dive` → forms the instance;
server broadcast `group.state { group_id, leader, members:[…] }`;
`group.chat { text }` → `group.message`.

**Guild (presence + chat only — everything durable is HTTP):**
`guild.chat { text }` → `guild.message { from, name, guild_tag, flag_id, text, ts }`
(delivered to all online guild members by `guild_id`); `guild.presence` pings
(online/offline + activity) feeding the roster; `chat.report { message_id }`.

**Flag display:** no new message — additive `guild_tag` + `flag_id` on the existing
avatar presence snapshot and on every `*.message`.

## Balance tunables (new `[guild]` / `[social]` blocks in `balance.toml`)

Every number here is **[TUNABLE]** — added to `balance.toml` behind the `meld-balance`
loader, never hardcoded (working agreement #2).

| Constant | Default | Purpose |
|---|---|---|
| `guild_founding_cost_chits` | 5,000 | anti-spam sink at founding |
| `guild_rename_cost_chits` | 2,500 | rename churn control |
| `guild_name_min/max_len` | 3 / 24 | name bounds |
| `guild_tag_min/max_len` | 2 / 5 | tag bounds |
| `guild_base_member_cap` | 20 | members at level 1 |
| `guild_member_cap_per_level` | 5 | member growth |
| `guild_vault_base_slots` / `_per_level` | 50 / 10 | vault capacity growth |
| `guild_xp_base` / `guild_xp_growth` / `guild_max_level` | 1000 / 1.5 / 30 | guild curve |
| `guild_leader_succession_days` | 30 | dead-leader succession threshold |
| `guild_chat_history` | 200 | persisted guild-chat backlog size |
| rank `withdraw_limit_chits_per_day` / `_items_per_day` | per-rank | vault theft controls |
| `coop_group_max_size` | 4 | group cap (Phase 1) |
| `raid_group_max_size` | 8 (16 Gatekeeper) | group cap (Phase 2) |
| `chat_rate_limit_msgs_per_10s` | 10 | anti-flood, all channels |

---

## Build plan (phased; each ships something usable)

Ordered so the cheap, high-value social glue lands first and the heavier persistence
(guild vault, progression) follows.

- **M0 — Co-op groups (`SOC-1` Phase 1).** Upgrade the ephemeral Lobby into a
  durable `CoopGroup` on the town loop: create / invite / accept / leave / kick /
  transfer, `group.state` presence, group chat, and **dive together** into one ≤4
  instance. *Outcome: form your crew once, keep diving together, see where they are.*
  No Postgres — pure town-loop memory, maze loop untouched.

- **M1 — Guild core (`SOC-2`).** Postgres models (`Guild`, `GuildMember`,
  `GuildRank`, `GuildInvite`/`Application`); **The Charterhouse** district + UI
  (found, invite, roster, ranks/permissions, MOTD); the guild identity (name/tag).
  *Outcome: guilds exist — you can found one, run a roster, and belong.*

- **M2 — Guild Vault + audit log.** `GuildVault`, deposit/withdraw with per-rank
  daily limits, the immutable `GuildVaultLog`, the ledger UI. Extend the chits
  conservation invariant. *Outcome: guilds share loot safely, with a full paper
  trail.*

- **M3 — Heraldry + chat + display.** `GET /v1/heraldry/catalog`, the flag editor,
  `PUT …/flag`; guild chat with persisted backlog; the additive `guild_tag`/`flag_id`
  on presence + chat, and the client render (banners over avatars, crests inline in
  chat, tags on nameplates). *Outcome: guilds look like guilds — everywhere.*

- **M4 — Progression + polish.** Guild XP/level/perks (member cap, vault slots,
  catalog tiers), contribution leaderboard, succession, moderation/report tooling.

- **Deferred (own scope):** raid groups (`SOC-1` Phase 2, 5–16 via merge); guild
  bounties (B.8); the Vanguard guild line (B.9); guild Halls + the premium guild
  instance (**MON-2** — needs its own doc, per ROADMAP).

When each milestone hardens, **fold it into CANON** with §/D-numbers and graduate the
relevant parts into `behaviors/guilds.md` + `interfaces/` (mirroring how verticality
and dungeons graduated — [`README.md`](../README.md)).

---

## CANON deltas to fold in (when the design hardens)

Proposed additions (numbering illustrative — assign at fold-in time):

- **New D-number — Co-op groups vs. party vs. instance.** A `CoopGroup` is a durable
  team on the town loop; it fills one instance (≤4) or, in Phase 2, linked instances
  on a shared seed that raid-merge (refines D5/D13). Distinct from `Party` (the ≤4
  *players* in an instance) and from a player's hero *party* (≤4 heroes).
- **New D-number — Guild.** Persistent, account-level org: one guild per player,
  unique name + tag, ranks with a permission bitset, member cap by guild level.
- **New D-number — Guild Vault & audit.** Guild-owned bucket; deposit/withdraw are
  **tax-free transfers** with per-rank daily withdraw limits; every mutation writes
  an immutable `GuildVaultLog` entry; guild-vault chits join the conservation total
  (extends [`economy.md`](../behaviors/economy.md) §I1, new transfers T6/T7).
- **New D-number — Heraldry.** Flags are composed from a bounded server catalog
  (never uploaded); rendered deterministically client-side from small enum ids.
- **Glossary (§G) additions:** `CoopGroup`, `Guild`, `GuildMember`, `GuildRank`,
  `GuildVault`, `GuildVaultLog`, `Heraldry`, guild `tag`.

---

## Open decisions (yours to call)

1. **District name** — "The Charterhouse," or an alternate in the Last City register
   (The Bindery, The Concord, The Compact, The Sigil-Hall).
2. **One guild per player** — hard cap (assumed here, matches genre norms) vs.
   allowing multiple / alt-guild membership. Recommendation: **one**, simplest and
   standard.
3. **Guild-vault tax** — this doc says **no tax** (internal transfer, not a market
   trade). Confirm — the alternative (a small deposit tax as a chits sink) would need
   a new K-row in the economy sink table and changes the conservation story.
4. **Group cap in Phase 1** — 4 (one clean instance) vs. jumping straight to raid
   groups. Recommendation: **4 first** (M0), raid groups deferred (Phase 2).
5. **Disband policy** — require an empty vault (assumed) vs. auto-distribute to the
   Leader. Recommendation: **require empty** — forces an explicit, audited drain.
6. **Guild chat history size / retention** — 200 messages (default) vs. a time
   window; and whether ward/group chat also get any backlog (assumed **no**).
