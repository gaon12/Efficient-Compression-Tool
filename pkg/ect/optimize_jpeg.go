package ect

import (
	"encoding/binary"
	"errors"
	"os"
)

func optimizeJPEG(path string, info os.FileInfo, cfg Config) (before, after int64, changed bool, err error) {
	original, err := os.ReadFile(path)
	if err != nil {
		return 0, 0, false, err
	}

	before = int64(len(original))
	if !cfg.Strip {
		return before, before, false, nil
	}

	stripped, err := stripJPEGMetadata(original)
	if err != nil {
		return 0, 0, false, err
	}

	after = int64(len(stripped))
	if after >= before {
		return before, before, false, nil
	}

	if err := replaceWithBytes(path, stripped, info, cfg.KeepModTime); err != nil {
		return 0, 0, false, err
	}
	return before, after, true, nil
}

func stripJPEGMetadata(data []byte) ([]byte, error) {
	if len(data) < 4 || data[0] != 0xff || data[1] != 0xd8 {
		return nil, errors.New("invalid JPEG")
	}

	out := make([]byte, 0, len(data))
	out = append(out, 0xff, 0xd8)

	i := 2
	for i < len(data) {
		if data[i] != 0xff {
			return nil, errors.New("invalid JPEG marker stream")
		}

		markerStart := i
		for i < len(data) && data[i] == 0xff {
			i++
		}
		if i >= len(data) {
			break
		}

		marker := data[i]
		i++

		if marker == 0xd9 {
			out = append(out, data[markerStart:i]...)
			break
		}

		if marker == 0xda {
			if i+2 > len(data) {
				return nil, errors.New("invalid JPEG SOS segment")
			}
			segLen := int(binary.BigEndian.Uint16(data[i : i+2]))
			if segLen < 2 || i+segLen > len(data) {
				return nil, errors.New("invalid JPEG SOS length")
			}
			headerEnd := i + segLen
			out = append(out, data[markerStart:headerEnd]...)
			out = append(out, data[headerEnd:]...)
			return out, nil
		}

		if marker == 0x01 || (marker >= 0xd0 && marker <= 0xd7) {
			out = append(out, data[markerStart:i]...)
			continue
		}

		if i+2 > len(data) {
			return nil, errors.New("invalid JPEG segment")
		}
		segLen := int(binary.BigEndian.Uint16(data[i : i+2]))
		if segLen < 2 || i+segLen > len(data) {
			return nil, errors.New("invalid JPEG segment length")
		}

		segEnd := i + segLen
		isMetadata := (marker >= 0xe0 && marker <= 0xef) || marker == 0xfe
		if !isMetadata {
			out = append(out, data[markerStart:segEnd]...)
		}
		i = segEnd
	}

	if len(out) == 0 {
		return nil, errors.New("failed to parse JPEG")
	}
	return out, nil
}
