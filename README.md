Efficient Compression Tool — Go
===============================

Efficient Compression Tool (ECT) is a lossless file optimizer for PNG, JPEG,
WebP, GZIP and ZIP files. **This branch (`go`) contains the pure Go
implementation**: no cgo, no C toolchain, trivial cross-compilation.

## Implementations and branches

ECT is maintained as three independent implementations, one per branch.
All of them share the same command line interface.

| Branch | Implementation | Focus |
| ------ | -------------- | ----- |
| [`cpp`](../../tree/cpp)   | C/C++ (CMake) | Maximum compression: zopfli, optipng, mozjpeg, libwebp |
| [`go`](../../tree/go)     | Go (this branch) | Pure Go library + CLI, no cgo |
| [`rust`](../../tree/rust) | Rust (Cargo) | Pure Rust library + CLI, no C toolchain |

`master` only holds the overview and the cross-implementation benchmarks.

## Highlights

- Importable library (`pkg/ect`) and CLI (`cmd/ect`)
- File type detection by signature (magic bytes) before extension
- Parallel processing of files (`--mt-file[=N]`)
- Output is only written when it is smaller than the input
- Safe file replacement: private temporary file + atomic rename, symlinks
  are never followed or replaced, existing `.gz` files are never overwritten

## Supported formats

| Format | Behavior |
| ------ | -------- |
| PNG    | Lossless re-encode with the Go PNG encoder at best compression (images above 256 megapixels are refused) |
| JPEG   | Removal of APPn/COM metadata segments with `-strip`; pixel data untouched |
| WebP   | Detected and parsed; animated WebP is skipped; no re-encode |
| GZIP   | Re-compress existing `.gz` (header kept); `-gzip` creates `<file>.gz` |
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
| `--mt-file[=N]` | Number of files processed in parallel (default: number of CPUs) |
| `--` | Treat all following arguments as paths |

Flags of the C/C++ version (`-progressive`, `-autorotate`, `--mt-deflate`,
`--reuse`, `--allfilters*`, `--pal_sort=N`) are accepted and ignored.

Exit codes: `0` success, `1` no input paths, `2` invalid options or at least
one file failed.

## Library usage

```go
package main

import (
    "fmt"

    "github.com/fhanau/efficient-compression-tool/pkg/ect"
)

func main() {
    cfg := ect.DefaultConfig()
    cfg.GzipMode = true
    cfg.Recursive = true

    result := ect.Process(cfg, []string{"./assets"})
    fmt.Println("processed:", result.ProcessedFiles)
    fmt.Println("saved:", result.SavedBytes)
}
```

## Build and test

Requires Go 1.22 or newer.

```bash
go build -o ect ./cmd/ect
go vet ./...
go test ./...
go test -run xxx -bench . ./pkg/ect   # benchmarks
```

Cross-compiling, e.g. for Linux on ARM64:

```bash
GOOS=linux GOARCH=arm64 CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" ./cmd/ect
```

## Releases

Go releases are tagged `go-vX.Y.Z`. Each release contains
`ect-go_<tag>_<os>_<arch>` archives for Windows, macOS and Linux (amd64 and
arm64) and a `checksums.txt` with SHA-256 sums.

## License

Apache License 2.0, see `License.txt`. Third-party notices are in
[licenses/Licenses.txt](licenses/Licenses.txt).
