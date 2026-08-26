#!/usr/bin/env python3
"""Generate the bestiary's 8-direction sprite sets from `creature_sprites.json`.

WHY A DRIVER AND NOT A SESSION OF TOOL CALLS. An 8-direction animation is 8 PixelLab
jobs and the account's cap is 10 concurrent, so animations SERIALIZE: one clip at a
time, a few minutes each. Two clips for each of 34 characters is hours of wall clock,
which no interactive session should be holding open. So this walks the list itself,
records every step in a state file, and is safe to kill and re-run — it picks up at the
first unfinished step rather than regenerating what it already paid for.

    PIXELLAB_TOKEN=... python3 client/scripts/gen_creature_sprites.py [--only KEY] [--dry-run]

Per character: create (v3, 8 directions, size 96) -> walk clip -> attack clip ->
download -> install through `install_class_sprite.sh`, which pads every frame out to the
184px canvas the renderer expects (see docs/asset-pipeline.md - a sprite that fills its
own frame renders at twice the size of everything beside it).
"""
import argparse, json, os, pathlib, subprocess, sys, time, urllib.error, urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "client/scripts/creature_sprites.json"
STATE = ROOT / "client/scripts/.creature_sprites_state.json"
ASSETS = ROOT / "client/crates/meld-client/assets/creatures"
ENDPOINT = "https://api.pixellab.ai/mcp"

ALL_DIRS = ["south", "south-east", "east", "north-east", "north", "north-west", "west",
            "south-west"]

# WALK TURNS, ATTACK DOES NOT, and the split is about where each clip is SEEN.
#
# Walking is the overworld: a creature crosses the view in every direction, all the time,
# and a body that slides sideways while facing you is the thing that reads as broken. So
# the walk is drawn eight times, which costs eight jobs against a ten-job concurrency cap
# and is most of this script's runtime.
#
# An attack is only ever seen in a BATTLE, where the arena faces the party — so seven of
# its eight directions would be art for a camera angle that never happens.
# `hd2d::load_creature_clips` reuses the south attack for every facing.
# THE WESTERN HALF IS THE EASTERN HALF FLIPPED, so it is not drawn. The eight facings
# are symmetric about the north-south axis: `south` and `north` sit ON that axis, and the
# rest are three mirrored pairs. Five generated directions give all eight once
# `mirror_sprites.py` fills the other three, which cuts a walk by 37% AND — because a
# clip's directions are one job each against a fixed cap — lets two characters' walks run
# at once instead of one.
MIRRORED_DIRS = ["south", "north", "south-east", "east", "north-east"]
CLIP_DIRS = {"walk": MIRRORED_DIRS, "attack": ["south"]}
TOKEN = os.environ.get("PIXELLAB_TOKEN", "")


def call(tool, args):
    """One JSON-RPC-over-SSE tool call. Returns the text payload."""
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": tool, "arguments": args}}).encode()
    req = urllib.request.Request(ENDPOINT, data=body, headers={
        "Authorization": f"Bearer {TOKEN}",
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    })
    for attempt in range(5):
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                raw = r.read().decode()
            break
        except (urllib.error.URLError, TimeoutError) as e:
            if attempt == 4:
                raise
            log(f"    transport error ({e}); retrying in 20s")
            time.sleep(20)
    payload = raw
    for line in raw.splitlines():
        if line.startswith("data:"):
            payload = line[5:].strip()
            break
    d = json.loads(payload)
    if "error" in d:
        return f"ERROR: {d['error']}"
    try:
        return d["result"]["content"][0]["text"]
    except Exception:
        return json.dumps(d)


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


# The account's concurrency cap. A five-direction walk is five jobs, so exactly two
# characters' walks fit at once; a one-direction attack is one job, so a whole chunk of
# them goes at once.
SLOTS = 10


def active_jobs():
    out = call("list_jobs", {})
    if "no active jobs" in out.lower():
        return 0, ""
    first = next((l for l in out.splitlines() if "processing" in l or "pending" in l), "")
    n = out.split(" jobs", 1)[0].strip()
    return (int(n) if n.isdigit() else 1), first.strip()[:70]


def wait_for_slot(label, free=1):
    """Block until at least `free` job slots are open."""
    while True:
        n, first = active_jobs()
        if n <= SLOTS - free:
            return
        log(f"    {label}: {n} active ({first})")
        time.sleep(20)


def wait_for_idle(label):
    """Block until the account has no active jobs at all (before a download)."""
    while True:
        n, first = active_jobs()
        if n == 0:
            return
        log(f"    {label}: waiting on {n} ({first})")
        time.sleep(20)


