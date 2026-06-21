#!/usr/bin/env bash
# Build a self-contained Linux bundle of nucleus-server (binary + ONNX Runtime).
#   packaging/build-release.sh [version] [outdir]
set -euo pipefail

VERSION="${1:-0.1.0}"
OUT="${2:-dist}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export CARGO_INCREMENTAL=0

echo "Building nucleus-server (release)..."
cargo build --release -p nucleus-server

STAGE="$OUT/nucleus-$VERSION-linux-x64"
mkdir -p "$STAGE"
cp target/release/nucleus-server "$STAGE/"

# ONNX Runtime shared library that `ort` downloaded during the build.
found=$(find target/release -maxdepth 1 -name 'libonnxruntime*.so*' -print -quit || true)
if [ -z "$found" ]; then
  echo "WARNING: no libonnxruntime*.so found; the bundle may not run." >&2
else
  find target/release -maxdepth 1 -name 'libonnxruntime*.so*' -exec cp {} "$STAGE/" \;
fi

cp packaging/README.md "$STAGE/README.md"
cp packaging/nucleus.service "$STAGE/"

tar -C "$OUT" -czf "$OUT/nucleus-$VERSION-linux-x64.tar.gz" "nucleus-$VERSION-linux-x64"
echo "Bundle: $OUT/nucleus-$VERSION-linux-x64.tar.gz"
