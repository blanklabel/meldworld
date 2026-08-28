#!/usr/bin/env python3
"""Turn a finished `create_tiles_pro` batch into a biome's ground atlas (or its cliff).

    python3 client/scripts/install_ground_biome.py <biome-key> <tiles-pro-id> [--cliff]

A cliff is ONE 64px tile, not an atlas, and it is sampled wrapping against itself — so the
tile that matters is the one whose right edge best continues into its own left. This takes
the least-mismatched of the sixteen rather than a nice-looking one, because the spread
within a batch is wide (35.1 to 62.3 on the Amber Wood) and picking by eye throws most of
that away.
"""
import argparse
import pathlib
import subprocess
import sys
import urllib.request

BASE = "https://backblaze.pixellab.ai/file/pixellab-tiles"
ACCOUNT = "f684d661-eac2-413f-8293-5e32d2af446a"
HERE = pathlib.Path(__file__).resolve().parent
GROUND = HERE.parent / "crates/meld-client/assets/ground"


def fetch(tiles_id, into):
    into.mkdir(parents=True, exist_ok=True)
    out = []
    for i in range(16):
        dst = into / f"tile_{i:02d}.png"
        req = urllib.request.Request(
            f"{BASE}/{ACCOUNT}/{tiles_id}/tile_{i}.png",
            headers={"User-Agent": "meldworld-asset-pipeline"},
        )
        with urllib.request.urlopen(req) as r:
            dst.write_bytes(r.read())
        out.append(dst)
    return out


def wrap_cost(path):
    from PIL import Image

    im = Image.open(path).convert("RGB")
    p, (w, h) = im.load(), im.size
    lr = sum(abs(p[w - 1, y][c] - p[0, y][c]) for y in range(h) for c in range(3)) / (h * 3)
    tb = sum(abs(p[x, h - 1][c] - p[x, 0][c]) for x in range(w) for c in range(3)) / (w * 3)
    return lr + tb


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("biome")
    ap.add_argument("tiles_id")
    ap.add_argument("--cliff", action="store_true")
    a = ap.parse_args()

    tmp = pathlib.Path("/tmp/meld-ground") / a.tiles_id
    tiles = fetch(a.tiles_id, tmp)

    if a.cliff:
        best = min(tiles, key=wrap_cost)
        costs = sorted(wrap_cost(t) for t in tiles)
        dst = GROUND / f"cliff_{a.biome}.png"
        dst.write_bytes(best.read_bytes())
        print(f"  {dst.name}: {best.name}, wrap cost {costs[0]:.1f}"
              f" (batch spread {costs[0]:.1f}-{costs[-1]:.1f})")
    else:
        dst = GROUND / "atlas" / f"{a.biome}.png"
        subprocess.run(
            [sys.executable, str(HERE / "pack_ground_atlas.py"), str(tmp), str(dst)],
            check=True,
        )


if __name__ == "__main__":
    main()
