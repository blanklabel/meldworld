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
ENDPOINT = "https://api.pixellab.ai/mcp"
# WALK AND ATTACK ARE SOUTH-FACING ONLY. The idle ROTATIONS are still a full eight
# directions — those come free with `create_character` — so a creature still turns to
# face where it is going. What it does not get is eight separate walk cycles, and that
# is the whole cost of the bestiary: an 8-direction clip is 8 jobs against a 10-job
# concurrency cap, so it both costs 8x and serializes everything behind it. One
# direction per clip takes a species from ~35 generations and ~8 minutes to ~7 and ~2.
# `hd2d::load_creature_clips` reuses the south clip for every facing.
CLIP_DIRS = ["south"]
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


# The account's concurrency cap. A south-only clip is ONE job, so a whole chunk of
# characters can be in flight at once — which is the entire reason the clips are
# south-only. Left at 8 rather than 10 so a retry always has somewhere to land.
SLOTS = 8


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


def load_state():
    return json.loads(STATE.read_text()) if STATE.exists() else {}


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


def animate(cid, clip, action):
    out = call("animate_character", {
        "character_id": cid, "mode": "v3", "animation_name": clip,
        "action_description": action, "frame_count": 8,
        "keep_first_frame": False, "directions": CLIP_DIRS,
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
            asset = c["key"] if rank == "leader" else f"{c['key']}_minion"
            plan.append({"asset": asset, "desc": c[rank], "walk": c["walk"],
                         "attack": c["attack"], "gate": c["gate"]})

    log(f"{len(plan)} characters, ~{len(plan) * 7} generations "
        f"(8-dir rotations + south-only walk/attack)")
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
    todo = [p for p in plan if not st.get(p["asset"], {}).get("installed")]
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
                wait_for_slot(f"{p['asset']}/{clip}")
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
