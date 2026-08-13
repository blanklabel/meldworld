# Hunt Board Endpoints

> Parent: [interfaces/http-api](../http-api.md)

The Hunt Board is the account's standing list of directed combat goals and what each one
pays (roadmap `AD-4`; behavior: [behaviors/hunt-board.md](../../behaviors/hunt-board.md)).
Progress is written **only** by the game loop off server-owned state — there is no HTTP
write surface for it. The one write here is the claim.

The roster itself is content (`meld_proto::hunts::HUNTS`), not a stored table: a hunt an
account has never touched still appears, at zero. Reward magnitudes are `[hunt]`
`[TUNABLE]`s resolved server-side, so the client never computes a payout.

---

## Shared object: Hunt

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| key | string | No | v0.1 | No | Stable hunt id; what a claim names. |
| name | string | No | v0.1 | No | Display name ("Cull the Bloom"). |
| objective | string | No | v0.1 | No | What the hunt wants, **with its number in it** ("Fell 8 Bloom Stalkers"). Formatted from the goal, so it can never disagree with the rule the server checks. |
| blurb | string | No | v0.1 | No | Flavour. Never carries a magnitude. |
| tier | integer (int32, 0–4) | No | v0.1 | No | Biome band. Orders the board shallow → deep and scales the reward. |
| progress | integer (int32, ≥ 0) | No | v0.1 | No | The caller's progress, capped at `target`. |
| target | integer (int32, ≥ 1) | No | v0.1 | No | Progress the hunt completes at. |
| claimable | boolean | No | v0.1 | No | Earned and not yet paid. |
| claimed | boolean | No | v0.1 | No | Already paid out. A hunt pays once per account. |
| reward_chits | integer (int64, ≥ 1) | No | v0.1 | No | Chits the claim pays (`[hunt] reward_chits_base × reward_chits_growth_per_tier ^ tier`). |
| reward_material | string | No | v0.1 | No | Item kind paid alongside the chits; `""` for chits alone. |
| reward_material_qty | integer (int32, ≥ 0) | No | v0.1 | No | Size of that stack; `0` when there is no material. |
| reward_gear | boolean | No | v0.1 | No | Finishing it also hands over a rolled piece of gear (tier ≥ 3 hunts). |
| where_to_look | string | No | v0.1 | No | Where to go to work it, derived server-side from the world's placement tables. Empty when the objective already says it (a depth). |

---

## GET /v1/hunts

Every posted hunt with the caller's progress against it. Requires a session.

**Response `200`**

```json
{
  "data": [
    {
      "key": "cull_the_bloom",
      "name": "Cull the Bloom",
      "objective": "Fell 8 Bloom Stalkers",
      "blurb": "They have learned to stand where the light comes through…",
      "tier": 0,
      "progress": 3,
      "target": 8,
      "claimable": false,
      "claimed": false,
      "reward_chits": 200,
      "reward_material": "forest_bloom_petal",
      "reward_material_qty": 2,
      "reward_gear": false,
      "where_to_look": "Found in the field or forest, from the first ring out."
    }
  ]
}
```

| Status | When |
|--------|------|
| 200 | Always, for an authenticated caller — an account that has done nothing gets the full roster at zero. |
| 401 | Missing or invalid session token. |

Reading it in-game: the **Bounty Board** district in Last City ([E] at the board).
`MELD_HUNTS` / `?hunts` opens it on arrival for screenshot frames.

---

## POST /v1/hunts/:key/claim

Take the reward for a finished hunt. No request body. Requires a session.

The claim stamp and the payout are one transaction: two concurrent presses cannot both
be paid. The reward lands in the **Vault** (chits and, if the hunt names one, a material
stack) — the caller is in the city, so there is no Backpack to route it through.

**Response `200`**

```json
{
  "key": "unseat_the_keeper",
  "reward_chits": 1166,
  "reward_material": "frost_shard",
  "reward_material_qty": 5,
  "reward_gear": "Unyielding Gauntlet of the Vigil",
  "chits": 2616
}
```

`reward_gear` is the name of the piece the board handed over, or `""` when the hunt pays
no gear. The piece lands in the Vault, unequipped.

`chits` is the Vault balance **after** the payout.

| Status | Code | When |
|--------|------|------|
| 200 | — | Paid. |
| 401 | `unauthorized` | Missing or invalid session token. |
| 404 | `not_found` | No hunt with that key. |
| 409 | `conflict` | Already paid out, **or** not finished — the message names the progress and restates the objective. |
