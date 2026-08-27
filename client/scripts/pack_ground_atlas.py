#!/usr/bin/env python3
"""Pack a `create_tiles_pro` batch into one 4x4 ground atlas.

A tiles_pro batch is SIXTEEN VARIATIONS OF ONE MATERIAL — that is what the generator is
for, and the UI says so: "describe one material; variations of it make organic-looking
ground". Picking one and discarding fifteen throws away the entire point and leaves the
ground a single 64px stamp repeated to the horizon, which is the monotony we then tried
to paper over in the shader.

So all sixteen ship, packed into one 256x256 texture (4x4 of 64px). The shader picks two
variations by a smooth low-frequency field and CROSS-FADES between them, rather than
hard-switching per cell — a hard switch would just trade a repeating grid for a mosaic of
visible tile borders, since these variations do not share edges.

    python3 client/scripts/pack_ground_atlas.py <tiles-dir> <out.png>

Tiles are laid out in sorted filename order, which is the generator's own numbering.
"""
import argparse, pathlib, struct, sys, zlib

CELL, GRID = 64, 4


def read_png(path):
    d = path.read_bytes()
    pos, w, h, idat = 8, 0, 0, b""
    while pos < len(d):
        ln = struct.unpack(">I", d[pos:pos + 4])[0]
        typ, body = d[pos + 4:pos + 8], d[pos + 8:pos + 8 + ln]
        if typ == b"IHDR":
            w, h, bd, ct = struct.unpack(">IIBB", body[:10])
            if (bd, ct) != (8, 6):
                sys.exit(f"{path}: expected 8-bit RGBA")
        elif typ == b"IDAT":
            idat += body
        pos += 12 + ln
    raw, stride = zlib.decompress(idat), w * 4
    out, prev, i = bytearray(), bytearray(stride), 0
    for _ in range(h):
        f = raw[i]; i += 1
        line = bytearray(raw[i:i + stride]); i += stride
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
    raw = b"".join(b"\x00" + px[y * w * 4:(y + 1) * w * 4] for y in range(h))
    def chunk(t, b):
        return struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b))
    path.write_bytes(b"\x89PNG\r\n\x1a\n"
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("src", type=pathlib.Path)
    ap.add_argument("dst", type=pathlib.Path)
    a = ap.parse_args()
    files = sorted(a.src.rglob("*.png"))
    if len(files) < GRID * GRID:
        sys.exit(f"{a.src}: need {GRID * GRID} tiles, found {len(files)}")
    side = CELL * GRID
    atlas = bytearray(side * side * 4)
    for i, f in enumerate(files[:GRID * GRID]):
        w, h, px = read_png(f)
        if (w, h) != (CELL, CELL):
            sys.exit(f"{f.name}: {w}x{h}, expected {CELL}x{CELL}")
        gx, gy = i % GRID, i // GRID
        for y in range(CELL):
            src = y * w * 4
            dst = ((gy * CELL + y) * side + gx * CELL) * 4
            atlas[dst:dst + CELL * 4] = px[src:src + CELL * 4]
    write_png(a.dst, side, side, bytes(atlas))
    print(f"  {a.dst.name}: {GRID}x{GRID} atlas of {CELL}px ({side}x{side})")


if __name__ == "__main__":
    main()
