package ect

import (
	"bytes"
	"fmt"
	"image/png"
	"os"
)

// maxPNGPixels bounds the decoded canvas (256 megapixels, i.e. up to 1 GiB
// for 8-bit RGBA and 2 GiB for 16-bit RGBA).
const maxPNGPixels = 1 << 28

func optimizePNG(path string, info os.FileInfo, cfg Config) (before, after int64, changed bool, err error) {
	original, err := os.ReadFile(path)
	if err != nil {
		return 0, 0, false, err
	}

	// Check the declared dimensions before decoding: a tiny, crafted file
	// can otherwise claim an enormous canvas and force a huge allocation.
	imgCfg, err := png.DecodeConfig(bytes.NewReader(original))
	if err != nil {
		return 0, 0, false, err
	}
	if int64(imgCfg.Width)*int64(imgCfg.Height) > maxPNGPixels {
		return 0, 0, false, fmt.Errorf("PNG dimensions %dx%d exceed the %d pixel limit", imgCfg.Width, imgCfg.Height, maxPNGPixels)
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
