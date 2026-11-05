Benchmark results
=================

All numbers were produced by `bench/bench.py` (`--levels 1,3,9 --runs 5
--max-level-runs 1`) on 2026-11-05 on two kinds of machines:

| Result file | Machine | How |
| ----------- | ------- | --- |
| `github-actions-macos.json`   | macOS 26 (Darwin 25.6), Apple M2 Pro (virtual, 5 CPUs), arm64 | `benchmark` workflow, `macos-latest` |
| `github-actions-ubuntu.json`  | Ubuntu, AMD EPYC 9V45 (4 CPUs) | `benchmark` workflow, `ubuntu-latest` |
| `github-actions-windows.json` | Windows Server 2025, AMD EPYC 9V74 (4 CPUs) | `benchmark` workflow, `windows-latest` |
| `windows-x86_64.json`         | Windows 11, Intel Core Ultra 5 325 (8 CPUs), laptop | run locally, native |
| `linux-wsl2-x86_64.json`      | WSL2 Ubuntu 26.04 on the same laptop | run locally |

Regenerate the tables below with:

    python3 bench/report.py bench/results/github-actions-*.json \
      bench/results/windows-x86_64.json bench/results/linux-wsl2-x86_64.json

Binaries:

| Implementation | Branch | Windows | Linux | macOS |
| -------------- | ------ | ------- | ----- | ----- |
| C/C++ | `cpp`  | MSVC 2022, Release, static CRT, no WebP | GCC, Release (`-O3`), no WebP | Apple Clang, Release (`-O3`), no WebP |
| Go    | `go`   | `CGO_ENABLED=0 go build -trimpath -ldflags='-s -w'` | same | same |
| Rust  | `rust` | `cargo build --release` (LTO, 1 codegen unit) | same | same |

## Key observations

- **Output size** is deterministic and identical on all five machines and
  all three operating systems (x86-64 and arm64). C/C++ always produces
  the smallest files: at the default `-3` it is 11% smaller than Rust and
  13% smaller than Go for gzip, and 4% smaller than Rust for PNG.
- **Speed**: Rust is the fastest everywhere except PNG `-9`, where it
  tries 14 filter/zlib combinations; Go does not vary PNG work by level.
  At `-3` (single worker) Rust is 24-30x faster than C/C++ for gzip and
  ZIP on the Linux and macOS runners (14-25x on Windows), and Go is
  13-15x faster (3.4x for Windows gzip). At `-9` C/C++ needs
  6-8 s for the 2.8 MB corpus versus 0.2-0.3 s for Rust.
- **macOS (Apple M2 Pro, arm64)** behaves like Linux: C/C++ gzip `-3`
  0.57 s, Go 44 ms, Rust 24 ms; PNG `-3` 0.56 s / 0.12 s / 0.10 s.
- **Windows** is the slowest platform for the same work, mostly because of
  per-process and per-file costs (process creation, NTFS, real-time
  antivirus scanning of new files): on the GitHub runners Go gzip `-3`
  took 0.21 s on Windows versus 36 ms on Linux.
- **The laptop numbers are noisy**: its power/thermal state changed
  between sessions (C/C++ gzip `-3` measured 1.19 s in the published
  WSL2 run and 0.55-0.61 s in later runs). Compare implementations within
  one table; prefer the GitHub Actions results for absolute numbers.
- **Parallelism**: `--mt-file` helps the multi-file gzip and PNG workloads.
  The ZIP workload is a single archive, which all three implementations
  process with one worker.
- Two C/C++ size differences remain uninvestigated: gzip with `--mt-file`
  (e.g. `-3`: 692,739-693,934 vs 688,154 bytes) and gzip `-1` built with
  MSVC vs GCC/Clang (729,089 vs 729,025 bytes).

## Build optimization

Are the release builds leaving speed on the table? Variants were built on
the laptop (WSL2, GCC 15.2, Go 1.26.4, Rust 1.99.1) and compared with
`bench.py --levels 3,9 --runs 7 --max-level-runs 3 --modes single`. All
variants produce byte-identical output; times are medians in seconds.

| Variant | gzip -3 | gzip -9 | zip -9 | png -3 | png -9 |
| ------- | ------- | ------- | ------ | ------ | ------ |
| C/C++ release (`-O3`)            | 0.545 | 10.14 | 9.59  | 1.144 | 10.98 |
| C/C++ + LTO                      | 0.612 | 9.32  | 10.26 | 0.958 | 10.55 |
| C/C++ `-march=x86-64-v2`         | 0.560 | 7.54  | -     | 0.530 | 4.89  |
| Go release                       | 0.040 | 0.457 | 0.841 | 0.177 | 0.167 |
| Go + PGO (`default.pgo` from the benchmark workloads) | 0.040 | 0.495 | 1.285 | 0.217 | 0.158 |
| Go `GOAMD64=v3`                  | 0.036 | 0.462 | 0.941 | 0.161 | 0.152 |
| Rust release (LTO, 1 CGU)        | 0.018 | 0.209 | 0.427 | 0.146 | 0.849 |
| Rust + `panic=abort`             | 0.018 | 0.210 | 0.321 | 0.184 | 0.892 |
| Rust `-C target-cpu=native`      | 0.018 | 0.212 | 0.362 | 0.136 | 0.914 |

