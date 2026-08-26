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
# Set by --manifest / --assets, which is what lets this drive the town's NPCs as well
# as the bestiary: they are the same job (make a character, give it a walk, install it),
# differing only in what each one needs.
MANIFEST = ROOT / "client/scripts/creature_sprites.json"
STATE = ROOT / "client/scripts/.creature_sprites_state.json"
ASSETS = ROOT / "client/crates/meld-client/assets/creatures"
ENDPOINT = "https://api.pixellab.ai/mcp"
ASSET_DIR = "creatures"
# asset -> which clips it needs, filled from the manifest in main().
NEEDS = {}

ALL_DIRS = ["south", "south-east", "east", "north-east", "north", "north-west", "west",
            "south-west"]

# THE WALK USES THE STOCK `walking` TEMPLATE, which is what the hand-made half of this
# bestiary already uses (`type=v3:walking`) — so generated creatures move like curated
# ones instead of each having its own invented gait. It is also the cheapest thing here:
# a template is ONE generation per direction, so all eight real facings cost eight, where
# five custom facings cost ten and still needed three mirrored.
#
# The usual objection to templates does not apply to creatures. Template animations
# retarget onto a skeleton and DROP hand-held weapons mid-swing, which is why the hero
# classes use custom v3 — but a boar has nothing in its hands. The ATTACK stays custom,
# because "rearing up and slamming down" is the thing that makes a creature read as
# itself and no template knows it.
WALK_TEMPLATE = "walking"
CLIP_DIRS = {"walk": ALL_DIRS, "attack": ["south"]}
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
        raise RuntimeError(f"{tool}: {d['error']}")
    result = d.get("result", {})
    try:
        text = result["content"][0]["text"]
    except Exception:
        return json.dumps(d)
    # ⚠️ A FAILED TOOL CALL COMES BACK 200 WITH `isError`. Returning its message as an
    # ordinary string is how `delete_animation` silently did nothing for a whole run
    # while this script logged "dropped a stale group" after every one of them — the
    # exact unchecked-success bug the install path had just been fixed for.
    if result.get("isError"):
        raise RuntimeError(f"{tool} failed: {text[:300]}")
    return text


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


def clips_for(entry):
    """Which clips this character actually needs.

    NOT EVERY CHARACTER FIGHTS. A townsfolk needs to walk and nothing else, so demanding
    an attack of one would leave every innkeeper permanently "unfinished" and re-queued
    on every run. An entry declares its own needs by carrying an `attack` description or
    not — so a soldier gets one and a shopkeeper does not.
    """
    return ["walk"] + (["attack"] if entry.get("attack") else [])


def installed_on_disk(asset, clips=("walk", "attack")):
    """Is this species' art already complete in the repo?

    THE LEDGER IS NOT THE TRUTH; THE REPO IS. The state file is a convenience that
    remembers PixelLab character ids, and it is machine-local and gitignored — so it goes
    missing, gets reset, or arrives on a different machine that has the art but not the
    ids. Trusting it alone means a wiped ledger silently REGENERATES art that already
    exists, which is how a bestiary ends up with six different first-area boars: pure
    cost, and worse than pure cost, because the shallow end of the world is exactly where
    a player notices the same species drawn six ways.

    So completeness is read off the files: the rotations, eight walk directions, and an
    attack IF this character is one that fights. Anything short is genuinely unfinished.
    """
    d = ASSETS / asset
    if not (d / "rotations" / "south.png").is_file():
        return False
    if "walk" in clips and not set(ALL_DIRS) <= clip_dirs_on_disk(asset, "walk"):
        return False
    return "attack" not in clips or "south" in clip_dirs_on_disk(asset, "attack")


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
        if d.is_dir() and installed_on_disk(d.name, NEEDS.get(d.name, ("walk", "attack"))):
            st.setdefault(d.name, {})["installed"] = True
    for asset, s in st.items():
        # BOTH directions, and the clearing half is the one that matters. A stale
        # `installed: true` shields everything under it, so a ledger written while the
        # walk was generated south-only would carry those one-direction clips forward
        # forever - which is exactly what happened.
        if not installed_on_disk(asset, NEEDS.get(asset, ("walk", "attack"))):
            s.pop("installed", None)
        if s.get("installed"):
            continue
        for clip in NEEDS.get(asset, ("walk", "attack")):
            dirs = CLIP_DIRS[clip]
            on_disk = clip_dirs_on_disk(asset, clip)
            if set(dirs) <= on_disk:
                # Drawn and complete: record it, even if this ledger never saw it made.
                # Only CLEARING flags was half a rule — art pulled straight from the
                # account arrives with no flags at all, so a finished walk read as
                # missing and would have been generated a second time over the top of
                # itself.
                s[clip] = True
            elif s.get(clip):
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


