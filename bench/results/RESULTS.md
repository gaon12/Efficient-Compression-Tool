Benchmark results
=================

Measured on 2026-11-05 with `bench/bench.py` (`--levels 1,3,9 --runs 5
--max-level-runs 1`) on **one laptop** (Intel Core Ultra 5 325, 8 logical
CPUs), once natively on Windows 11 and once in WSL2 (Ubuntu 26.04) on the
same hardware. Raw data: `windows-x86_64.json`, `linux-wsl2-x86_64.json`.
Regenerate this file with:

    python3 bench/report.py bench/results/windows-x86_64.json bench/results/linux-wsl2-x86_64.json

Binaries (built from the branch heads used for these measurements):

| Implementation | Commit | Windows build | Linux build |
| -------------- | ------ | ------------- | ----------- |
| C/C++ | `cpp` 496ede5 | MSVC 2022, Release, static CRT, no WebP | GCC, Release (Ninja), no WebP |
| Go    | `go` ec4ea56  | Go 1.26.4, `-trimpath -ldflags='-s -w'` | same, CGO_ENABLED=0 |
| Rust  | `rust` c1a99a9 | Rust 1.99, release profile (LTO) | same |

Key observations:

- **Compression ratio**: C/C++ always produces the smallest files (Zopfli
  and optipng): at the default `-3` it is 11% smaller than Rust and 13%
  smaller than Go for gzip, and 4% smaller than Rust for PNG.
- **Speed**: Rust is the fastest at every level except PNG `-9`, where it
  tries 14 filter/zlib combinations; Go does not vary PNG work by level.
  At `-3` on Linux, Rust is ~34x (gzip) and ~21x (ZIP) faster than
  C/C++; at `-9` the C/C++ version needs 14 s (Linux) to 33 s (Windows)
  for the 2.8 MB corpus versus 0.3-0.7 s for Rust.
- **Windows vs Linux on the same hardware**: Go and Rust are 1.5-4x slower
  natively on Windows than under WSL2 for these small, file-heavy
  workloads, most likely because per-process and per-file costs (process
  creation, NTFS, real-time antivirus scanning of each new file) dominate
  such short runs. Windows timings
  also varied much more between runs (e.g. C/C++ gzip `-9`: 24.5 s and
  32.8 s in two sessions), so treat them as approximate.
- **Parallelism**: `--mt-file` helps the multi-file gzip and PNG workloads.
  The ZIP workload is a single archive, which all three implementations
  process with one worker, so parallel mode does not speed it up.
- Output sizes are identical across worker modes and operating systems
  with two exceptions in the C/C++ version, whose cause was not
  investigated: gzip `-3` with `--mt-file` (693,934 vs 688,154 bytes) and
  gzip `-1` built with MSVC vs GCC (729,089 vs 729,025 bytes).

### Windows 11 (native, MSVC build of C/C++)

- OS: Windows 11 (build 26200) (AMD64)
- CPU: Intel(R) Core(TM) Ultra 5 325, 8 logical CPUs

**gzip: compress the 11 Canterbury Corpus files to .gz** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.42 s | 0.25 s | **0.12 s** | **729,089 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -1 | parallel | 0.11 s | 0.13 s | **68 ms** | **729,089 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -3 | single | 1.58 s | 0.23 s | **0.14 s** | **688,154 (24.5%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -3 | parallel | 0.53 s | 0.12 s | **70 ms** | **693,934 (24.7%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -9 | single | 32.84 s | 2.35 s | **0.67 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |
| -9 | parallel | 19.18 s | 2.36 s | **0.54 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |

**zip: re-optimize a stored (uncompressed) ZIP of the corpus (`-zip`)** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.38 s | 0.20 s | **0.12 s** | **729,834 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -1 | parallel | 0.32 s | 93 ms | **79 ms** | **729,834 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -3 | single | 1.75 s | 0.42 s | **0.12 s** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -3 | parallel | 1.65 s | 0.13 s | **88 ms** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -9 | single | 23.93 s | 0.83 s | **0.40 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |
| -9 | parallel | 20.82 s | 0.89 s | **0.42 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |

**png: re-optimize 4 fast-zlib PNGs** (input 1,082,696 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.46 s | 0.27 s | **0.11 s** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -1 | parallel | 0.36 s | 0.20 s | **74 ms** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -3 | single | 1.25 s | 0.29 s | **0.27 s** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -3 | parallel | 0.80 s | 0.25 s | **0.19 s** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -9 | single | 8.27 s | **0.19 s** | 0.95 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |
| -9 | parallel | 3.71 s | **0.17 s** | 0.70 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |

### Linux (WSL2 Ubuntu 26.04, GCC build of C/C++)

- OS: Linux 6.18.33.2-microsoft-standard-WSL2 (x86_64)
- CPU: Intel(R) Core(TM) Ultra 5 325, 8 logical CPUs

**gzip: compress the 11 Canterbury Corpus files to .gz** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.12 s | 58 ms | **26 ms** | **729,025 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -1 | parallel | 48 ms | 15 ms | **10 ms** | **729,025 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -3 | single | 1.19 s | 62 ms | **35 ms** | **688,154 (24.5%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -3 | parallel | 0.54 s | 41 ms | **16 ms** | **693,934 (24.7%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -9 | single | 13.96 s | 0.63 s | **0.28 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |
| -9 | parallel | 5.58 s | 0.54 s | **0.24 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |

**zip: re-optimize a stored (uncompressed) ZIP of the corpus (`-zip`)** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.15 s | 43 ms | **28 ms** | **729,770 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -1 | parallel | 0.12 s | 38 ms | **29 ms** | **729,770 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -3 | single | 1.25 s | 0.11 s | **60 ms** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -3 | parallel | 1.06 s | 65 ms | **30 ms** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -9 | single | 13.34 s | 0.61 s | **0.27 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |
| -9 | parallel | 16.05 s | 0.73 s | **0.39 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |

**png: re-optimize 4 fast-zlib PNGs** (input 1,082,696 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.38 s | 0.24 s | **54 ms** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -1 | parallel | 0.34 s | 0.20 s | **44 ms** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -3 | single | 1.41 s | 0.19 s | **0.15 s** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -3 | parallel | 0.86 s | 0.20 s | **0.13 s** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -9 | single | 8.27 s | **0.16 s** | 0.92 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |
| -9 | parallel | 3.89 s | **0.12 s** | 0.71 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |

### Cross-OS summary (-3, single worker)

| Machine | Workload | cpp | go | rust |
|---|---|---|---|---|
| Windows 11 (native, MSVC build of C/C++) | gzip | 1.58 s | 0.23 s | 0.14 s |
| Windows 11 (native, MSVC build of C/C++) | zip | 1.75 s | 0.42 s | 0.12 s |
| Windows 11 (native, MSVC build of C/C++) | png | 1.25 s | 0.29 s | 0.27 s |
| Linux (WSL2 Ubuntu 26.04, GCC build of C/C++) | gzip | 1.19 s | 62 ms | 35 ms |
| Linux (WSL2 Ubuntu 26.04, GCC build of C/C++) | zip | 1.25 s | 0.11 s | 60 ms |
| Linux (WSL2 Ubuntu 26.04, GCC build of C/C++) | png | 1.41 s | 0.19 s | 0.15 s |