def installed_on_disk(asset):
    """Is this species' art already complete in the repo?

    THE LEDGER IS NOT THE TRUTH; THE REPO IS. The state file is a convenience that
    remembers PixelLab character ids, and it is machine-local and gitignored — so it goes
    missing, gets reset, or arrives on a different machine that has the art but not the
    ids. Trusting it alone means a wiped ledger silently REGENERATES art that already
    exists, which is how a bestiary ends up with six different first-area boars: pure
    cost, and worse than pure cost, because the shallow end of the world is exactly where
    a player notices the same species drawn six ways.

    So completeness is read off the files: eight walk directions, an attack, and the
    rotations. Anything short of that is genuinely unfinished and worth redoing.
    """
    d = ASSETS / asset
    if not (d / "rotations" / "south.png").is_file():
        return False
    # All EIGHT walk facings by name (five drawn, three mirrored), and a south attack.
    return set(ALL_DIRS) <= clip_dirs_on_disk(asset, "walk") and \
        "south" in clip_dirs_on_disk(asset, "attack")


def clip_dirs_on_disk(asset, clip):
    """Which facings of this clip actually exist, BY NAME.

    Counting is not enough: a colliding export produced seven directions with `south`
    missing, and `len(...) >= 5` happily called that finished. A set of names cannot make
    that mistake.
    """
    d = ASSETS / asset / "animations" / clip
    if not d.is_dir():
        return set()
    return {x.name for x in d.iterdir() if x.is_dir() and any(x.glob("*.png"))}


def load_state():
    """The ledger, reconciled against the repo — in BOTH directions.

    Art that is complete on disk is marked done even if the ledger has never heard of it
    (a wiped or missing ledger must never cause a re-roll). And a clip the ledger calls
    done that is NOT on disk at its full width has its flag cleared, so it gets redone.

    That second half is not hypothetical: the walk was generated south-only for a while,
    and a ledger saying `walk: true` happily carried those one-direction clips forward
    forever. A flag is a claim about the repo; the repo is what settles it.
    """
    st = json.loads(STATE.read_text()) if STATE.exists() else {}
    for d in sorted(ASSETS.iterdir()) if ASSETS.is_dir() else []:
        if d.is_dir() and installed_on_disk(d.name):
            st.setdefault(d.name, {})["installed"] = True
    for asset, s in st.items():
        # BOTH directions, and the clearing half is the one that matters. A stale
        # `installed: true` shields everything under it, so a ledger written while the
        # walk was generated south-only would carry those one-direction clips forward
        # forever - which is exactly what happened.
        if not installed_on_disk(asset):
            s.pop("installed", None)
        if s.get("installed"):
            continue
        for clip, dirs in CLIP_DIRS.items():
            if s.get(clip) and not set(dirs) <= clip_dirs_on_disk(asset, clip):
                s.pop(clip, None)
    return st


def save_state(st):
    STATE.write_text(json.dumps(st, indent=1, sort_keys=True))


def create(name, description, style):
    out = call("create_character", {
        "description": f"{description} {style}. Full body, head to toe, whole creature "
                       f"inside the frame, standing on the ground.",
        "name": name, "mode": "v3", "size": 96, "view": "low top-down",
        "detail": "high detail", "outline": "single color black outline",
    })
    for line in out.splitlines():
        if line.startswith("id:"):
            return line.split(":", 1)[1].strip()
    raise RuntimeError(f"create failed for {name}: {out[:300]}")


def existing_clip_groups(cid, clip):
    """Animation-group ids already on this character under `clip`.

    ⚠️ TWO GROUPS OF THE SAME NAME COLLIDE ON EXPORT. The download zip lays a group out as
    `animations/<name>/<dir>/`, so a second group called `walk` writes into the same
    folders as the first — and what came back was seven directions with `south` silently
    missing, because both groups had a `south` and only one survived. Re-generating a clip
    therefore has to REPLACE, not append.
    """
    out = call("get_character", {"character_id": cid, "include_preview": False})
    ids = []
    for line in out.splitlines():
        stripped = line.strip()
        if not stripped.startswith(clip):
            continue
        # `  <name> — <N> dir (...), 8f <date> [type=...] [group: <uuid>]`
        if "[group:" not in stripped:
            continue
        name = stripped.split("—", 1)[0].strip() if "—" in stripped else ""
        if name != clip:
            continue
        ids.append(stripped.split("[group:", 1)[1].split("]", 1)[0].strip())
    return ids