(The `x86-64-v2` row comes from a second session, see the note on laptop
noise above; its baseline in that session was gzip -9 7.57 s, png -9
4.90 s.) Differences below ~15% are within the noise of this machine.

The only consistent gain was **AVX2 code generation for C/C++**. Interleaved
A/B runs of `ect -9` on the four benchmark PNGs (4 rounds each):

| C/C++ build | Time per run |
| ----------- | ------------ |
| release (`-O3`)               | 7.20-7.68 s |
| `-march=x86-64-v3` (AVX2)     | 5.00-5.37 s (~30% faster) |

gzip `-9` was ~10% faster; `-3` and lower did not change. The `cpp` branch
therefore offers it as the opt-in CMake option `-DECT_X86_64_V3=ON`
(`/arch:AVX2` with MSVC). Release binaries stay portable because such a
binary does not start on CPUs older than Intel Haswell / AMD Excavator.

Conclusions:

- **C/C++**: already `-O3`; LTO brings nothing measurable. Build with
  `-DECT_X86_64_V3=ON` (or `-march=native`) for your own machine if you
  use `-9`.
- **Go**: the default build is as good as it gets here. PGO did not help
  (the time goes into `compress/flate` and `image/png`, which PGO barely
  changes) and `GOAMD64=v3` is within noise.
- **Rust**: the release profile (LTO, `codegen-units = 1`) is already
  optimal; `panic=abort` and `target-cpu=native` are within noise.

## Full tables

### macos-latest (GitHub Actions)

- OS: Darwin 25.6.0 (arm64)
- CPU: Apple M2 Pro (Virtual), 5 logical CPUs

