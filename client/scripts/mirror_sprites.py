#!/usr/bin/env python3
"""Fill in a sprite clip's missing facings by mirroring the ones that were drawn.

The eight facings are symmetric about the north-south axis: `south` and `north` sit ON
that axis and have no partner, and the other six are three mirrored pairs —
south-east/south-west, east/west, north-east/north-west. So a clip only ever needs FIVE
generated directions (south, north, south-east, east, north-east); the western half is
the eastern half flipped horizontally.

That is a 37% cut in what an animated clip costs, and because a clip's directions are
generated as one job each against a fixed concurrency cap, it is also what lets two
characters' walk cycles run at the same time instead of one.

    python3 client/scripts/mirror_sprites.py <character-dir>

Idempotent and non-destructive: a direction that already exists is left alone, so this is
a no-op on a set that was generated with all eight. Run it BEFORE `pad_sprites.py` so the
flip operates on the raw art.

⚠️ Mirroring flips asymmetry. A creature with a torn left ear has a torn right ear when it
walks west. For a 90px pixel-art billboard at gameplay distance that is invisible, and it
is the standard trade in hand-drawn sprite work — but it is the reason a character whose
asymmetry MATTERS (a one-handed weapon that must stay in the same hand) wants all eight
generated instead.
"""
import argparse, pathlib, struct, sys, zlib

# west-facing direction -> the east-facing one it is a mirror of. `south` and `north` lie
# on the mirror axis and so are never derived.
MIRROR_OF = {
    "south-west": "south-east",
    "west": "east",
    "north-west": "north-east",
}


def read_png(path):
    d = path.read_bytes()
    pos, w, h, idat = 8, 0, 0, b""
    while pos < len(d):
        ln = struct.unpack(">I", d[pos : pos + 4])[0]
        typ, body = d[pos + 4 : pos + 8], d[pos + 8 : pos + 8 + ln]
        if typ == b"IHDR":
            w, h, bd, ct = struct.unpack(">IIBB", body[:10])
            if (bd, ct) != (8, 6):
                raise SystemExit(f"{path}: expected 8-bit RGBA, got depth {bd} type {ct}")
        elif typ == b"IDAT":
            idat += body
        pos += 12 + ln
    raw, stride = zlib.decompress(idat), w * 4
    out, prev, i = bytearray(), bytearray(stride), 0
    for _ in range(h):
        f = raw[i]; i += 1
        line = bytearray(raw[i : i + stride]); i += stride
        for x in range(stride):
            a = line[x - 4] if x >= 4 else 0
            b = prev[x]
            c = prev[x - 4] if x >= 4 else 0
            if f == 1: line[x] = (line[x] + a) & 255
            elif f == 2: line[x] = (line[x] + b) & 255
            elif f == 3: line[x] = (line[x] + ((a + b) >> 1)) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        out += line; prev = line
    return w, h, bytes(out)


def write_png(path, w, h, px):
    raw = b"".join(b"\x00" + px[y * w * 4 : (y + 1) * w * 4] for y in range(h))
    def chunk(t, b):
        return struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b))
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def flip(src, dst):
    w, h, px = read_png(src)
    out = bytearray(len(px))
    for y in range(h):
        row = y * w * 4
        for x in range(w):
            s = row + x * 4
            d = row + (w - 1 - x) * 4
            out[d : d + 4] = px[s : s + 4]
    write_png(dst, w, h, bytes(out))


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dir", type=pathlib.Path, help="character folder (animations/<clip>/<dir>/)")
    a = ap.parse_args()
    anims = a.dir / "animations"
    if not anims.is_dir():
        print(f"{a.dir}: no animations/, nothing to mirror")
        return
    made = 0
    for clip in sorted(p for p in anims.iterdir() if p.is_dir()):
        for west, east in MIRROR_OF.items():
            src_dir, dst_dir = clip / east, clip / west
            if dst_dir.is_dir() or not src_dir.is_dir():
                continue  # already drawn, or nothing to mirror from
            dst_dir.mkdir(parents=True)
            for frame in sorted(src_dir.glob("*.png")):
                flip(frame, dst_dir / frame.name)
                made += 1
    print(f"{a.dir.name}: mirrored {made} frames into the western facings")


if __name__ == "__main__":
    main()
