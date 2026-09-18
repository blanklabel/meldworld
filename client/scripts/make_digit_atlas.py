#!/usr/bin/env python3
"""Bake the HP-ring digit atlas from the game's OWN font.

The numbers in a combatant's feet ring are drawn as `ForwardDecal`s projected onto the
glass tube (see `client/crates/meld-client/src/ring_digits.rs`), so they are a TEXTURE
rather than laid-out text — and a texture has to come from somewhere.

⚠️ **IT IS BAKED, NOT RASTERISED AT RUN TIME.** The alternative is a live glyph atlas, and
Bevy's own lives behind its text pipeline where a material cannot reach it; standing up a
second rasteriser beside it to draw eleven characters that never change is a lot of moving
parts for a fixed set.

⚠️ **AND IT IS THE GAME'S FONT, not a hand-drawn numeral set.** JetBrains Mono is MONOSPACE,
which is what makes a uniform grid exact: every cell is the same width, so a glyph is picked
with nothing but a `uv_transform` offset and the digits cannot drift apart as the number
changes. Draw them by hand and the ring's numbers stop matching every other number in the
game.

The outline is baked IN. On the tube the digits cross green, red and bare dark glass as the
fight goes, so no single ink works everywhere — white on a dark stroke is legible on all
three, and baking it means one quad per glyph instead of the two the UI version needed.

Regenerate:  python3 client/scripts/make_digit_atlas.py
"""
import os
from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
FONT = os.path.join(ROOT, "crates/meld-client/assets/fonts/JetBrainsMonoNerdFont-Regular.ttf")
OUT = os.path.join(ROOT, "crates/meld-client/assets/fx/digits")

# ⚠️ THE ORDER IS THE ABI. `ring_digits::CHARS` indexes these files by POSITION (`<i>.png`);
# a character inserted here and not there picks the neighbouring glyph, silently.
CHARS = "0123456789/"
CELL = 128          # square cells, so a glyph's aspect is the cell's
GLYPH = 86          # cap height in pixels. ⚠️ **THE MARGIN IS A BUDGET, NOT A LUXURY.** The
                    # quad this lands on may not be wider than the tube it prints on (see
                    # `ring_digits::CELL`), so every pixel of margin is height the number gives
                    # up — at 74 the ink was two thirds of the cell and the digits read small.
                    # What the margin still has to buy: a decal seen at a steep angle smears
                    # its edge texels, and margin is what it smears into rather than the
                    # neighbouring glyph.
STROKE = 12         # the baked outline. ⚠️ **IT IS FAT BECAUSE THE NUMBER IS SMALL.** The
                    # digits print onto a tube ~20 screen pixels across, so a stroke scaled for
                    # the atlas (7px of 128) came out at about ONE pixel in the game and the
                    # white glyph sat on bright green with nothing separating them. The outline
                    # is most of the legibility at this size, not a finishing touch.


def main():
    # ⚠️ **ONE FILE PER GLYPH, NOT ONE STRIP.** This was a single atlas picked apart with a
    # `uv_transform`, which is the obvious and cheaper shape — and a forward decal does not
    # sample the UV it was given. It SHIFTS it, by however far its own fragment is from the
    # surface the depth buffer holds under it, and at the edge of a quad that overhangs the
    # tube that shift is large. On a strip, large means several cells over: the leading digits
    # came out as horizontal stripes of their neighbours' edges, sampled again and again.
    #
    # Separate textures make the bug unreachable rather than unlikely. Bevy's default sampler
    # clamps to edge, so an overflowing UV can only ever reach this glyph's own transparent
    # border — there is nothing else in the image to find.
    font = ImageFont.truetype(FONT, GLYPH)
    os.makedirs(OUT, exist_ok=True)
    for i, ch in enumerate(CHARS):
        img = Image.new("RGBA", (CELL, CELL), (255, 255, 255, 0))
        ImageDraw.Draw(img).text(
            (CELL // 2, CELL // 2),
            ch,
            font=font,
            fill=(255, 255, 255, 255),
            stroke_width=STROKE,
            stroke_fill=(6, 8, 8, 255),
            anchor="mm",
        )
        img.save(os.path.join(OUT, f"{i}.png"))
    print(f"{OUT}/  {len(CHARS)} files of {CELL}x{CELL}px for {CHARS!r}")


if __name__ == "__main__":
    main()
