#!/usr/bin/env bash
# Minimal end-to-end check of a freshly built ect binary.
#
#   smoke-test.sh <path-to-ect>
#
# - gzip mode: creates sample.txt.gz and verifies it decompresses to the
#   original bytes,
# - zip mode: packs a directory and verifies the archive with `unzip -t`
#   when unzip is available,
# - PNG: recompresses a PNG stored without compression and checks that it
#   shrank (needs python3 or python to generate the input).
set -euo pipefail

ect="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

# Deterministic, compressible text (~300 KB).
for i in $(seq 1 6000); do echo "line $i: Efficient Compression Tool smoke test"; done > sample.txt
cp sample.txt original.txt

"$ect" -quiet -gzip sample.txt
test -s sample.txt.gz
gzip -dc sample.txt.gz | cmp - original.txt
echo "gzip: ok ($(wc -c < original.txt) -> $(wc -c < sample.txt.gz) bytes)"

mkdir -p bundle/sub
cp original.txt bundle/a.txt
echo "small" > bundle/sub/b.txt
"$ect" -quiet -zip bundle
test -s bundle.zip
if command -v unzip > /dev/null; then
  unzip -tq bundle.zip
fi
echo "zip: ok ($(wc -c < bundle.zip) bytes)"

# Pick an interpreter that actually runs (on Windows `python3` may be a
# Microsoft Store stub that only prints an error).
py=""
for candidate in python3 python; do
  if "$candidate" -c "import zlib" > /dev/null 2>&1; then
    py="$candidate"
    break
  fi
done
if [ -z "$py" ]; then
  echo "png: skipped (no python)"
  exit 0
fi
# 64x64 RGB gradient; the IDAT stream uses stored (level 0) deflate blocks.
"$py" - <<'PY'
import struct, zlib
w = h = 64
rows = (b"\x00" + b"".join(bytes((x * 4 % 256, y * 4 % 256, 128)) for x in range(w)) for y in range(h))
raw = b"".join(rows)
def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
png = b"\x89PNG\r\n\x1a\n"
png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(raw, 0))
png += chunk(b"IEND", b"")
open("image.png", "wb").write(png)
PY
before=$(wc -c < image.png)
"$ect" -quiet image.png
after=$(wc -c < image.png)
test "$after" -lt "$before"
echo "png: ok ($before -> $after bytes)"
