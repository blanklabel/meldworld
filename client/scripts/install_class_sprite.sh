#!/usr/bin/env bash
# Pull a finished PixelLab character into the repo as a class sprite set.
#
#   client/scripts/install_class_sprite.sh <character-id> <key> [asset-dir]
#
# `asset-dir` is the folder under `assets/` the set lands in — `characters` (default,
# hero classes) or `creatures` (the roaming bestiary, which gets the same treatment: a
# creature that used to be one static 32px png now turns and walks like a hero does).
#
# PixelLab hands back a zip already in the renderer's layout — `rotations/<dir>.png`
# and `animations/<clip>/<dir>/frame_NNN.png` — under a folder named after the
# character STATE ("Idle"), not the class, so this lifts that folder's contents into
# `characters/<class-key>/` and drops the state name.
#
# It then runs pad_sprites.py, which is not optional: PixelLab used to inflate its
# canvas ~40% past the requested size and no longer does, so a fresh 96px character
# fills its whole frame where the shipped classes fill 48% of theirs — and since the
# billboard maps the WHOLE png onto a fixed quad, an unpadded class walks into the
# party at twice the size of everyone else. See docs/asset-pipeline.md.
set -euo pipefail

[ $# -ge 2 ] || { echo "usage: $0 <character-id> <key> [asset-dir]" >&2; exit 2; }
CID=$1 CLASS=$2 ASSET_DIR=${3:-characters}
: "${PIXELLAB_TOKEN:?set PIXELLAB_TOKEN to your PixelLab API key}"

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DEST="$ROOT/client/crates/meld-client/assets/$ASSET_DIR/$CLASS"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

echo "==> downloading $CID"
curl --fail --silent --show-error \
  -H "Authorization: Bearer $PIXELLAB_TOKEN" \
  -o "$TMP/c.zip" "https://api.pixellab.ai/mcp/characters/$CID/download"
unzip -q "$TMP/c.zip" -d "$TMP/x"

# Exactly one state folder beside metadata.json; anything else means the character
# has multiple states and the caller has to say which one they meant.
STATE=$(find "$TMP/x" -mindepth 1 -maxdepth 1 -type d)
[ "$(echo "$STATE" | wc -l)" -eq 1 ] || { echo "expected one state folder, got:"; echo "$STATE"; exit 1; }

echo "==> installing -> $ASSET_DIR/$CLASS"
rm -rf "$DEST"; mkdir -p "$DEST"
cp -R "$STATE"/. "$DEST"/
[ -f "$TMP/x/metadata.json" ] && cp "$TMP/x/metadata.json" "$DEST"/

# Fill the western facings from their eastern mirrors BEFORE padding, so the flip
# operates on the raw art. A no-op on a set that was generated with all eight.
python3 "$ROOT/client/scripts/mirror_sprites.py" "$DEST"
python3 "$ROOT/client/scripts/pad_sprites.py" "$DEST"

echo "==> $CLASS: $(find "$DEST" -name '*.png' | wc -l | tr -d ' ') frames"
find "$DEST/animations" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | while read -r a; do
  echo "    $(basename "$a"): $(ls "$a" | wc -l | tr -d ' ') dirs"
done
