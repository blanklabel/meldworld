# MELDWORLD docs

All design, spec, planning, proposal, and status docs for MELDWORLD. (The only
markdown that stays at the repo root is [`AGENTS.md`](../AGENTS.md)/`CLAUDE.md` —
the agent convention-of-record — and the repo [`README.md`](../README.md).)

Read [`AGENTS.md`](../AGENTS.md) first if you're an agent; it's the map. This index
is the human table of contents.

## Start here

| Doc | What it is |
|---|---|
| [`ROADMAP.md`](ROADMAP.md) | **The live worklist** — what we're building next, as checkable items with stable IDs. Tick boxes here when you land work. |
| [`GDD.md`](GDD.md) | Game Design Document — the vision. Source of truth for *intent*. |
| [`CANON.md`](CANON.md) | Authoritative resolutions of GDD gaps: names, enums, formulas, `[TUNABLE]`s. **Wins over the GDD on conflict.** |
| [`BUILD-PLAN.md`](BUILD-PLAN.md) | Milestones (M0–M…) and team/task decomposition (T1–T6). |
| [`spec-index.md`](spec-index.md) | Index of the behavior + interface specs below. |

## Spec — how it must behave

- [`behaviors/`](behaviors/) — observable behavior: [world generation](behaviors/world-generation.md),
  [run lifecycle](behaviors/run-lifecycle.md), [combat/ATB](behaviors/combat-atb.md),
  [economy](behaviors/economy.md), [meta-progression](behaviors/meta-progression.md),
  [disconnect handling](behaviors/disconnect-handling.md),
  [async interaction](behaviors/async-interaction.md),
  [endgame & seasons](behaviors/endgame-seasons.md).
- [`interfaces/`](interfaces/) — wire/data contracts:
  [HTTP API](interfaces/http-api.md), [realtime protocol](interfaces/realtime-protocol.md),
  [data models](interfaces/data-models.md).
- [`edge-cases/`](edge-cases/limits.md) — the consolidated table of every numeric
  limit, cap, and timeout.

## Proposals — designs not yet folded into CANON

- [`proposals/last-city.md`](proposals/last-city.md) — **Last City**, the persistent
  social/economic hub (M0 shipped; M1–M3 = roadmap epic **LC**).
- [`proposals/verticality.md`](proposals/verticality.md) — terraces + ladders/ropes/slopes
  (implemented as a spike; awaiting a CANON §/D-number).

## What's built vs. next

There's no static status snapshot — they rot. Trust the **code** for what's live
and [`ROADMAP.md`](ROADMAP.md) for what's next. (The GDD also predates shipped
features like Last City and verticality; where it disagrees with the code, the
code wins.)
