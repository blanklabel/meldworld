#!/usr/bin/env bash
# DEV/QA harness: load ONE biome's overworld maze in the real embedded client and grab
# native screenshots of it — instead of waiting for autoplay to random-walk into a deep
# ashfall/mire section. Uses the `MELD_BIOME` server override (pins every section to the
# biome) + `MELD_SEED` (fixed layout) + the file-channel screenshot request.
#
# Usage:  client/scripts/view_biome.sh <forest|desert|ashfall|tundra|mire> [seed] [frames]
# Output: /tmp/meld-biome-<biome>-<n>.png  (also left in /tmp/meld-game-latest.png)
#
# Requires the embedded binary built once:
#   (cd client && cargo build -p meld-client --release --features embedded-server)
set -euo pipefail

BIOME="${1:?usage: view_biome.sh <biome> [seed] [frames]}"
SEED="${2:-7}"
FRAMES="${3:-4}"
BIN="$(cd "$(dirname "$0")/.." && pwd)/target/release/meld-client"
SHOT_REQ=/tmp/meld-game-shot-request
SHOT=/tmp/meld-game-latest.png
LOG=/tmp/meld-biome-$BIOME.log

[ -x "$BIN" ] || { echo "build the embedded binary first: (cd client && cargo build -p meld-client --release --features embedded-server)"; exit 1; }

# A pulled-back, tilted survey camera so a whole stretch of the maze is in frame
# (edge-on sprites are fine — we're inspecting the ground/obstacle layout, not the hero).
cat > /tmp/meld-game-look.json <<'JSON'
{ "cam_pitch": 42.0, "cam_dist": 52.0, "fog_start": 120.0, "fog_end": 320.0, "orbit": false }
JSON

rm -f "$SHOT" "$SHOT_REQ"
MELD_BIOME="$BIOME" MELD_SEED="$SEED" MELD_AUTOPLAY=1 \
  MELD_PARTY=hunter,psyker,resonant,hunter "$BIN" > "$LOG" 2>&1 &
PID=$!
trap 'kill "$PID" 2>/dev/null || true' EXIT

until grep -q "running self-contained" "$LOG" 2>/dev/null; do sleep 1; done
sleep 6  # let the run enter + the biome stream in

for n in $(seq 1 "$FRAMES"); do
  rm -f "$SHOT"; touch "$SHOT_REQ"
  until [ -f "$SHOT" ]; do sleep 1; done
  cp "$SHOT" "/tmp/meld-biome-$BIOME-$n.png"
  echo "captured /tmp/meld-biome-$BIOME-$n.png"
  sleep 12
done
