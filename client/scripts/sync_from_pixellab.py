#!/usr/bin/env python3
"""Rebuild `assets/creatures/` from the PixelLab account, by character ID.

THE ACCOUNT IS THE ROSTER. Characters get renamed over there as the bestiary is curated,
and this repo's folder names came from a local ledger that does not follow — so the same
art ended up on disk under an old name while the account called it something else, and
the next planning pass read a renamed creature as a MISSING one and offered to generate
art that already existed.

So the naming authority is PixelLab and this pulls from it. The only thing imposed here
is that a variant's folder must start with a species key the world actually spawns
(`creatures_for_biome`), because that prefix is how the renderer finds a species' pool —
hence SPECIES_FIX, for names that describe the creature correctly but do not start with
its key.

    PIXELLAB_TOKEN=... python3 client/scripts/sync_from_pixellab.py [--dry-run]

Skips anything already complete on disk, so it is cheap to re-run. Downloads only; it
never generates, so it cannot cost a single credit.
"""
import argparse, json, os, pathlib, subprocess, sys, urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
ASSETS = ROOT / "client/crates/meld-client/assets/creatures"
STATE = ROOT / "client/scripts/.creature_sprites_state.json"
ENDPOINT = "https://api.pixellab.ai/mcp"
TOKEN = os.environ.get("PIXELLAB_TOKEN", "")
DIRS = ["south", "south-east", "east", "north-east", "north", "north-west", "west",
        "south-west"]

# Account name -> the name it has to have here. The species key is the prefix the
# renderer groups a pool by, so a variant whose name does not start with one is
# invisible to it however good the art is.
SPECIES_FIX = {
    # Only names that genuinely disagree with a species key belong here. The myconid
    # entries are GONE because the species key was renamed from `myconid_brute` to
    # `myconid` — the brute is one myconid among several, not the species — so every
    # `myconid_*` name the account chooses now attaches by itself. That is what a fix-map
    # should shrink toward: a rename over there should not cost a line over here.
    "bog_singer_licker": "bog_stinger_licker",
}


def call(tool, args):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": tool, "arguments": args}}).encode()
    req = urllib.request.Request(ENDPOINT, data=body, headers={
        "Authorization": f"Bearer {TOKEN}", "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream"})
    raw = urllib.request.urlopen(req, timeout=180).read().decode()
    for line in raw.splitlines():
        if line.startswith("data:"):
            d = json.loads(line[5:].strip())
            break
    else:
        d = json.loads(raw)
    r = d.get("result", {})
    text = r["content"][0]["text"]
    if r.get("isError"):
        raise RuntimeError(f"{tool}: {text[:200]}")
    return text


