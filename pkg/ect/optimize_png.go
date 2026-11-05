package ect

import (
	"bytes"
	"errors"
	"fmt"
	"image"
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

	before = int64(len(original))
	chunks, err := parsePNGChunks(original)
	if err != nil {
		return 0, 0, false, err
	}
	// APNG: image/png decodes only the default image, so re-encoding would
	// drop every animation frame. Leave animated files alone.
	if _, animated := findPNGChunk(chunks, "acTL"); animated {
		return before, before, false, nil
	}

	img, err := png.Decode(bytes.NewReader(original))
	if err != nil {
		return 0, 0, false, err
	}
	// image/png silently extends the palette with black entries when a pixel
	// indexes past the end of PLTE (an invalid file). Writing that palette
	// back would turn the error into "valid" black pixels, so refuse.
	if paletted, ok := img.(*image.Paletted); ok {
		if plte, ok := findPNGChunk(chunks, "PLTE"); ok && len(paletted.Palette) > len(plte.data)/3 {
			return 0, 0, false, errors.New("palette index out of range of the PLTE chunk")
		}
	}

	var out bytes.Buffer
	enc := png.Encoder{CompressionLevel: png.BestCompression}
	if err := enc.Encode(&out, img); err != nil {
		return 0, 0, false, err
	}
	// The encoder writes no metadata: put gAMA/iCCP/pHYs/text/... back
	// unless -strip was given.
	encoded, err := copyPNGMetadata(original, out.Bytes(), cfg.Strip)
	if err != nil {
		return 0, 0, false, err
	}

	after = int64(len(encoded))
	if after >= before {
		return before, before, false, nil
	}

	if err := replaceWithBytes(path, encoded, info, cfg.KeepModTime); err != nil {
		return 0, 0, false, err
	}
	return before, after, true, nil
}