def character_exists(cid):
    try:
        call("get_character", {"character_id": cid, "include_preview": False})
        return True
    except RuntimeError as e:
        if "not found" in str(e):
            return False
        raise


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
        call("delete_animation", {"character_id": cid, "animation_group_id": gid})
        log(f"    dropped a stale {clip} group ({gid[:8]})")
    left = existing_clip_groups(cid, clip)
    if left:
        # Say so rather than generating into a group that is still there: a second group
        # of the same name collides with the first on export and silently drops facings.
        raise RuntimeError(f"{clip} still has groups after delete: {left}")
    args = {
        "character_id": cid, "animation_name": clip,
        "directions": CLIP_DIRS[clip],
    }
    if clip == "walk":
        # Template mode: the frame count is the template's, and `mode` is auto-detected
        # from the presence of a template id.
        args["template_animation_id"] = WALK_TEMPLATE
    else:
        args |= {"mode": "v3", "action_description": action, "frame_count": 8,
                 "keep_first_frame": False}
    out = call("animate_character", args)
    if out.startswith("ERROR"):
        raise RuntimeError(f"{clip} failed: {out[:300]}")
    return out


def install(cid, key, clips=("walk", "attack")):
    """Download, mirror, pad — then CHECK, and report whether it actually landed.

    An install that says "done" without looking is how half-finished sets get marked
    finished and skipped forever. The caller only records `installed` when this returns
    True, so an incomplete set is simply picked up again by the next run.
    """
    # The download is a large zip over a flaky link and a whole batch should not die on
    # one dropped connection — this run lost twenty minutes of queued work to a single
    # curl exit 56. Retry, then give up on THIS species rather than on the run.
    for attempt in range(4):
        r = subprocess.run(
            [str(ROOT / "client/scripts/install_class_sprite.sh"), cid, key, ASSET_DIR],
            env={**os.environ, "PIXELLAB_TOKEN": TOKEN},
        )
        if r.returncode == 0:
            break
        if attempt == 3:
            log(f"    ⚠ {key}: install failed {r.returncode} four times; next pass will retry")
            return False
        log(f"    {key}: install failed ({r.returncode}), retrying in 20s")
        time.sleep(20)
    if installed_on_disk(key, clips):
        return True
    missing = sorted(set(ALL_DIRS) - clip_dirs_on_disk(key, "walk"))
    log(f"    ⚠ {key} came back incomplete (walk missing {', '.join(missing) or 'nothing'}"
        f"); leaving it for the next pass")
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", help="one creature key, both its leader and minion")
    ap.add_argument("--dry-run", action="store_true", help="print the plan, spend nothing")
    ap.add_argument("--manifest", help="sprite manifest (default: the bestiary's)")
    ap.add_argument("--assets", help="asset subdir under assets/ (default: creatures)")
    ap.add_argument("--force", action="store_true",
                    help="redo species whose art is already complete (spends generations "
                         "on art you already have - only for a deliberate re-roll)")
    a = ap.parse_args()
    if not TOKEN and not a.dry_run:
        sys.exit("set PIXELLAB_TOKEN")

    global MANIFEST, STATE, ASSETS, ASSET_DIR
    if a.manifest:
        MANIFEST = ROOT / a.manifest
        STATE = MANIFEST.with_name("." + MANIFEST.stem + "_state.json")
    if a.assets:
        ASSET_DIR = a.assets
        ASSETS = ROOT / "client/crates/meld-client/assets" / a.assets
    man = json.loads(MANIFEST.read_text())
    global NEEDS
    NEEDS = {v: tuple(clips_for(c)) for c in man["creatures"] for v in c["variants"]}
    style = man["style"]
    # Leader before minion, species by species, shallowest biome first - the manifest is
    # already in the order a player meets them, so an interrupted run still leaves the
    # shallow end (which is what almost everyone actually sees) finished first.
    # A species is a POOL of variants now, not a leader/ordinary pair. `<key>` is the
    # ordinary creature, `<key>_pack_leader` leads a pack, and the rest are its siblings —
    # so five of a species draws five different bodies rather than one sprite at five
    # sizes. Order within a species is the manifest's, so an interrupted run leaves whole
    # species finished rather than a scatter.
    plan = []
    for c in man["creatures"]:
        if a.only and c["key"] != a.only:
            continue
        for asset, desc in c["variants"].items():
            plan.append({"asset": asset, "desc": desc, "walk": c["walk"],
                         "attack": c.get("attack"), "gate": c.get("gate", 0)})

    log(f"{len(plan)} characters, ~{len(plan) * 12} generations "
        f"(8-dir rotations + templated 8-dir walk + south-only attack)")
    if a.dry_run:
        for p in plan:
            print(f"  d{p['gate']:<4} {p['asset']}")
        return

    # START FROM AN IDLE ACCOUNT. Killing this script does not cancel the jobs it has
    # already queued — they keep running server-side and land minutes later, INTO the
    # very animation groups a restarted run has just deleted. That produced sets with
    # `south` missing and sets with only `south`, and it looked like a bug in the
    # generator rather than in the handover between two runs of it.
    wait_for_idle("startup")

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

    skipped = []
    for base in range(0, len(todo), CHUNK):
        chunk = todo[base : base + CHUNK]
        log(f"--- chunk {base // CHUNK + 1}: {', '.join(c['asset'] for c in chunk)}")

        for p in chunk:
            s_ = st.setdefault(p["asset"], {})
            # A DELETED CHARACTER IS A DECISION, NOT AN ACCIDENT. Ids go stale because
            # someone tidied the PixelLab UI, and this script used to respond by
            # regenerating — which immediately undid the deletion and spent generations
            # doing it. Skipping is the only behaviour that does not fight whoever is
            # curating the account. `--force` is the way to say "yes, make it again".
            if s_.get("id") and not character_exists(s_["id"]):
                if not a.force:
                    log(f"    ⏭ {p['asset']}: character {s_['id'][:8]} was deleted - "
                        f"skipping (pass --force to remake it)")
                    skipped.append(p["asset"])
                    continue
                log(f"    {p['asset']}: character is gone; --force, so remaking it")
                s_.clear()
                save_state(st)
            if not s_.get("id"):
                try:
                    wait_for_slot(p["asset"])
                    s_["id"] = create(p["asset"], p["desc"], style)
                    save_state(st)
                    log(f"    created {p['asset']} {s_['id']}")
                except RuntimeError as e:
                    log(f"    ⚠ {p['asset']}: create failed ({e}); next pass will retry")

        for clip in ("walk", "attack"):
            # A clip is generated FROM the finished character, so the whole chunk has to
            # exist before any of its clips can be asked for.
            wait_for_idle(f"chunk/{clip}")
            for p in chunk:
                s_ = st[p["asset"]]
                if clip not in clips_for(p):
                    continue  # a townsfolk has no attack to make
                if s_.get(clip) or not s_.get("id") or p["asset"] in skipped:
                    continue
                # An 8-direction clip needs the whole cap to itself; a 1-direction one
                # shares happily. Asking for the clip's own width is what lets the attack
                # wave run eight-up while the walk wave takes its turn.
                try:
                    wait_for_slot(f"{p['asset']}/{clip}", free=len(CLIP_DIRS[clip]))
                    animate(s_["id"], clip, p[clip])
                    s_[clip] = True
                    save_state(st)
                    log(f"    queued {p['asset']}/{clip}")
                except RuntimeError as e:
                    log(f"    ⚠ {p['asset']}/{clip}: {e}; next pass will retry")

        wait_for_idle("chunk/install")
        for p in chunk:
            s_ = st[p["asset"]]
            if s_.get("installed") or p["asset"] in skipped or not s_.get("id"):
                continue
            if not install(s_["id"], p["asset"], clips_for(p)):
                # Its clips are re-queued next run: `load_state` clears any flag the
                # files do not back up, so this heals itself rather than needing a human.
                continue
            s_["installed"] = True
            save_state(st)
            log(f"    ✔ {p['asset']} installed")

    if skipped:
        log(f"skipped {len(skipped)} whose characters were deleted: {', '.join(skipped)}")
    log("done")


if __name__ == "__main__":
    main()
