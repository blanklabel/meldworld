#!/usr/bin/env python3
"""Rewrite `CREATURE_CHARS` from the creature sets that are actually COMPLETE on disk.

A set counts as complete when it has all eight walk facings, a south attack and its idle
rotations — the same things `hd2d::load_creature_clips` asks for. Listing a half-finished
set is a wall of missing-asset errors every launch, so the list tracks what is finished
rather than what has been started. Run this after a generation batch lands.
"""
import pathlib, re

ROOT = pathlib.Path(__file__).resolve().parents[2]
ASSETS = ROOT / "client/crates/meld-client/assets/creatures"
SRC = ROOT / "client/crates/meld-client/src/world_render.rs"
DIRS = ["south", "south-east", "east", "north-east", "north", "north-west", "west",
        "south-west"]


# ⚠️ THE FRAME COUNT IS A PROPERTY OF THE ART, NOT A CONSTANT.
#
# This asked every walk for exactly eight frames, so a six-frame one — which is what the
# stock `walking` template produces — counted as unfinished forever and, if listed
# anyway, threw a missing-asset error per absent frame per direction on every launch.
#
# Both fixes were wrong. Regenerating them costs credits and overwrites art somebody
# made by hand; hardcoding 8 makes the repo argue with its own files. The count is read
# off the frames instead and carried into the loader, so a 6-frame walk plays as six.
MIN_FRAMES = 4


def walk_frames(d):
    """How many frames this set's walk actually has, or 0 if it has no usable walk."""
    counts = []
    for x in DIRS:
        w = d / "animations" / "walk" / x
        n = len(list(w.glob("frame_*.png"))) if w.is_dir() else 0
        if n < MIN_FRAMES:
            return 0
        counts.append(n)
    # Every facing must agree: a walk with eight frames east and six west would step at
    # two different rates depending on which way the creature happened to be going.
    return counts[0] if len(set(counts)) == 1 else 0


def complete(d):
    if not (d / "rotations" / "south.png").is_file():
        return False
    if not (d / "animations" / "attack" / "south" / "frame_000.png").is_file():
        return False
    return walk_frames(d) > 0


def main():
    done, partial = [], []
    for d in sorted(p for p in ASSETS.iterdir() if p.is_dir()) if ASSETS.is_dir() else []:
        (done.append((d.name, walk_frames(d))) if complete(d) else partial.append(d.name))
    body = "".join(f'    ("{k}", {n}),\n' for k, n in done)
    listing = (f"pub(crate) const CREATURE_CHARS: &[(&str, usize)] = &[\n{body}];"
               if done else "pub(crate) const CREATURE_CHARS: &[(&str, usize)] = &[];")
    s = SRC.read_text()
    SRC.write_text(re.sub(r'pub\(crate\) const CREATURE_CHARS: &\[\(?&str[^=]*= &\[[^\]]*\];',
                          listing, s, count=1))
    print(f"listed {len(done)} complete set(s)")
    if partial:
        print(f"still unfinished ({len(partial)}): {', '.join(partial)}")


if __name__ == "__main__":
    main()
