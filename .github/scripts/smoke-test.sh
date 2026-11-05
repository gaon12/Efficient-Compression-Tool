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
#   shrank,
# - ZIP re-optimization: rewrites a ZIP whose entries are stored
#   (uncompressed) at -1 and -3 and checks that every entry still has the
#   original content (-1 used to corrupt such archives with MSVC builds).
# - JPEG: a corrupt JPEG is left unchanged without aborting the run (the
#   next file is still optimized), and data after the EOI marker (motion
#   photos) survives optimization.
# The last three need python3 or python to generate and verify the inputs.
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
cp image.png uncompressed.png
before=$(wc -c < image.png)
"$ect" -quiet image.png
after=$(wc -c < image.png)
test "$after" -lt "$before"
echo "png: ok ($before -> $after bytes)"

# Stored -> Deflate re-optimization for both level families: -1 uses zlib,
# -2..-9 use Zopfli.
for level in 1 3; do
  "$py" - <<'PY'
import zipfile
with zipfile.ZipFile("stored.zip", "w", zipfile.ZIP_STORED) as z:
    z.write("original.txt", "a.txt")
    z.writestr("b.txt", b"small\n")
PY
  before=$(wc -c < stored.zip)
  "$ect" -quiet -$level -zip stored.zip
  after=$(wc -c < stored.zip)
  test "$after" -lt "$before"
  "$py" - <<'PY'
import zipfile
with zipfile.ZipFile("stored.zip") as z:
    assert z.testzip() is None, "CRC mismatch"
    assert z.read("a.txt") == open("original.txt", "rb").read()
    assert z.read("b.txt") == b"small\n"
PY
  echo "zip -$level re-optimize: ok ($before -> $after bytes)"
done

# A small baseline JPEG (32x32 gradient, standard Huffman tables).
"$py" - <<'PY'
import base64
jpg = base64.b64decode("/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgMCAgMDAwMEAwMEBQgFBQQEBQoHBwYIDAoMDAsKCwsNDhIQDQ4RDgsLEBYQERMUFRUVDA8XGBYUGBIUFRT/2wBDAQMEBAUEBQkFBQkUDQsNFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBT/wAARCAAgACADASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwD4T0nwp0+Suz0nwp935P0rsdJ8KdPk/Suz0nwp935P0r9bxmcb6nBw1n/w6nHaT4U6fJ+ldnpPhT7vyfpXY6T4U+78n6V2ek+FOnyfpXwuMzjfU/qvhrP/AIdTitJ8Kfd+T9K7PSfCnT5P0rsdJ8Kfd+T9K7PSfCn3fk/Svh8ZnG+p/mLw1n/w6nHaT4U+78n6V2ek+FPu/J+ldjpPhTp8n6V2ek+FPu/J+lfDYzON9T+q+Gs/+HU//9k=")
open("good.jpg", "wb").write(jpg)
open("trailer.jpg", "wb").write(jpg + b"TRAILER-DATA")
open("corrupt.jpg", "wb").write(jpg[: len(jpg) // 2])
PY
# The corrupt JPEG must stay byte-identical, and ECT must go on to
# optimize the PNG listed after it (it used to exit() on the first
# libjpeg error).
cp corrupt.jpg corrupt.orig
before=$(wc -c < uncompressed.png)
"$ect" -quiet -3 corrupt.jpg uncompressed.png > /dev/null 2>&1 || true
cmp corrupt.jpg corrupt.orig
test "$(wc -c < uncompressed.png)" -lt "$before"
before=$(wc -c < good.jpg)
"$ect" -quiet -3 good.jpg trailer.jpg
after=$(wc -c < good.jpg)
test "$after" -lt "$before"
tail -c 12 trailer.jpg | grep -q "TRAILER-DATA"
echo "jpeg: ok ($before -> $after bytes; corrupt input untouched, trailer kept)"
