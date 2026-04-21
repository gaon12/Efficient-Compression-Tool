package ect

import (
	"bytes"
	"image/png"
	"os"
)

func optimizePNG(path string, info os.FileInfo, cfg Config) (before, after int64, changed bool, err error) {
	original, err := os.ReadFile(path)
	if err != nil {
		return 0, 0, false, err
	}

	img, err := png.Decode(bytes.NewReader(original))
	if err != nil {
		return 0, 0, false, err
	}

	var out bytes.Buffer
	enc := png.Encoder{CompressionLevel: png.BestCompression}
	if err := enc.Encode(&out, img); err != nil {
		return 0, 0, false, err
	}

	before = int64(len(original))
	after = int64(out.Len())
	if after >= before {
		return before, before, false, nil
	}

	if err := replaceWithBytes(path, out.Bytes(), info, cfg.KeepModTime); err != nil {
		return 0, 0, false, err
	}
	return before, after, true, nil
}
