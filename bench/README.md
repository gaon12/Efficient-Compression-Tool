Cross-implementation benchmark
==============================

`bench.py` runs the C/C++, Go and Rust builds of ECT on identical inputs on
the current machine and records wall time and output size. `report.py`
turns one or more result files into Markdown tables. Both scripts only need
Python 3.9+ and its standard library.

## Workloads

| Name | Command | Input |
| ---- | ------- | ----- |
| `gzip` | `ect -L -gzip <files>` | The 11 files of the [Canterbury Corpus](https://corpus.canterbury.ac.nz/descriptions/) (2,810,784 bytes: English text, HTML, C and Lisp source, a spreadsheet, a fax image, a SPARC binary). |
| `zip`  | `ect -L -zip corpus.zip` | The same files in a ZIP with stored (uncompressed) entries; re-optimized in place. |
| `png`  | `ect -L <files>` | Four generated 8-bit PNGs written with filter 0 and zlib level 1 (photo-like RGB, screenshot-like RGB, RGBA icon, noisy grayscale pattern; 1,082,696 bytes). |

Every result is verified before it is recorded: each `.gz` must decompress
to its source file, each ZIP entry must extract to the original bytes (with
a valid CRC) and every PNG must still be a PNG. The corpus archive is
checked against a pinned SHA-256 and the PNGs are generated with a fixed
PRNG seed, so all machines process exactly the same bytes.

Each workload runs at the requested levels in two worker modes:

- `single`: one file at a time (`--mt-file=1` for Go and Rust, which
  default to all CPUs; the C/C++ default),
- `parallel`: `--mt-file=<logical CPUs>` for every implementation.

The reported time is the median of `--runs` runs (`--max-level-runs` for
`-9`, which takes the C/C++ version tens of seconds). Times include process
start-up and file I/O, as a user would observe them. Each run gets a
fresh temporary directory.

## Running

```bash
curl -LO https://corpus.canterbury.ac.nz/resources/cantrbry.zip
python3 bench/bench.py \
  --impl cpp=path/to/cpp/ect --impl go=path/to/go/ect --impl rust=path/to/rust/ect \
  --corpus cantrbry.zip --levels 1,3,9 --runs 5 --label "my machine" --out results.json
python3 bench/report.py results.json > RESULTS.md
```

Pass several result files to `report.py` to get a cross-OS comparison.

The `benchmark` workflow (`.github/workflows/benchmark.yml`, manual
trigger) builds the `cpp`, `go` and `rust` branches on Linux, macOS and
Windows runners, runs this benchmark on each and publishes the combined
report as the job summary and as an artifact. Shared CI runners are noisy,
so compare implementations within one runner rather than across runs.

## Interpreting the results

- Output size is deterministic; timings vary between runs, especially on
  Windows, where process creation and real-time antivirus scanning of the
  newly written files add a large, variable overhead.
- The C/C++ version spends far more CPU time (Zopfli, optipng trials) to
  get the smallest output. Go and Rust use zlib-class Deflate encoders and
  trade some compression for speed.
