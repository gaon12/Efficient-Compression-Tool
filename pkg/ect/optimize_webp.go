package ect

import (
	"bytes"
	"encoding/binary"
	"os"
)

func optimizeWEBP(path string, _ os.FileInfo, _ Config) (before, after int64, changed bool, err error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return 0, 0, false, err
	}

	before = int64(len(b))
	if isAnimatedWebP(b) {
		return before, before, false, nil
	}

	// Pure-Go lossless WebP re-encoding is intentionally skipped here.
	return before, before, false, nil
}

func isAnimatedWebP(data []byte) bool {
	if len(data) < 12 || !bytes.Equal(data[:4], []byte("RIFF")) || !bytes.Equal(data[8:12], []byte("WEBP")) {
		return false
	}

	i := 12
	for i+8 <= len(data) {
		chunkType := string(data[i : i+4])
		chunkSize := int(binary.LittleEndian.Uint32(data[i+4 : i+8]))
		chunkDataStart := i + 8
		chunkDataEnd := chunkDataStart + chunkSize
		if chunkDataEnd > len(data) {
			return false
		}
		if chunkType == "ANIM" {
			return true
		}
		i = chunkDataEnd
		if chunkSize%2 == 1 {
			i++
		}
	}
	return false
}
