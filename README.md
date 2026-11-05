Efficient Compression Tool — Rust
=================================

Efficient Compression Tool (ECT) is a lossless file optimizer for PNG, JPEG,
WebP, GZIP and ZIP files. **This branch (`rust`) contains the pure Rust
implementation**: a library crate plus the `ect` command line tool, with no
C toolchain required.

## Implementations and branches

ECT is maintained as three independent implementations, one per branch.
All of them share the same command line interface.

| Branch | Implementation | Focus |
| ------ | -------------- | ----- |
| [`cpp`](../../tree/cpp)   | C/C++ (CMake) | Maximum compression: zopfli, optipng, mozjpeg, libwebp |
| [`go`](../../tree/go)     | Go | Pure Go library + CLI, no cgo |
| [`rust`](../../tree/rust) | Rust (this branch) | Pure Rust library + CLI, no C toolchain |

`master` only holds the overview and the cross-implementation benchmarks.

## Highlights

- Library crate `ect` and an `ect` binary built from the same package
- File type detection by signature (magic bytes) before extension
- Parallel processing of files (`--mt-file[=N]`) on scoped threads
- Fast pure-Rust deflate (`flate2` with the `zlib-rs` backend)
- Each ZIP entry is compressed only once
- Output is only written when it is smaller than the input
- Safe file replacement: private temporary file (O_EXCL, mode 0600 on Unix)
  plus atomic rename; symlinks are never followed or replaced; existing
  `.gz` files are never overwritten; PNG decoding is capped at 1 GiB

## Supported formats

| Format | Behavior |
| ------ | -------- |
| PNG    | Lossless re-encode (same color type, bit depth, palette and tRNS) at high compression; APNG is left untouched |
| JPEG   | Removal of APPn/COM metadata segments with `-strip`; pixel data untouched |
| WebP   | Detected and parsed; animated WebP is skipped; no re-encode |
| GZIP   | Re-compress existing `.gz` keeping name, comment, extra field, mtime and OS; `-gzip` creates `<file>.gz` |
| ZIP    | Re-deflate entries (stored entries become deflated unless `--strict`); `-zip` creates `<path>.zip` |

## CLI

```bash
ect [options] files/folders...
```

| Option | Meaning |
| ------ | ------- |
| `-1` ... `-9` | Compression level (default `-3`) |
| `-strip` | Strip metadata where the format allows |
| `-recurse` | Recursively process directories |
| `-gzip` | Optimize `.gz` files and compress other files to `<file>.gz` |
| `-zip` | Create `<path>.zip` for every given file or directory |
| `-keep` | Keep the original modification time |
| `-quiet` | Print only errors |
| `--disable-png`, `--disable-jpg`, `--disable-webp` | Skip a format |
| `--strict` | Keep ZIP stored entries stored; refuse gzip files with extra/comment fields |
| `--mt-file[=N]` | Number of files processed in parallel (default: available parallelism) |
| `--` | Treat all following arguments as paths |

Flags of the C/C++ version (`-progressive`, `-autorotate`, `--mt-deflate`,
`--reuse`, `--allfilters*`, `--pal_sort=N`) are accepted and ignored.

Exit codes: `0` success, `1` no input paths, `2` invalid options or at least
one file failed.

## Library usage

```rust
use ect::Config;

fn main() {
    let cfg = Config {
        gzip_mode: true,
        recursive: true,
        ..Config::default()
    };

    let report = ect::process(cfg, &["./assets"]);
    println!("processed: {}", report.processed_files);
    println!("saved: {}", report.saved_bytes);
    for err in &report.errors {
        eprintln!("{}: {}", err.path.display(), err.error);
    }
}
```

## Build and test

Requires Rust 1.88 or newer.

```bash
cargo build --release            # binary: target/release/ect
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --release --example zip_bench   # ZIP optimization benchmark
```

## Releases

Rust releases are tagged `rust-vX.Y.Z`. Each release contains
`ect-rust_<tag>_<os>_<arch>` archives for Windows, macOS and Linux (amd64
and arm64) and a `checksums.txt` with SHA-256 sums.

## License

Apache License 2.0, see `License.txt`. Third-party notices are in
[licenses/Licenses.txt](licenses/Licenses.txt).
