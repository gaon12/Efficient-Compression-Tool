Efficient Compression Tool — C/C++
==================================

Efficient Compression Tool (or ECT) is a C++ file optimizer.
It supports PNG, JPEG, WebP (non-animated lossless optimization), GZIP and ZIP files.
**This branch (`cpp`) contains the original C/C++ implementation**, which
gives the strongest compression (zopfli, optipng, mozjpeg, libwebp).

## Implementations and branches

ECT is maintained as three independent implementations, one per branch.
All of them share the same command line interface.

| Branch | Implementation | Focus |
| ------ | -------------- | ----- |
| [`cpp`](../../tree/cpp)   | C/C++ (this branch) | Maximum compression: zopfli, optipng, mozjpeg, libwebp |
| [`go`](../../tree/go)     | Go | Pure Go library + CLI, no cgo |
| [`rust`](../../tree/rust) | Rust | Pure Rust library + CLI, no C toolchain |

`master` only holds the overview and the cross-implementation benchmarks.

## Behavior Notes
- Type detection uses file signatures (magic bytes) before file extensions.
- ZIP stored entries are recompressed with Deflate by default if this makes output smaller.
- `--strict` keeps ZIP stored entries as stored (no stored->deflate conversion).
- Zip64 single-disk archives are supported for reading and writing.
- Multi-disk ZIP/Zip64 archives are not supported and fail safely.
- Animated WebP files are skipped (not treated as an error).

## CLI Notes
- `--disable-webp`: disable WebP optimization.
- `--`: treat all following arguments as positional paths, even if they start with `-`.
- Run `ect -help` for the complete list of options.

Performance (v0.9.7)
-------------------------
Benchmarked on 2026-04-21.

- OS: Windows 11
- CPU: Intel Core i3-8130U (2C/4T)
- Compiler: MSVC (Release)
- Input: `enwik8` (100,000,000 bytes), `-gzip` mode
- Notes: each level was run once on the same machine

|  Compressor    |  File Size   | Time       |
|  ----------    |  -----       | ---------- |
|  ECT -1        |  36,493,273  |      5.1s  |
|  ECT -3        |  35,014,572  |     32.6s  |
|  ECT -5        |  34,942,674  |     54.4s  |
|  ECT -7        |  34,942,246  |   2m 12.3s |
|  ECT -9        |  34,937,520  |   4m 46.7s |

Comparisons with the Go and Rust implementations on Windows, Linux and
macOS are published on the `master` branch.

## Building
To build ECT, you need to recursively clone it, just downloading isn’t enough:

```bash
git clone --recursive -b cpp https://github.com/fhanau/Efficient-Compression-Tool.git
```

You also need CMake, a C/C++17 compiler and `nasm` (used by mozjpeg's SIMD
code). libwebp is optional: when its development files are found, lossless
WebP optimization is enabled. No system zlib is required; the bundled,
optimized zlib is used everywhere (including by libpng).

### Command line (Linux, macOS, MinGW)
```bash
cmake -S src -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build --parallel
./build/ect -help
```

### Visual Studio (MSVC)
Make sure `nasm.exe` is on `PATH`, then:
```bash
cmake -S src -B build -A x64
cmake --build build --config Release --parallel
build\Release\ect.exe -help
```
Add `-DCMAKE_POLICY_DEFAULT_CMP0091=NEW -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`
to the first command to link the C runtime statically (no Visual C++
redistributable needed).

### CMake options
- `-DECT_MULTITHREADING=OFF`: Turn off multithreading support
- `-DECT_WEBP_SUPPORT=OFF`: Never link libwebp
- `-DWITH_SIMD=OFF`: Build mozjpeg without assembly (no NASM needed, slower JPEG processing)
- `-DECT_X86_64_V3=ON`: Optimize for x86-64-v3 CPUs (AVX2; `-march=x86-64-v3`, or `/arch:AVX2` with MSVC). The output is identical and `-9` was ~30% faster for PNGs and ~10% faster for gzip on an Intel Core Ultra 5 325, but the binary does not run on CPUs older than Intel Haswell / AMD Excavator, so release binaries are built without it.

### With Xcode
You can use cmake to generate an Xcode project.  Just add `-G Xcode` to the cmake configure command:
```bash
cmake -S src -B build -G Xcode
```
You will run into a slight issue in that Xcode doesn't know how to compile some of the asm files within mozjpeg.  To fix this, locate your copy of `nasm` (`/usr/local/bin/nasm` in the example) navigate to the Build Rules of the `simd` target, and add a custom rule to process source files matching `*.asm` with the following script:
```sh
/usr/local/bin/nasm "-I${PROJECT_DIR}/mozjpeg" -DMACHO -D__x86_64__ "-I${PROJECT_DIR}/mozjpeg/simd/nasm/" "-I${PROJECT_DIR}/mozjpeg/simd/x86_64/" -f macho64 -o "${BUILT_PRODUCTS_DIR}/x86_64/${INPUT_FILE_BASE}.o" "${INPUT_FILE_PATH}"
```
and set `$(BUILT_PRODUCTS_DIR)/x86_64/${INPUT_FILE_BASE}.o` as the output files.

If you are using Xcode for development and do not need maximum speed, you can also disable the asm files by adding `-DWITH_SIMD=OFF` to the cmake.

## Releases
C/C++ releases are tagged `cpp-vX.Y.Z`. Each release contains
`ect-cpp_<tag>_<os>_<arch>` archives (Linux amd64/arm64, macOS arm64,
Windows amd64 with a static runtime) and a `checksums.txt` with SHA-256
sums. Release binaries are built without WebP support so they have no
runtime dependency on libwebp.
