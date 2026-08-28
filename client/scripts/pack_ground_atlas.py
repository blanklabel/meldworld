#!/usr/bin/env python3
"""Pack a `create_tiles_pro` batch into one 4x4 ground atlas.

A tiles_pro batch is SIXTEEN VARIATIONS OF ONE MATERIAL — that is what the generator is
for, and the UI says so: "describe one material; variations of it make organic-looking
ground". Picking one and discarding fifteen throws away the entire point and leaves the
ground a single 64px stamp repeated to the horizon, which is the monotony we then tried
to paper over in the shader.

So all sixteen ship, packed into one atlas. The shader picks two variations by a smooth
low-frequency field and CROSS-FADES between them, rather than hard-switching per cell —
a hard switch would just trade a repeating grid for a mosaic of visible tile borders,
since these variations do not share edges.

⚠️ EVERY CELL CARRIES A ONE-PIXEL GUTTER COPIED FROM ITS OPPOSITE EDGE, so the atlas is
4x4 of 66px (264x264) holding 64px tiles. A tile in an atlas cannot use the hardware
REPEAT wrap — the sampler would wrap the whole atlas, not the cell — so at the cell edge
bilinear filtering has to be given the wrapped neighbour explicitly. That is what the
gutter is: the left border is a copy of the rightmost column, the top of the bottom row,
and so on, which is exactly the texel REPEAT would have fetched.

The alternative, clamping the sample away from the cell edge, was what shipped first and
it is worse twice over. It bled anyway (the inset was computed against the ATLAS width
while being applied in CELL space, so a "half texel" was an eighth of one and filtering
still pulled a third of the neighbouring variation in as a fringe), and even done
correctly it REPEATS the edge texel instead of wrapping it — which destroys the
seamlessness of any tile that actually was seamless. A gutter preserves it.

    python3 client/scripts/pack_ground_atlas.py <tiles-dir> <out.png>
    python3 client/scripts/pack_ground_atlas.py --repack <old-atlas.png> <out.png>

Tiles are laid out in sorted filename order, which is the generator's own numbering.
"""
import argparse, pathlib, struct, sys, zlib

CELL, GRID, PAD = 64, 4, 1
STRIDE = CELL + 2 * PAD          # 66: the tile plus its wrap gutter
SIDE = STRIDE * GRID             # 264


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


def place(atlas, tile, gx, gy):
    """Blit one 64px tile into its cell, ringed by a 1px gutter of its own opposite edge.

    The modulo IS the wrap: reading the tile at -1 fetches column 63, which is the texel
    a REPEAT sampler would have blended at that boundary.
    """
    for y in range(-PAD, CELL + PAD):
        row = ((gy * STRIDE + PAD + y) * SIDE) * 4
        for x in range(-PAD, CELL + PAD):
            src = ((y % CELL) * CELL + (x % CELL)) * 4
            dst = row + (gx * STRIDE + PAD + x) * 4
            atlas[dst:dst + 4] = tile[src:src + 4]


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repack", action="store_true",
                    help="src is an existing unpadded 4x4 atlas, not a directory of tiles")
    ap.add_argument("src", type=pathlib.Path)
    ap.add_argument("dst", type=pathlib.Path)
    a = ap.parse_args()

    tiles = []
    if a.repack:
        w, h, px = read_png(a.src)
        if (w, h) != (CELL * GRID, CELL * GRID):
            sys.exit(f"{a.src}: {w}x{h}, expected an unpadded {CELL * GRID}px atlas")
        for i in range(GRID * GRID):
            gx, gy = i % GRID, i // GRID
            t = bytearray()
            for y in range(CELL):
                o = ((gy * CELL + y) * w + gx * CELL) * 4
                t += px[o:o + CELL * 4]
            tiles.append(bytes(t))
    else:
        files = sorted(a.src.rglob("*.png"))
        if len(files) < GRID * GRID:
            sys.exit(f"{a.src}: need {GRID * GRID} tiles, found {len(files)}")
        for f in files[:GRID * GRID]:
            w, h, px = read_png(f)
            if (w, h) != (CELL, CELL):
                sys.exit(f"{f.name}: {w}x{h}, expected {CELL}x{CELL}")
            tiles.append(px)

    atlas = bytearray(SIDE * SIDE * 4)
    for i, t in enumerate(tiles):
        place(atlas, t, i % GRID, i // GRID)
    write_png(a.dst, SIDE, SIDE, bytes(atlas))
    print(f"  {a.dst.name}: {GRID}x{GRID} atlas of {CELL}px +{PAD}px gutter ({SIDE}x{SIDE})")


if __name__ == "__main__":
    main()
