#!/usr/bin/env bash
# Bake a login-screen backdrop clip into the WebP frame sequence the client plays.
#
#   client/scripts/bake_login_bg.sh <clip.mp4> [width] [fps] [quality]
#
# Bevy plays no video, so `LoginBg` (client/crates/meld-client/src/screens.rs) steps
# a frame sequence instead. Frames are WebP, NOT JPEG: every zune-jpeg 0.5.x — what
# bevy's `jpeg` feature pulls in — fails to build on the current rustc.
#
# Frame count and pace are compiled in as LOGIN_BG_FRAMES / LOGIN_BG_FPS; change
# either here and change them there too. Every frame is a live GPU texture while the
# login screen is up (width x height x 4 bytes each), which is what bounds the width.
#
# macOS only: decoding goes through AVFoundation (no ffmpeg needed), encoding through
# Pillow.
set -euo pipefail

CLIP="${1:?usage: bake_login_bg.sh <clip.mp4> [width] [fps] [quality]}"
WIDTH="${2:-640}"
FPS="${3:-12}"
QUALITY="${4:-82}"

NAME="$(basename "${CLIP%.*}")"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/crates/meld-client/assets/loginscreens/$NAME"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/extract.swift" <<'SWIFT'
import AVFoundation
import AppKit
import Foundation

let a = CommandLine.arguments
let url = URL(fileURLWithPath: a[1])
let outDir = a[2]
let fps = Double(a[3])!
let width = Double(a[4])!

try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)

let asset = AVURLAsset(url: url)
let sem = DispatchSemaphore(value: 0)
Task {
    let dur = CMTimeGetSeconds(try await asset.load(.duration))
    let track = try await asset.loadTracks(withMediaType: .video).first!
    let natural = try await track.load(.naturalSize)
    let height = (width * natural.height / natural.width).rounded()

    let gen = AVAssetImageGenerator(asset: asset)
    gen.appliesPreferredTrackTransform = true
    gen.requestedTimeToleranceBefore = .zero
    gen.requestedTimeToleranceAfter = .zero
    gen.maximumSize = CGSize(width: width, height: height)

    let n = Int((dur * fps).rounded())
    for i in 0..<n {
        let t = CMTime(seconds: Double(i) / fps, preferredTimescale: 600)
        let cg = try gen.copyCGImage(at: t, actualTime: nil)
        let rep = NSBitmapImageRep(cgImage: cg)
        rep.size = NSSize(width: width, height: height)
        let data = rep.representation(using: .png, properties: [:])!
        let name = String(format: "frame%03d.png", i)
        try data.write(to: URL(fileURLWithPath: outDir).appendingPathComponent(name))
    }
    print("decoded \(n) frames at \(Int(width))x\(Int(height))")
    sem.signal()
}
sem.wait()
SWIFT

swift "$TMP/extract.swift" "$CLIP" "$TMP/png" "$FPS" "$WIDTH" 2>/dev/null

rm -rf "$OUT"
mkdir -p "$OUT"
python3 - "$TMP/png" "$OUT" "$QUALITY" <<'PY'
from PIL import Image
import pathlib, sys

src, dst, quality = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), int(sys.argv[3])
frames = sorted(src.glob("frame*.png"))
for p in frames:
    Image.open(p).convert("RGB").save(dst / f"{p.stem}.webp", "WEBP", quality=quality, method=6)
print(f"wrote {len(frames)} webp frames -> {dst}")
print(f"set LOGIN_BG_FRAMES = {len(frames)} in screens.rs")
PY

du -sh "$OUT"
