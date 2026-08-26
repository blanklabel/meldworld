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


def complete(d):
    if not (d / "rotations" / "south.png").is_file():
        return False
    if not (d / "animations" / "attack" / "south" / "frame_000.png").is_file():
        return False
    return all((d / "animations" / "walk" / x / "frame_000.png").is_file() for x in DIRS)


def main():
    done, partial = [], []
    for d in sorted(p for p in ASSETS.iterdir() if p.is_dir()) if ASSETS.is_dir() else []:
        (done if complete(d) else partial).append(d.name)
    body = "".join(f'    "{k}",\n' for k in done)
    listing = f"pub(crate) const CREATURE_CHARS: &[&str] = &[\n{body}];" if done else \
        "pub(crate) const CREATURE_CHARS: &[&str] = &[];"
    s = SRC.read_text()
    SRC.write_text(re.sub(r'pub\(crate\) const CREATURE_CHARS: &\[&str\] = &\[[^\]]*\];',
                          listing, s, count=1))
    print(f"listed {len(done)} complete set(s)")
    if partial:
        print(f"still unfinished ({len(partial)}): {', '.join(partial)}")


if __name__ == "__main__":
    main()
