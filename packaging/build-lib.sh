#!/usr/bin/env bash
# Build a Linux/macOS bundle of Nucleus in embedded (shared-library) mode:
#   libnucleus.{so,dylib} + C header + C# P/Invoke binding (+ ONNX Runtime shared
# library if it was linked dynamically). The non-Windows counterpart of
# packaging/build-dll.ps1.
#
#   packaging/build-lib.sh [VERSION] [OUTDIR]
#
# Produces  <OUTDIR>/nucleus-lib-<VERSION>-<os>-<arch>.tar.gz
set -euo pipefail

VERSION="${1:-0.1.0}"
OUTDIR="${2:-dist}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export CARGO_INCREMENTAL=0

case "$(uname -s)" in
  Linux)  os=linux;  libname=libnucleus.so ;;
  Darwin) os=macos;  libname=libnucleus.dylib ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64)        arch=x64 ;;
  arm64|aarch64) arch=arm64 ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

echo "Building nucleus-ffi (release) for ${os}-${arch}..."
cargo build --release -p nucleus-ffi

lib="target/release/${libname}"
[ -f "$lib" ] || { echo "missing ${lib}" >&2; exit 1; }

name="nucleus-lib-${VERSION}-${os}-${arch}"
stage="${OUTDIR}/${name}"
rm -rf "$stage"
mkdir -p "$stage"

cp "$lib" "$stage/"

# Bundle the ONNX Runtime shared library only if it was linked dynamically (i.e. a
# shared lib exists in the build tree). A static link leaves nothing to ship — the
# engine is then self-contained, like the Windows DLL. (Kept bash-3.2 friendly for
# the stock macOS shell: no mapfile / associative arrays.)
ort_found=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  cp -f "$f" "$stage/"
  ort_found=1
done < <(find target/release -name 'libonnxruntime*' 2>/dev/null)
if [ "$ort_found" -eq 1 ]; then
  echo "Bundled ONNX Runtime shared library."
else
  echo "No separate ONNX Runtime library found (statically linked or cached elsewhere)."
fi

# C header + docs.
cp crates/ffi/include/nucleus.h "$stage/"
cp packaging/dll-README.md "$stage/README.md"

# C# P/Invoke binding (source) — consumers add it as a project reference.
mkdir -p "$stage/csharp"
cp clients/csharp/Nucleus.Native/*.cs "$stage/csharp/"
cp clients/csharp/Nucleus.Native/Nucleus.Native.csproj "$stage/csharp/"

tarball="${OUTDIR}/${name}.tar.gz"
rm -f "$tarball"
tar -czf "$tarball" -C "$OUTDIR" "$name"
echo "Bundle: ${tarball}"
