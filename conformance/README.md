Conformance harness
===================

`conformance.py` checks that every ECT implementation (C/C++, Go, Rust)
is lossless and robust. It generates about 150 deterministic inputs and
runs each `ect` binary on them at levels 1, 3 and 9. Each run gets a
fresh directory and a timeout.

| Category | Examples |
| -------- | -------- |
| PNG      | every color type and bit depth, Adam7, 16-bit, palettes with tRNS, color keys, metadata chunks (gAMA, iCCP, text, private), APNG, 1x1 and 1x300 images, split IDAT |
| JPEG     | baseline/progressive, grayscale/CMYK, EXIF/ICC, restart markers, data after EOI, multi-picture (MPF) |
| WebP     | lossy, lossless, alpha, animated |
| gzip/ZIP | header fields (name, comment, extra, HCRC), multi-member, stored/deflated/data-descriptor/ZIP64 entries, UTF-8 names, comments, nested archives, duplicate names |
| corrupt  | empty, garbage, truncated and bit-flipped versions of every format, bad CRCs, out-of-range palette indices, 100000x100000 PNG headers |
| file system | read-only files, symbolic links, missing files, names with spaces/Unicode or a leading dash, wrong extensions, `-keep`, directories with and without `-recurse` |

Every run is checked for the following:

- **crashes**: signals, sanitizer reports, Go/Rust panics, data races and
  timeouts;
- **losslessness**: an optimized file must decode to exactly the same
  content as the original. That means the same pixels (RGB under fully
  transparent pixels is ignored), the same APNG frames, the same
  decompressed gzip bytes, and the same ZIP entries (nested archives and
  images are compared by content);
- **metadata**: without `-strip`, PNG color-space and text chunks, JPEG
  EXIF/ICC data and data after EOI must survive;
- **corrupt input**: it must stay byte-identical. The exceptions are
  damage that cannot be detected (pixel-identical JPEG output) and damaged
  ZIP entries that are copied verbatim;
- **growth and debris**: no file may grow, and no temporary files may be
  left behind.

Findings are written incrementally to `<out>.jsonl`, sorted into
`<out>.json`, and summarized on stdout. Each one is an `error` (a bug) or
a `warn`. A `warn` is a documented difference between the implementations,
for example "empty files are not gzipped" or "symlinks are not followed".

## Running

The harness needs Python 3.10+ and Pillow, and runs on Linux and macOS.
It uses `resource` to cap the memory of uninstrumented binaries at 2 GiB.

```bash
python3 -m pip install Pillow
python3 selftest.py   # the harness PNG decoder must agree with Pillow

python3 conformance.py \
    --impl cpp=/path/to/ect-cpp \
    --impl go=/path/to/ect-go \
    --impl rust=/path/to/ect-rust \
    --out findings.json --jobs 4
python3 group.py findings.jsonl
```

| Option | Meaning |
| ------ | ------- |
| `--impl name=path` | Binary to test (repeatable). Names starting with `cpp` also get the cases for options only the C/C++ version has. |
| `--filter REGEX`   | Only run cases whose name matches, e.g. `'^(png|corrupt-png)-'` |
| `--levels 1,3`     | Override the levels per case |
| `--jobs N`, `--timeout S` | Parallel runs and the per-run timeout (default 120 s) |

Instrumented builds are worth running too. Put a `-` in their names
(`cpp-san`, `go-race`, `rust-debug`) so they are not memory-capped:

```bash
# C/C++ with AddressSanitizer + UndefinedBehaviorSanitizer
cmake -S src -B build-san -DCMAKE_BUILD_TYPE=Debug \
  -DCMAKE_C_FLAGS="-fsanitize=address,undefined -fno-sanitize=alignment -fno-sanitize-recover=undefined" \
  -DCMAKE_CXX_FLAGS="-fsanitize=address,undefined -fno-sanitize=alignment -fno-sanitize-recover=undefined"
# Go with the race detector
go build -race -o ect-go-race ./cmd/ect
# Rust with debug assertions and overflow checks
cargo build
```

Sanitizer builds need several times more memory than release builds. On
a machine with 8 GB of RAM, use `--jobs 3` or fewer.

## Mutation fuzzing

`fuzz.py` mutates the valid inputs of a dozen cases (bit flips, byte
overwrites, truncation, duplicated and deleted ranges). It checks every
mutant like a regular case: a mutant that a reference decoder still
accepts must keep its decoded content, and any other mutant must stay
byte-identical. Mutants are reproducible from `--seed`.

```bash
python3 fuzz.py --impl cpp=/path/to/ect-cpp --impl rust=/path/to/ect-rust \
    --count 60 --seed 1 --out fuzz.json
```

The harness found the bugs fixed in `cpp-v0.9.8`, `go-v0.11.0` and
`rust-v0.1.0`. Those include lossy color-key PNGs (Rust), dropped PNG
metadata (Go, Rust) and APNG frames (Go), corrupt JPEGs aborting a whole run
(C/C++), and memory-safety bugs found by ASan/UBSan (C/C++).
