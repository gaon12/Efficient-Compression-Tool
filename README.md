Efficient Compression Tool
============================

Efficient Compression Tool (ECT) is now implemented in pure Go.
It does not wrap or call any C/C++ compressor binaries.

## Legacy C/C++ Branch
If you are looking for the legacy C/C++ implementation (improved fork version, not the original upstream), check the `enhance-ect` branch.

## Highlights
- Pure Go library + CLI split:
  - importable library: `pkg/ect`
  - executable CLI: `cmd/ect`
- No CMake, NASM, or C/C++ runtime dependency required
- Type detection uses file signatures (magic bytes) before extensions
- ZIP stored entries are recompressed with Deflate by default when beneficial
- `--strict` keeps ZIP stored entries as stored
- `--mt-file[=N]` enables multi-worker file processing
- `--`: treat all following arguments as positional paths

## Supported Formats and Behavior
- PNG: lossless recompression via Go PNG encoder
- JPEG: metadata stripping when `-strip` is set (lossless pixel data)
- WebP: detected and parsed; animated WebP is skipped
- GZIP: optimize existing `.gz` files and create `.gz` from regular files with `-gzip`
- ZIP: optimize existing archives; create new `.zip` archives with `-zip`

## CLI
```bash
ect [options] files/folders...
```

Common options:
- `-1` to `-9`: compression level (default `-3`)
- `-strip`: strip metadata where supported
- `-recurse`: recursively process directories
- `-gzip`: gzip mode (optimize `.gz`, compress non-gzip files to `.gz`)
- `-zip`: zip packaging mode (create `.zip` for each input path)
- `-keep`: keep original modification time
- `-quiet`: print only errors
- `--disable-png`, `--disable-jpg`, `--disable-webp`
- `--strict`
- `--`

Compatibility note:
- Legacy flags such as `-progressive`, `-autorotate`, `--mt-deflate`, `--reuse`, `--allfilters`, `--pal_sort` are accepted for CLI compatibility as no-op flags.
- `--mt-file` is active and controls parallel workers.

## Library Usage
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

## Build
```bash
go build ./...
```

## Run
```bash
go run ./cmd/ect -help
```
