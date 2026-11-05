Efficient Compression Tool
==========================

Efficient Compression Tool (ECT) is a file optimizer for PNG, JPEG, WebP,
GZIP and ZIP files. It rewrites files losslessly so that they become
smaller while staying fully compatible with every decoder.

## One branch per implementation

ECT exists as three independent implementations. Each one lives on its own
branch with its own source tree, build system, CI, release workflow and
documentation, and each is released separately:

| Branch | Language | Build requirements | Highlights | Release tags |
| ------ | -------- | ------------------ | ---------- | ------------ |
| [`cpp`](../../tree/cpp)   | C/C++ | CMake, C/C++17 compiler, NASM | Original implementation: zopfli, optipng, mozjpeg, libwebp. Smallest output, slowest. | `cpp-vX.Y.Z` |
| [`go`](../../tree/go)     | Go    | Go toolchain only (no cgo)     | Importable library (`pkg/ect`) + CLI, single static binary. | `go-vX.Y.Z` |
| [`rust`](../../tree/rust) | Rust  | Cargo only (no C toolchain)    | Library crate + CLI, fastest of the three. | `rust-vX.Y.Z` |

To work on one implementation, check out its branch:

```bash
git clone -b cpp --recursive https://github.com/fhanau/Efficient-Compression-Tool.git ect-cpp
git clone -b go   https://github.com/fhanau/Efficient-Compression-Tool.git ect-go
git clone -b rust https://github.com/fhanau/Efficient-Compression-Tool.git ect-rust
```

Release archives are named `ect-<implementation>_<tag>_<os>_<arch>`
(for example `ect-rust_rust-v1.0.0_linux_amd64.tar.gz`) and each release
includes a `checksums.txt` file with SHA-256 sums.

## Common command line

All implementations accept the same core options:

```
ect [options] files/folders...
```

| Option | Meaning |
| ------ | ------- |
| `-1` ... `-9` | Compression level (default `-3`). Higher is smaller and slower. |
| `-strip` | Strip metadata where supported. |
| `-recurse` | Process directories recursively. |
| `-gzip` | Compress files to `.gz` (and re-optimize existing `.gz` files). |
| `-zip` | Pack files/directories into `.zip`; existing ZIP archives are re-optimized in place. |
| `-keep` | Keep the original modification time. |
| `-quiet` | Print errors only. |
| `--strict` | Keep stored ZIP entries stored (no stored -> Deflate conversion). |
| `--mt-file[=N]` | Process files with N parallel workers. |
| `--disable-png`, `--disable-jpg`, `--disable-webp` | Skip a file type. |
| `--` | Treat all following arguments as paths. |

Each branch's README lists the options and behavior specific to that
implementation (for example the C/C++ version also supports
`--mt-deflate`, `--allfilters` and `-progressive` JPEG encoding).

## What is on this branch

`master` holds no implementation code. It contains:

- this overview,
- [`bench/`](bench): a cross-implementation benchmark (`bench.py`) and a
  Markdown report generator (`report.py`),
- the `benchmark` GitHub Actions workflow, which builds all three branches
  on Linux, macOS and Windows and benchmarks them on identical inputs,
- [`doc/Manual.docx`](doc/Manual.docx), the original user manual.

## Which one should I use?

Measured with `bench/bench.py` by the `benchmark` workflow on GitHub
Actions runners, default level `-3`, one file at a time. Output sizes are
identical on every OS:

| Workload | C/C++ size | Go size | Rust size |
| -------- | ---------- | ------- | --------- |
| gzip, Canterbury Corpus (2.81 MB) | **688,154 B** | 787,320 B | 771,260 B |
| ZIP re-optimization (2.81 MB stored) | **688,899 B** | 788,263 B | 772,005 B |
| PNG re-optimization (1.08 MB) | **498,485 B** | 534,530 B | 518,839 B |

Wall time, C/C++ / Go / Rust:

| Workload | Linux (x86-64) | macOS (Apple M2 Pro) | Windows (x86-64) |
| -------- | -------------- | -------------------- | ---------------- |
| gzip | 0.49 s / 36 ms / **17 ms** | 0.57 s / 44 ms / **24 ms** | 0.71 s / 0.21 s / **49 ms** |
| ZIP  | 0.50 s / 34 ms / **18 ms** | 0.58 s / 38 ms / **20 ms** | 0.72 s / 51 ms / **29 ms** |
| PNG  | 0.48 s / 99 ms / **85 ms** | 0.56 s / 0.12 s / **0.10 s** | 0.67 s / 0.13 s / **0.11 s** |

- Smallest files, time is no concern: **C/C++** (`cpp`). If you use `-9`
  on a CPU with AVX2, build it with `-DECT_X86_64_V3=ON` (~30% faster).
- Fast optimization with good results, simple builds: **Rust** (`rust`).
- Embedding in a Go program, or a dependency-free Go toolchain build:
  **Go** (`go`).

Full tables for levels 1, 3 and 9, single and parallel workers, results
from a local laptop, and a comparison of compiler/build optimizations are
in [bench/results/RESULTS.md](bench/results/RESULTS.md).

## License

ECT is licensed under the Apache License 2.0 (see `License.txt`). The
bundled third-party components and their licenses are listed in each
implementation branch under `licenses/`.
