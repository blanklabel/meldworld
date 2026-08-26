#!/usr/bin/env python3
"""Pad PixelLab character PNGs out to the canvas the renderer expects.

WHY THIS EXISTS. A character billboard maps the WHOLE PNG onto a fixed-size quad
(`hd2d::cyl_billboard_mesh`), so what decides how big a hero draws in the world is
not the art's pixel height — it is the fraction of the canvas the art fills. The
class sets shipped before this script were generated when PixelLab inflated its
canvas ~40% past the requested size, so they are ~90px of art centred on a 184px
canvas: a 48% fill. PixelLab no longer does that — a `size: 96` request now returns
a 96px canvas with the art running edge to edge — so a newly generated class drops
into the party at roughly TWICE the size of the ones beside it.

The fix is padding, never scaling: pixel art must not be resampled, so the art is
copied through untouched and only transparent margin is added around it. Run this
over a freshly downloaded character folder before committing it.

    python3 client/scripts/pad_sprites.py <character-dir> [--canvas 184]

Idempotent: a file already at the target canvas is left alone.
"""
import argparse, pathlib, struct, sys, zlib


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


def pad(path, canvas):
    w, h, px = read_png(path)
    if w == canvas and h == canvas:
        return False
    if w > canvas or h > canvas:
        raise SystemExit(f"{path}: {w}x{h} is larger than the {canvas} canvas - regenerate smaller, never downscale")
    # Centred, matching how the existing class sets sit in their canvas (measured:
    # psyker 47px above / 48px below). The billboard grounds the image BOTTOM, so an
    # off-centre pad would sink or float the hero relative to its own shadow.
    ox, oy = (canvas - w) // 2, (canvas - h) // 2
    out = bytearray(canvas * canvas * 4)
    for y in range(h):
        src = y * w * 4
        dst = ((y + oy) * canvas + ox) * 4
        out[dst : dst + w * 4] = px[src : src + w * 4]
    write_png(path, canvas, canvas, bytes(out))
    return True


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dir", type=pathlib.Path, help="character folder (rotations/ + animations/)")
    ap.add_argument("--canvas", type=int, default=184, help="target square canvas (default 184, the shipped class size)")
    a = ap.parse_args()
    if not a.dir.is_dir():
        raise SystemExit(f"{a.dir}: not a directory")
    files = sorted(a.dir.rglob("*.png"))
    if not files:
        raise SystemExit(f"{a.dir}: no PNGs found")
    n = sum(pad(f, a.canvas) for f in files)
    print(f"{a.dir}: padded {n}/{len(files)} to {a.canvas}x{a.canvas}")


if __name__ == "__main__":
    main()