**gzip: compress the 11 Canterbury Corpus files to .gz** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 70 ms | 30 ms | **20 ms** | **729,025 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -1 | parallel | 25 ms | 20 ms | **10 ms** | **729,025 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -3 | single | 0.57 s | 44 ms | **24 ms** | **688,154 (24.5%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -3 | parallel | 0.30 s | 20 ms | **11 ms** | **692,739 (24.6%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -9 | single | 6.75 s | 0.52 s | **0.23 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |
| -9 | parallel | 3.28 s | 0.41 s | **0.16 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |

**zip: re-optimize a stored (uncompressed) ZIP of the corpus (`-zip`)** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 67 ms | 24 ms | **17 ms** | **729,770 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -1 | parallel | 61 ms | 27 ms | **19 ms** | **729,770 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -3 | single | 0.58 s | 38 ms | **20 ms** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -3 | parallel | 0.60 s | 38 ms | **23 ms** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -9 | single | 6.88 s | 0.58 s | **0.21 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |
| -9 | parallel | 6.75 s | 0.53 s | **0.23 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |

**png: re-optimize 4 fast-zlib PNGs** (input 1,082,696 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.18 s | 0.12 s | **32 ms** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -1 | parallel | 0.14 s | 74 ms | **23 ms** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -3 | single | 0.56 s | 0.12 s | **0.10 s** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -3 | parallel | 0.32 s | 67 ms | **58 ms** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -9 | single | 4.52 s | **0.11 s** | 0.55 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |
| -9 | parallel | 1.66 s | **78 ms** | 0.40 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |

### ubuntu-latest (GitHub Actions)

- OS: Linux 7.0.0-1017-azure (x86_64)
- CPU: AMD EPYC 9V45 96-Core Processor, 4 logical CPUs

**gzip: compress the 11 Canterbury Corpus files to .gz** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 67 ms | 26 ms | **14 ms** | **729,025 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -1 | parallel | 27 ms | 13 ms | **8 ms** | **729,025 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -3 | single | 0.49 s | 36 ms | **17 ms** | **688,154 (24.5%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -3 | parallel | 0.26 s | 19 ms | **8 ms** | **691,931 (24.6%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -9 | single | 6.43 s | 0.38 s | **0.20 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |
| -9 | parallel | 3.30 s | 0.30 s | **0.16 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |

**zip: re-optimize a stored (uncompressed) ZIP of the corpus (`-zip`)** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 65 ms | 25 ms | **15 ms** | **729,770 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -1 | parallel | 63 ms | 24 ms | **15 ms** | **729,770 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -3 | single | 0.50 s | 34 ms | **18 ms** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -3 | parallel | 0.49 s | 34 ms | **18 ms** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -9 | single | 6.24 s | 0.39 s | **0.20 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |
| -9 | parallel | 6.41 s | 0.41 s | **0.21 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |

**png: re-optimize 4 fast-zlib PNGs** (input 1,082,696 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.19 s | 99 ms | **30 ms** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -1 | parallel | 0.15 s | 68 ms | **21 ms** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -3 | single | 0.48 s | 99 ms | **85 ms** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -3 | parallel | 0.30 s | 66 ms | **59 ms** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -9 | single | 4.02 s | **93 ms** | 0.57 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |
| -9 | parallel | 1.91 s | **64 ms** | 0.42 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |

### windows-latest (GitHub Actions)

- OS: Windows 2025Server (AMD64)
- CPU: AMD EPYC 9V74 80-Core Processor, 4 logical CPUs

**gzip: compress the 11 Canterbury Corpus files to .gz** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.13 s | 0.32 s | **57 ms** | **729,089 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -1 | parallel | 61 ms | 31 ms | **24 ms** | **729,089 (25.9%)** | 864,841 (30.8%) | 816,949 (29.1%) |
| -3 | single | 0.71 s | 0.21 s | **49 ms** | **688,154 (24.5%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -3 | parallel | 0.38 s | 38 ms | **23 ms** | **691,931 (24.6%)** | 787,320 (28.0%) | 771,260 (27.4%) |
| -9 | single | 8.23 s | 0.56 s | **0.29 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |
| -9 | parallel | 4.58 s | 0.39 s | **0.19 s** | **673,320 (24.0%)** | 727,997 (25.9%) | 729,130 (25.9%) |

**zip: re-optimize a stored (uncompressed) ZIP of the corpus (`-zip`)** (input 2,810,784 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.13 s | 35 ms | **25 ms** | **729,834 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -1 | parallel | 0.13 s | 35 ms | **25 ms** | **729,834 (26.0%)** | 865,784 (30.8%) | 817,694 (29.1%) |
| -3 | single | 0.72 s | 51 ms | **29 ms** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -3 | parallel | 0.72 s | 50 ms | **29 ms** | **688,899 (24.5%)** | 788,263 (28.0%) | 772,005 (27.5%) |
| -9 | single | 8.28 s | 0.54 s | **0.26 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |
| -9 | parallel | 8.23 s | 0.55 s | **0.26 s** | **674,065 (24.0%)** | 728,940 (25.9%) | 729,875 (26.0%) |

**png: re-optimize 4 fast-zlib PNGs** (input 1,082,696 bytes; size = output bytes, % of input)

| Level | Workers | cpp time | go time | rust time | cpp size | go size | rust size |
|---|---|---|---|---|---|---|---|
| -1 | single | 0.26 s | 0.13 s | **42 ms** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -1 | parallel | 0.20 s | 89 ms | **31 ms** | 535,841 (49.5%) | **534,530 (49.4%)** | 542,777 (50.1%) |
| -3 | single | 0.67 s | 0.13 s | **0.11 s** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -3 | parallel | 0.41 s | 89 ms | **86 ms** | **498,485 (46.0%)** | 534,530 (49.4%) | 518,839 (47.9%) |
| -9 | single | 5.29 s | **0.13 s** | 0.83 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |
| -9 | parallel | 2.60 s | **89 ms** | 0.58 s | **484,206 (44.7%)** | 534,530 (49.4%) | 515,343 (47.6%) |

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
| macos-latest (GitHub Actions) | gzip | 0.57 s | 44 ms | 24 ms |
| macos-latest (GitHub Actions) | zip | 0.58 s | 38 ms | 20 ms |
| macos-latest (GitHub Actions) | png | 0.56 s | 0.12 s | 0.10 s |
| ubuntu-latest (GitHub Actions) | gzip | 0.49 s | 36 ms | 17 ms |
| ubuntu-latest (GitHub Actions) | zip | 0.50 s | 34 ms | 18 ms |
| ubuntu-latest (GitHub Actions) | png | 0.48 s | 99 ms | 85 ms |
| windows-latest (GitHub Actions) | gzip | 0.71 s | 0.21 s | 49 ms |
| windows-latest (GitHub Actions) | zip | 0.72 s | 51 ms | 29 ms |
| windows-latest (GitHub Actions) | png | 0.67 s | 0.13 s | 0.11 s |
| Windows 11 (native, MSVC build of C/C++) | gzip | 1.58 s | 0.23 s | 0.14 s |
| Windows 11 (native, MSVC build of C/C++) | zip | 1.75 s | 0.42 s | 0.12 s |
| Windows 11 (native, MSVC build of C/C++) | png | 1.25 s | 0.29 s | 0.27 s |
| Linux (WSL2 Ubuntu 26.04, GCC build of C/C++) | gzip | 1.19 s | 62 ms | 35 ms |
| Linux (WSL2 Ubuntu 26.04, GCC build of C/C++) | zip | 1.25 s | 0.11 s | 60 ms |
| Linux (WSL2 Ubuntu 26.04, GCC build of C/C++) | png | 1.41 s | 0.19 s | 0.15 s |