def complete(name, kind="creatures"):
    d = ASSETS.parent / kind / name
    return (d / "rotations/south.png").is_file() and \
        all((d / "animations/walk" / x / "frame_000.png").is_file() for x in DIRS)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--all", action="store_true",
                    help="re-pull every character even if its art looks complete here. "
                         "A clip REGENERATED on the account keeps its name, so nothing "
                         "about the folder on disk changes and no name-based check can "
                         "see it - the repo just quietly keeps serving the old motion. "
                         "This is downloads only and cannot spend a generation, so when "
                         "in doubt it is the cheap answer.")
    a = ap.parse_args()
    if not TOKEN:
        sys.exit("set PIXELLAB_TOKEN")

    import re, collections
    chars = []
    for off in (0, 50, 100):
        txt = call("list_characters", {"limit": 50, "offset": off})
        for line in txt.splitlines():
            m = re.match(r'\s+([0-9a-f-]{36}) \| (.+?) \| \ddir (\d+)x', line)
            if m and int(m.group(3)) == 96:
                chars.append((m.group(1), m.group(2).strip()))
        if "next:" not in txt:
            break

    # ⚠️ A NAME IS THE KEY HERE, SO TWO CHARACTERS SHARING ONE IS AMBIGUOUS.
    # Whichever the listing happened to return last would win, silently — and the loser
    # could be the finished one, so a run would animate an empty duplicate and install it
    # over real art. That is a coin flip, not a bug you would ever catch by reading a log,
    # so it stops the sync instead. Rename or delete one over there and run again.
    by_name = collections.defaultdict(list)
    for cid, name in chars:
        by_name[name].append(cid)
    dupes = {n: ids for n, ids in by_name.items() if len(ids) > 1}
    if dupes:
        print("DUPLICATE NAMES ON THE ACCOUNT - refusing to guess which one you meant:\n")
        for n, ids in sorted(dupes.items()):
            print(f"  {n}")
            for cid in ids:
                print(f"      {cid}")
        sys.exit("\nrename or delete one of each, then run again")

    # ⚠️ WHERE A CHARACTER BELONGS IS ALREADY WRITTEN DOWN — in which asset folder it is
    # sitting in. Everything at 96px used to be assumed to be a creature, with the hero
    # classes carved out by name; that carve-out silently did not cover the townsfolk, so
    # a `--all` run filed all 23 NPCs into the BESTIARY, duplicating every one of them and
    # listing five as loadable creatures.
    #
    # Asking the repo instead of maintaining a list of exceptions is both correct and
    # smaller: a name that already lives somewhere goes back there, and only a genuinely
    # new character falls through to the default.
    HOME = {}
    for kind in ("characters", "creatures", "npcs", "bosses"):
        d = ASSETS.parent / kind
        if d.is_dir():
            for x in d.iterdir():
                if x.is_dir():
                    HOME[x.name] = kind
    CLASSES = {n for n, k in HOME.items() if k == "characters"}
    CLASSES |= {"iron_hull_monk"}  # its folder here is `iron_hull`
    todo, skip, not_ours = [], [], []
    for cid, name in chars:
        want = SPECIES_FIX.get(name, name)
        if want in CLASSES:
            not_ours.append(want)
            continue
        # Its existing home wins; a character this repo has never seen defaults to the
        # bestiary, which is what almost every new 96px character is.
        kind = HOME.get(want, "creatures")
        (todo if a.all else (skip if complete(want, kind) else todo)).append((cid, want, kind))

    print(f"{len(chars)} 96px characters on the account; {len(not_ours)} are hero "
          f"classes, {len(skip)} creatures already complete here, {len(todo)} to pull")
    for cid, want, kind in todo:
        print(f"  {kind}/{want}")
    if a.dry_run:
        return

    # ⚠️ RECORD EVERY ID, INCLUDING THE ONES WE DID NOT DOWNLOAD.
    #
    # This is the whole reason a sync has to touch the ledger at all. The generator keys
    # off `<asset> -> character id`; a variant whose art is on disk but whose id is NOT in
    # the ledger looks to it like a creature that has never been made, so it CREATES A
    # SECOND CHARACTER with the same name. That happened: four creatures that needed
    # nothing but an attack clip got brand-new duplicates instead, which is exactly the
    # "a shit ton of first area creatures" failure this pipeline is supposed to prevent.
    #
    # The account is the roster, so the account is where ids come from — and they are
    # written down here even for a variant this run skipped, because skipping means "you
    # already have it", never "it does not exist".
    state = json.loads(STATE.read_text()) if STATE.exists() else {}
    for cid, want, _kind in skip + todo:
        state.setdefault(want, {})["id"] = cid
    STATE.write_text(json.dumps(state, indent=1, sort_keys=True))
    print(f"recorded {len(skip) + len(todo)} character ids in the ledger")

    for cid, want, kind in todo:
        subprocess.run([str(ROOT / "client/scripts/install_class_sprite.sh"), cid, want,
                        kind], env={**os.environ, "PIXELLAB_TOKEN": TOKEN})


if __name__ == "__main__":
    main()
