package ect

import (
	"compress/gzip"
	"errors"
	"io"
	"os"
	"path/filepath"
)

func optimizeExistingGzip(path string, info os.FileInfo, cfg Config) (before, after int64, changed bool, err error) {
	in, err := os.Open(path)
	if err != nil {
		return 0, 0, false, err
	}
	defer in.Close()

	before = info.Size()
	if before == 0 {
		return 0, 0, false, errors.New("compression of empty files is not supported")
	}

	head := make([]byte, 10)
	if _, err := io.ReadFull(in, head); err != nil {
		return 0, 0, false, errors.New("invalid gzip header")
	}
	flags := head[3]
	if cfg.Strict && (flags&0x04 != 0 || flags&0x10 != 0) {
		return 0, 0, false, errors.New("file includes extra field or comment, can't be optimized in strict mode")
	}

	if _, err := in.Seek(0, io.SeekStart); err != nil {
		return 0, 0, false, err
	}

	zr, err := gzip.NewReader(in)
	if err != nil {
		return 0, 0, false, err
	}
	header := zr.Header

	temp, err := os.CreateTemp(filepath.Dir(path), ".ect-gzip-*")
	if err != nil {
		_ = zr.Close()
		return 0, 0, false, err
	}
	tempName := temp.Name()

	cleanup := func() {
		_ = temp.Close()
		_ = os.Remove(tempName)
	}

	zw, err := gzip.NewWriterLevel(temp, levelToGzip(cfg.Level))
	if err != nil {
		_ = zr.Close()
		cleanup()
		return 0, 0, false, err
	}

	zw.Name = header.Name
	zw.Comment = header.Comment
	zw.ModTime = header.ModTime
	zw.Extra = append([]byte(nil), header.Extra...)

	_, copyErr := io.Copy(zw, zr)
	closeReadErr := zr.Close()
	closeWriteErr := zw.Close()
	if copyErr != nil {
		cleanup()
		return 0, 0, false, copyErr
	}
	if closeReadErr != nil {
		cleanup()
		return 0, 0, false, closeReadErr
	}
	if closeWriteErr != nil {
		cleanup()
		return 0, 0, false, closeWriteErr
	}
	if err := temp.Close(); err != nil {
		_ = os.Remove(tempName)
		return 0, 0, false, err
	}

	newInfo, err := os.Stat(tempName)
	if err != nil {
		_ = os.Remove(tempName)
		return 0, 0, false, err
	}
	after = newInfo.Size()

	if after >= before {
		_ = os.Remove(tempName)
		return before, before, false, nil
	}

	if err := replaceWithFile(path, tempName, info, cfg.KeepModTime); err != nil {
		_ = os.Remove(tempName)
		return 0, 0, false, err
	}

	return before, after, true, nil
}

func compressFileToGzip(path string, info os.FileInfo, cfg Config) (outSize int64, err error) {
	if info.Size() == 0 {
		return 0, errors.New("compression of empty files is not supported")
	}

	outPath := path + ".gz"
	if _, err := os.Stat(outPath); err == nil {
		return 0, errors.New("compressed file already exists")
	}

	in, err := os.Open(path)
	if err != nil {
		return 0, err
	}
	defer in.Close()

	out, err := os.Create(outPath)
	if err != nil {
		return 0, err
	}
	defer func() {
		if err != nil {
			_ = os.Remove(outPath)
		}
	}()

	zw, err := gzip.NewWriterLevel(out, levelToGzip(cfg.Level))
	if err != nil {
		_ = out.Close()
		return 0, err
	}
	zw.Name = filepath.Base(path)
	zw.ModTime = info.ModTime()

	if _, err := io.Copy(zw, in); err != nil {
		_ = zw.Close()
		_ = out.Close()
		return 0, err
	}
	if err := zw.Close(); err != nil {
		_ = out.Close()
		return 0, err
	}
	if err := out.Close(); err != nil {
		return 0, err
	}

	if err := os.Chmod(outPath, info.Mode().Perm()); err != nil {
		return 0, err
	}
	if cfg.KeepModTime {
		_ = os.Chtimes(outPath, info.ModTime(), info.ModTime())
	}

	writtenInfo, err := os.Stat(outPath)
	if err != nil {
		return 0, err
	}
	return writtenInfo.Size(), nil
}

func levelToGzip(level int) int {
	switch {
	case level <= 1:
		return gzip.BestSpeed
	case level >= 9:
		return gzip.BestCompression
	default:
		return level
	}
}

func levelToFlate(level int) int {
	switch {
	case level <= 1:
		return 1
	case level >= 9:
		return 9
	default:
		return level
	}
}
