#!/usr/bin/env python3
"""Reconcile the PixelLab account, the assets on disk, and what the game actually LOADS.

There are three copies of "what art exists" and they drift independently:

  1. the PixelLab account  - where art is made and renamed
  2. `assets/<kind>/<key>/` - what this repo has
  3. the registries in `world_render.rs` - what the renderer opens at run time

Art has to be in all three to reach a player. Missing from (2) is invisible but cheap to
fix; missing from (3) is the dangerous one, because the art is right there in the repo,
the build is green, every test passes, and the thing simply never plays. That is not
hypothetical - `cinder_imp_fire_mage` has a `fireball` clip on disk today that nothing
declares, and eight creatures spent a whole session with their walks under `Walking/`.

The reverse is worse in a different way: a registry that names a clip with no frames
behind it is a wall of asset-loader errors on every launch.

    PIXELLAB_TOKEN=... python3 client/scripts/audit_art.py [--offline]

Read-only. Safe to run while a generation batch is going.
"""
import argparse, json, os, pathlib, re, sys, urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
ASSETS = ROOT / "client/crates/meld-client/assets"
SRC = ROOT / "client/crates/meld-client/src/world_render.rs"
ENDPOINT = "https://api.pixellab.ai/mcp"
TOKEN = os.environ.get("PIXELLAB_TOKEN", "")
KINDS = ["characters", "bosses", "creatures", "npcs"]

# Mirrors `normalize_clips.py`: the folder takes the animation's PixelLab display name,
# so `Walking` and `walk` are the same clip and comparing raw names invents differences.
ALIASES = {"walking": "walk", "walk cycle": "walk", "running": "walk", "run": "walk",
           "attacking": "attack"}


def canonical(name):
    key = name.strip().lower()
    # An animation made from a template and never given a name comes back as
    # `v3:walking` — the mode and the template, not a clip name. Ironmaw, Rustfang and
    # the Briar Lord all had their walks re-cut that way and every one of them installed
    # under a `v3:walking/` folder the loader does not open, leaving the OLD walk in
    # place. Strip the mode prefix and let the alias table do the rest.
    if ":" in key:
        key = key.split(":", 1)[1]
    key = key.replace("-", " ")
    return ALIASES.get(key, key.replace(" ", "_"))


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
    if r.get("isError"):
        raise RuntimeError(f"{tool}: {r['content'][0]['text'][:200]}")
    return r["content"][0]["text"]


def on_disk():
    """kind -> key -> {clip: {facings}}."""
    out = {}
    for kind in KINDS:
        d = ASSETS / kind
        if not d.is_dir():
            continue
        out[kind] = {}
        for s in sorted(x for x in d.iterdir() if x.is_dir()):
            clips = {}
            anim = s / "animations"
            if anim.is_dir():
                for c in sorted(x for x in anim.iterdir() if x.is_dir()):
                    clips[c.name] = {f.name for f in c.iterdir()
                                     if f.is_dir() and any(f.glob("*.png"))}
            out[kind][s.name] = clips
    return out


def declared_lengths():
    """kind -> key -> clip -> frame count, straight out of the same match arms."""
    src = SRC.read_text()
    out = {"characters": {}, "bosses": {}, "creatures": {}, "npcs": {}}
    for fn, kind in (("class_clips", "characters"), ("boss_clips", "bosses")):
        blk = src[src.index(f"fn {fn}("):]
        blk = blk[:blk.index("\n    }\n")]
        for m in re.finditer(r'((?:"[a-z_]+"\s*\|\s*)*"[a-z_]+")\s*=>\s*\{?\s*&\[(.*?)\]',
                             blk, re.S):
            pairs = dict((c, int(n)) for c, n in re.findall(r'\("(\w+)",\s*(\d+)', m.group(2)))
            for k in re.findall(r'"([a-z_]+)"', m.group(1)):
                out[kind][k] = pairs
    # Creatures and NPCs share one shape: an 8-frame walk, an 8-frame attack.
    for kind, const in (("creatures", "CREATURE_CHARS"), ("npcs", "NPC_CHARS")):
        m = re.search(rf'{const}: &\[&str\] = &\[(.*?)\];', src, re.S)
        if m:
            for k in re.findall(r'"([a-z_0-9]+)"', m.group(1)):
                out[kind][k] = {"walk": 8, "attack": 8}
    return out


