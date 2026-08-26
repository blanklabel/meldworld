#!/usr/bin/env python3
"""Give a downloaded character's animation folders the names the renderer looks for.

The on-disk folder for a clip is the animation's DISPLAY NAME in PixelLab, which is
whoever-made-it's choice: `Walking` from the stock template, `walk` from a custom action,
`Attack` or `attack` depending on the day. The loader asks for exact lowercase keys, so a
set animated in the UI installs with a `Walking/` folder the game never looks in — the
art is right there and the creature stands frozen.

This is the boundary where that gets settled: the repo layout is canonical, and PixelLab
naming is free. Run it before mirroring and padding.

    python3 client/scripts/normalize_clips.py <character-dir>

Lowercasing plus an alias table, and it refuses to merge two clips onto one name rather
than silently letting one win.
"""
import argparse, pathlib, sys

# Display name (lowercased) -> the key the renderer asks for. Derived from the names
# actually in use across the bestiary, not guessed.
#
# `running` maps to `walk` because the renderer has ONE locomotion clip and a creature
# that runs instead of walking is still just moving — a swarmer with a run cycle is the
# variety working, not a second mechanic. Anything not listed keeps its own name in
# snake_case, which is what lets genuine ability art like `fireball` survive.
ALIASES = {
    "walking": "walk",
    "walk cycle": "walk",
    "running": "walk",
    "run": "walk",
    "attacking": "attack",
}

# PixelLab falls back to the action text when an animation was never given a display
# name, so the folder comes out as the whole sentence. That is not a clip name.
UNNAMED_PREFIX = "custom-"


def canonical(name):
    key = name.strip().lower().replace("-", " ")
    return ALIASES.get(key, key.replace(" ", "_"))


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dir", type=pathlib.Path)
    a = ap.parse_args()
    anims = a.dir / "animations"
    if not anims.is_dir():
        print(f"{a.dir.name}: no animations/, nothing to normalize")
        return
    renamed = []
    for clip in sorted(p for p in anims.iterdir() if p.is_dir()):
        if clip.name.lower().startswith(UNNAMED_PREFIX):
            # Nameless in PixelLab, so its folder is the action sentence. Guessing what
            # it was meant to be is how art ends up under a key nothing plays.
            print(f"  ⚠ {a.dir.name}: animation '{clip.name}' was never named in PixelLab "
                  f"- give it a name there (probably 'attack'); leaving it as-is")
            continue
        want = canonical(clip.name)
        if want == clip.name:
            continue
        dst = anims / want
        if dst.exists():
            # Two clips claiming one key: say so rather than picking a winner, because
            # the loser is art that silently never plays.
            sys.exit(f"{a.dir.name}: both '{clip.name}' and '{want}' exist - "
                     f"rename one in PixelLab; refusing to merge them")
        clip.rename(dst)
        renamed.append(f"{clip.name} -> {want}")
    print(f"{a.dir.name}: " + (", ".join(renamed) if renamed else "clip names already canonical"))


if __name__ == "__main__":
    main()
