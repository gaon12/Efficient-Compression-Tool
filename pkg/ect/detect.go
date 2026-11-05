package ect

import (
	"bytes"
	"io"
	"os"
	"path/filepath"
	"strings"
)

func detectFileType(path string) (FileType, error) {
	f, err := os.Open(path)
	if err != nil {
		return FileUnknown, err
	}
	defer f.Close()

	buf := make([]byte, 12)
	n, err := io.ReadFull(f, buf)
	// Short files are fine: ReadFull reports io.EOF for an empty file and
	// io.ErrUnexpectedEOF for one shorter than buf. Both fall through to the
	// extension check (an empty "a.zip" is still a ZIP target, not a file to
	// pack into "a.zip.zip").
	if err != nil && err != io.ErrUnexpectedEOF && err != io.EOF {
		return FileUnknown, err
	}
	buf = buf[:n]

	if len(buf) >= 8 && bytes.Equal(buf[:8], []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'}) {
		return FilePNG, nil
	}
	if len(buf) >= 3 && buf[0] == 0xff && buf[1] == 0xd8 && buf[2] == 0xff {
		return FileJPEG, nil
	}
	if len(buf) >= 3 && buf[0] == 0x1f && buf[1] == 0x8b && buf[2] == 0x08 {
		return FileGZIP, nil
	}
	if len(buf) >= 4 && bytes.Equal(buf[:4], []byte{'P', 'K', 0x03, 0x04}) {
		return FileZIP, nil
	}
	if len(buf) >= 12 && bytes.Equal(buf[:4], []byte("RIFF")) && bytes.Equal(buf[8:12], []byte("WEBP")) {
		return FileWEBP, nil
	}
	return fallbackTypeByExtension(path), nil
}

func fallbackTypeByExtension(path string) FileType {
	ext := strings.ToLower(strings.TrimPrefix(filepath.Ext(path), "."))
	switch ext {
	case "png":
		return FilePNG
	case "jpg", "jpeg":
		return FileJPEG
	case "gz":
		return FileGZIP
	case "zip":
		return FileZIP
	case "webp":
		return FileWEBP
	default:
		return FileUnknown
	}
}