def declared():
    """What the renderer opens: key -> {clip names}, per kind.

    Parsed out of the match arms rather than duplicated here, so this cannot become a
    fourth copy of the same list that drifts from the other three.
    """
    src = SRC.read_text()
    res = {"characters": {}, "bosses": {}, "creatures": {}, "npcs": {}}

    def arms(fn):
        blk = src[src.index(f"fn {fn}("):]
        blk = blk[:blk.index("\n    }\n")]
        out = {}
        for m in re.finditer(r'((?:"[a-z_]+"\s*\|\s*)*"[a-z_]+")\s*=>\s*\{?\s*&\[(.*?)\]',
                             blk, re.S):
            keys = re.findall(r'"([a-z_]+)"', m.group(1))
            clips = re.findall(r'\("(\w+)",\s*\d+', m.group(2))
            for k in keys:
                out[k] = set(clips)
        return out

    res["characters"] = arms("class_clips")
    res["bosses"] = arms("boss_clips")
    # Creatures and NPCs declare one shape for every entry in their list.
    for kind, const, clips in [("creatures", "CREATURE_CHARS", {"walk", "attack"}),
                               ("npcs", "NPC_CHARS", {"walk"})]:
        m = re.search(rf'{const}: &\[&str\] = &\[(.*?)\];', src, re.S)
        if m:
            for k in re.findall(r'"([a-z_0-9]+)"', m.group(1)):
                res[kind][k] = set(clips)
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--offline", action="store_true", help="skip the account, audit repo only")
    a = ap.parse_args()

    disk, decl = on_disk(), declared()
    problems = 0

    if not a.offline:
        if not TOKEN:
            sys.exit("set PIXELLAB_TOKEN (or pass --offline)")
        acct = {}
        for off in (0, 50, 100):
            txt = call("list_characters", {"limit": 50, "offset": off})
            for line in txt.splitlines():
                m = re.match(r'\s+([0-9a-f-]{36}) \| (.+?) \| \ddir', line)
                if m:
                    acct[m.group(2).strip()] = m.group(1)
            if "next:" not in txt:
                break
        every = {k: kind for kind, ks in disk.items() for k in ks}
        # Account names are curated and this repo files some of them under a corrected
        # key, so an account name is "in the repo" if it or its fix lands on disk.
        FIX = {"myconid_mage": "myconid_brute_mage", "myconid_minion": "myconid_brute_minion",
               "bog_singer_licker": "bog_stinger_licker", "iron_hull_monk": "iron_hull",
               "Out of the shadows steps": "briarlord"}
        def repo_key(n):
            n2 = FIX.get(n, n)
            return n2 if n2 in every else (n2.lower() if n2.lower() in every else None)

        missing = sorted(n for n in acct if not repo_key(n))
        print(f"== ON THE ACCOUNT, NOT IN THE REPO ({len(missing)}) ==")
        for n in missing:
            print(f"   {n}")
        print("   (some are deliberately not game art)\n")

        # ⚠️ THE ONE THIS AUDIT EXISTS FOR. An animation added or re-cut on the account
        # after its art was pulled does NOT appear here by itself: the folder is already
        # on disk, the build is green, every test passes, and the new clip simply is not
        # in the game. Comparing clip NAMES per character is the only thing that sees it.
        print("== CHANGED ON THE ACCOUNT SINCE IT WAS PULLED ==")
        stale = 0
        for name, cid in sorted(acct.items()):
            key = repo_key(name)
            if not key:
                continue
            try:
                txt = call("get_character", {"character_id": cid, "include_preview": False})
            except RuntimeError:
                continue
            acct_clips = set()
            for line in txt.splitlines():
                m = re.match(r'\s+(.+?) — \d+ dir', line)
                if m and "[group:" in line:
                    acct_clips.add(canonical(m.group(1).strip()))
            here = set(disk[every[key]][key])
            new_clips = acct_clips - here
            if new_clips:
                print(f"   {every[key]}/{key}  account has {', '.join(sorted(new_clips))} "
                      f"that this repo does not - re-pull it")
                stale += 1
                problems += 1
        if not stale:
            print("   nothing - the repo matches the account\n")
        else:
            print()

    print("== IN THE REPO, NOT LOADED ==")
    for kind in KINDS:
        for key, clips in disk.get(kind, {}).items():
            if key not in decl.get(kind, {}):
                print(f"   {kind}/{key}  installed but no registry entry - never loaded")
                problems += 1
                continue
            extra = set(clips) - decl[kind][key]
            if extra:
                print(f"   {kind}/{key}  has {', '.join(sorted(extra))} on disk that "
                      f"nothing declares - art nobody plays")
                problems += 1

    print("\n== DECLARED BUT NOT ON DISK (asset errors every launch) ==")
    for kind in KINDS:
        for key, clips in decl.get(kind, {}).items():
            have = disk.get(kind, {}).get(key)
            if have is None:
                print(f"   {kind}/{key}  declared but no folder")
                problems += 1
                continue
            for c in sorted(clips - set(have)):
                print(f"   {kind}/{key}/{c}  declared, no frames")
                problems += 1

    print("\n== WRONG FRAME COUNT ==")
    # `boss_clips`/`class_clips` declare a length per clip — the humanoid boss walk really
    # is 6 frames — so the check is against what was DECLARED, not a global 8. A creature
    # at 6 beside its pack at 8 walks with a different gait; a boss at 6 is correct.
    lengths = declared_lengths()
    for kind in KINDS:
        for key, clips in disk.get(kind, {}).items():
            for c, facings in clips.items():
                want = lengths.get(kind, {}).get(key, {}).get(c)
                if not want or not facings:
                    continue
                d = ASSETS / kind / key / "animations" / c / sorted(facings)[0]
                got = len(list(d.glob("*.png")))
                if got != want:
                    print(f"   {kind}/{key}/{c}  {got} frames, declared {want}")
                    problems += 1

    print("\n== INCOMPLETE FACINGS ==")
    DIRS = {"south", "south-east", "east", "north-east", "north", "north-west", "west",
            "south-west"}
    for kind in KINDS:
        for key, clips in disk.get(kind, {}).items():
            for c, facings in clips.items():
                # An attack is drawn south-only on purpose; a walk is not.
                if c == "walk" and not DIRS <= facings:
                    print(f"   {kind}/{key}/walk  missing {', '.join(sorted(DIRS - facings))}")
                    problems += 1
    print(f"\n{problems} problem(s)" if problems else "\nall art is installed and loaded ✔")


if __name__ == "__main__":
    main()