def animate(cid, clip, action):
    for gid in existing_clip_groups(cid, clip):
        call("delete_animation", {"animation_group_id": gid})
        log(f"    dropped a stale {clip} group ({gid[:8]})")
    out = call("animate_character", {
        "character_id": cid, "mode": "v3", "animation_name": clip,
        "action_description": action, "frame_count": 8,
        "keep_first_frame": False, "directions": CLIP_DIRS[clip],
    })
    if out.startswith("ERROR"):
        raise RuntimeError(f"{clip} failed: {out[:300]}")
    return out


def install(cid, key):
    subprocess.run(
        [str(ROOT / "client/scripts/install_class_sprite.sh"), cid, key, "creatures"],
        check=True, env={**os.environ, "PIXELLAB_TOKEN": TOKEN},
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", help="one creature key, both its leader and minion")
    ap.add_argument("--dry-run", action="store_true", help="print the plan, spend nothing")
    ap.add_argument("--force", action="store_true",
                    help="redo species whose art is already complete (spends generations "
                         "on art you already have - only for a deliberate re-roll)")
    a = ap.parse_args()
    if not TOKEN and not a.dry_run:
        sys.exit("set PIXELLAB_TOKEN")

    man = json.loads(MANIFEST.read_text())
    style = man["style"]
    # Leader before minion, species by species, shallowest biome first - the manifest is
    # already in the order a player meets them, so an interrupted run still leaves the
    # shallow end (which is what almost everyone actually sees) finished first.
    plan = []
    for c in man["creatures"]:
        if a.only and c["key"] != a.only:
            continue
        for rank in ("leader", "minion"):
            # THE BASE NAME IS THE ORDINARY CREATURE, and the leader is the marked one.
            # Most spawns are ordinary — a lone creature or a pack's rank and file — so
            # the unsuffixed key is the common case, and `<kind>_pack_leader` is the
            # variant that has to earn its own art.
            asset = f"{c['key']}_pack_leader" if rank == "leader" else c["key"]
            plan.append({"asset": asset, "desc": c[rank], "walk": c["walk"],
                         "attack": c["attack"], "gate": c["gate"]})

    log(f"{len(plan)} characters, ~{len(plan) * 15} generations "
        f"(8-dir rotations + 5-dir walk mirrored to 8 + south-only attack)")
    if a.dry_run:
        for p in plan:
            print(f"  d{p['gate']:<4} {p['asset']}")
        return

    st = load_state()
    # Work in CHUNKS rather than one character at a time, and phase-wise inside a chunk:
    # create everything, then queue every walk, then every attack, then install. A
    # south-only clip is one job, so a chunk of 8 fills the concurrency cap instead of
    # leaving 7 slots idle behind a single character — the difference between roughly
    # four hours and under one.
    #
    # It is CHUNKED rather than phase-wise over the whole list on purpose: the manifest
    # is ordered by how close to the hub a creature lives, and chunking preserves that.
    # An interrupted run leaves the shallow end finished and installed, which is what
    # almost every player actually sees; phasing over all 34 would leave everything
    # half-made and nothing installed.
    CHUNK = 8
    todo = [p for p in plan
            if a.force or not st.get(p["asset"], {}).get("installed")]
    if a.force:
        log("--force: redoing art that already exists")
    log(f"{len(plan) - len(todo)} already installed, {len(todo)} to go")

    for base in range(0, len(todo), CHUNK):
        chunk = todo[base : base + CHUNK]
        log(f"--- chunk {base // CHUNK + 1}: {', '.join(c['asset'] for c in chunk)}")

        for p in chunk:
            s_ = st.setdefault(p["asset"], {})
            if not s_.get("id"):
                wait_for_slot(p["asset"])
                s_["id"] = create(p["asset"], p["desc"], style)
                save_state(st)
                log(f"    created {p['asset']} {s_['id']}")

        for clip in ("walk", "attack"):
            # A clip is generated FROM the finished character, so the whole chunk has to
            # exist before any of its clips can be asked for.
            wait_for_idle(f"chunk/{clip}")
            for p in chunk:
                s_ = st[p["asset"]]
                if s_.get(clip):
                    continue
                # An 8-direction clip needs the whole cap to itself; a 1-direction one
                # shares happily. Asking for the clip's own width is what lets the attack
                # wave run eight-up while the walk wave takes its turn.
                wait_for_slot(f"{p['asset']}/{clip}", free=len(CLIP_DIRS[clip]))
                animate(s_["id"], clip, p[clip])
                s_[clip] = True
                save_state(st)
                log(f"    queued {p['asset']}/{clip}")

        wait_for_idle("chunk/install")
        for p in chunk:
            s_ = st[p["asset"]]
            if s_.get("installed"):
                continue
            install(s_["id"], p["asset"])
            s_["installed"] = True
            save_state(st)
            log(f"    ✔ {p['asset']} installed")

    log("done")


if __name__ == "__main__":
    main()
