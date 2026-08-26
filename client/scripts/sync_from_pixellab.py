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
    "myconid_mage": "myconid_brute_mage",
    "myconid_minion": "myconid_brute_minion",
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


def complete(name):
    d = ASSETS / name
    return (d / "rotations/south.png").is_file() and \
        all((d / "animations/walk" / x / "frame_000.png").is_file() for x in DIRS)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true")
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

    # The hero classes are 96px characters too, and they belong in `characters/`, not
    # here — pulling `explorer` into the bestiary would file a playable class as
    # wildlife. Read the exclusion off the repo rather than hardcoding a roster that
    # would go stale the next time a class is added.
    CLASSES = {d.name for d in (ASSETS.parent / "characters").iterdir() if d.is_dir()}
    CLASSES |= {"iron_hull_monk"}  # its folder here is `iron_hull`
    todo, skip, not_ours = [], [], []
    for cid, name in chars:
        want = SPECIES_FIX.get(name, name)
        if want in CLASSES:
            not_ours.append(want)
            continue
        (skip if complete(want) else todo).append((cid, want))

    print(f"{len(chars)} 96px characters on the account; {len(not_ours)} are hero "
          f"classes, {len(skip)} creatures already complete here, {len(todo)} to pull")
    for cid, want in todo:
        print(f"  {want}")
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
    for cid, want in skip + todo:
        state.setdefault(want, {})["id"] = cid
    STATE.write_text(json.dumps(state, indent=1, sort_keys=True))
    print(f"recorded {len(skip) + len(todo)} character ids in the ledger")

    for cid, want in todo:
        subprocess.run([str(ROOT / "client/scripts/install_class_sprite.sh"), cid, want,
                        "creatures"], env={**os.environ, "PIXELLAB_TOKEN": TOKEN})


if __name__ == "__main__":
    main()
