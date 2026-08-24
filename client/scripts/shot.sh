#!/usr/bin/env bash
# DEV/QA: take a REPRODUCIBLE native screenshot of the game.
#
# ⚠️ THIS EXISTS BECAUSE UNCONTROLLED CAPTURES PRODUCE CONFIDENT WRONG ANSWERS.
# Comparing two frames only means something if everything except the thing under test is
# held still, and three separate failures made that untrue:
#
#   * the avatar WALKS. `MELD_AUTOPLAY` marches east, so two runs frame different ground
#     and "this biome is darker" may just be "this frame has more sky in it". `MELD_IDLE`
#     enters the dive and then stands still, which is what makes two frames comparable.
#   * the CAMERA drifts unless pinned. The look file is written atomically here, because
#     the client hot-reloads it and reading it mid-write logs `EOF while parsing` and keeps
#     the old camera.
#   * the capture comes back PURE BLACK whenever the window is not frontmost (and always,
#     with the screen locked). A black frame is indistinguishable from a dark scene, which
#     is how "the mire renders at night" survived as a diagnosis for half a day. So the
#     window is fronted, the frame is checked, and a black one is RETRIED rather than
#     measured.
#
# Usage:  scripts/shot.sh <out.png> [KEY=VAL ...]     # extra env goes to the client
#   e.g.  scripts/shot.sh /tmp/mire.png MELD_BIOME=mire MELD_START_LEVEL=25
#
# Camera: override with LOOK_PITCH / LOOK_YAW / LOOK_DIST / FOG_START / FOG_END.
set -euo pipefail

OUT="${1:?usage: shot.sh <out.png> [KEY=VAL ...]}"; shift || true
BIN="$(cd "$(dirname "$0")/.." && pwd)/target/release/meld-client"
SHOT=/tmp/meld-game-latest.png
REQ=/tmp/meld-game-shot-request
LOG="/tmp/shot-$(basename "$OUT" .png).log"

[ -x "$BIN" ] || { echo "build first: (cd client && cargo build -p meld-client --release --features embedded-server)" >&2; exit 1; }

# Pin the camera. Written to a temp file and MOVED, so the hot-reloader never reads a
# half-written file.
cat > /tmp/meld-look.tmp <<JSON
{ "cam_pitch": ${LOOK_PITCH:-45.0}, "cam_yaw": ${LOOK_YAW:-0.0}, "cam_dist": ${LOOK_DIST:-30.0},
  "fog_start": ${FOG_START:-200.0}, "fog_end": ${FOG_END:-600.0}, "orbit": false }
JSON
mv -f /tmp/meld-look.tmp /tmp/meld-game-look.json

pkill -f "target/release/meld-client" 2>/dev/null || true
sleep 2
rm -f "$SHOT" "$REQ"

# Noon, fair weather, a day long enough that it cannot advance mid-session: time of day and
# a passing storm both change the exposure, and neither is what a comparison is about.
env MELD_AUTOPLAY=1 MELD_IDLE=1 \
    MELD_WORLD_FEEL="${MELD_WORLD_FEEL:-sky_t=0.5,day_len=100000,fair_secs=100000}" \
    "$@" "$BIN" > "$LOG" 2>&1 &
PID=$!
trap 'kill "$PID" 2>/dev/null || true' EXIT

until grep -q "running self-contained" "$LOG" 2>/dev/null; do sleep 1; done
sleep "${SETTLE:-12}"   # let the dive start and the world stream in

for attempt in 1 2 3 4 5; do
  osascript -e 'tell application "System Events" to set frontmost of first process whose name contains "meld" to true' >/dev/null 2>&1 || true
  sleep 2
  rm -f "$SHOT"; touch "$REQ"
  for _ in $(seq 1 30); do [ -f "$SHOT" ] && break; sleep 1; done
  [ -f "$SHOT" ] || { echo "attempt $attempt: no frame produced" >&2; continue; }
  # A frame that is essentially all black is a FAILED CAPTURE, not a dark scene.
  if python3 - "$SHOT" <<'PY'
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert('RGB').resize((320, 180))
px = list(im.getdata())
lit = sum(1 for r, g, b in px if r > 12 or g > 12 or b > 12) / len(px)
sys.exit(0 if lit > 0.02 else 1)
PY
  then
    cp "$SHOT" "$OUT"
    echo "captured $OUT (attempt $attempt)"
    exit 0
  fi
  echo "attempt $attempt: black frame — window not frontmost? retrying" >&2
done
echo "FAILED: only black frames after 5 attempts (is the screen locked?)" >&2
exit 1
