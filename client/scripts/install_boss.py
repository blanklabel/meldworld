#!/usr/bin/env python3
"""Install a PixelLab character (or object) into `assets/bosses/<key>/`.

    python3 client/scripts/install_boss.py <key> <pixellab-id> [--object]

Pulls the account's own zip rather than fetching rotations one by one, so whatever
animations the character actually has come down with it instead of being guessed at.

Clip folders are lowercased on the way in (`Idle` -> `idle`): the account names a clip
after whoever made it, and the loader asks for a fixed name.
"""
import argparse
import io
import os
import pathlib
import shutil
import sys
import urllib.request
import zipfile

TOKEN = os.environ.get("PIXELLAB_TOKEN", "")
API = "https://api.pixellab.ai/mcp"
BOSSES = pathlib.Path(__file__).resolve().parent.parent / "crates/meld-client/assets/bosses"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("key")
    ap.add_argument("pixellab_id")
    ap.add_argument("--object", action="store_true")
    a = ap.parse_args()
    if not TOKEN:
        sys.exit("set PIXELLAB_TOKEN")

    kind = "objects" if a.object else "characters"
    req = urllib.request.Request(
        f"{API}/{kind}/{a.pixellab_id}/download",
        headers={"Authorization": f"Bearer {TOKEN}"},
    )
    with urllib.request.urlopen(req) as r:
        blob = r.read()

    dest = BOSSES / a.key
    if dest.exists():
        shutil.rmtree(dest)
    (dest / "rotations").mkdir(parents=True)

    clips = set()
    with zipfile.ZipFile(io.BytesIO(blob)) as z:
        for name in z.namelist():
            parts = name.split("/")
            # A character nests everything under its state (`Idle/rotations/...`); an
            # object does not (`rotations/...`). Find the marker rather than its depth.
            at = parts.index("rotations") if "rotations" in parts else -1
            an = parts.index("animations") if "animations" in parts else -1
            if at >= 0 and len(parts) > at + 1:
                (dest / "rotations" / parts[-1]).write_bytes(z.read(name))
            elif an >= 0 and len(parts) >= an + 4:
                clip, facing, frame = parts[an + 1].lower(), parts[an + 2], parts[an + 3]
                out = dest / "animations" / clip / facing
                out.mkdir(parents=True, exist_ok=True)
                (out / frame).write_bytes(z.read(name))
                clips.add((clip, facing))
            elif parts[-1] == "metadata.json":
                (dest / "metadata.json").write_bytes(z.read(name))

    rots = len(list((dest / "rotations").glob("*.png")))
    detail = ", ".join(
        f"{c}/{f} {len(list((dest / 'animations' / c / f).glob('*.png')))}f"
        for c, f in sorted(clips)
    )
    print(f"  {a.key}: {rots} facings" + (f" + {detail}" if detail else " (no clips)"))


if __name__ == "__main__":
    main()
