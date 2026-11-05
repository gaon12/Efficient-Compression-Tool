package ect

import (
	"archive/zip"
	"compress/flate"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

func optimizeExistingZIP(path string, info os.FileInfo, cfg Config) (before, after int64, changed bool, err error) {
	before = info.Size()

	zr, err := zip.OpenReader(path)
	if err != nil {
		return 0, 0, false, err
	}
	defer zr.Close()

	temp, err := os.CreateTemp(filepath.Dir(path), ".ect-zip-*")
	if err != nil {
		return 0, 0, false, err
	}
	tempName := temp.Name()

	cleanup := func() {
		_ = temp.Close()
		_ = os.Remove(tempName)
	}

	zw := zip.NewWriter(temp)
	zw.RegisterCompressor(zip.Deflate, func(w io.Writer) (io.WriteCloser, error) {
		return flate.NewWriter(w, levelToFlate(cfg.Level))
	})
	_ = zw.SetComment(zr.Comment)

	for _, f := range zr.File {
		if err := copyZipEntry(zw, f, cfg); err != nil {
			_ = zw.Close()
			cleanup()
			return 0, 0, false, err
		}
	}

	if err := zw.Close(); err != nil {
		cleanup()
		return 0, 0, false, err
	}
	if err := temp.Close(); err != nil {
		_ = os.Remove(tempName)
		return 0, 0, false, err
	}
	// Release the source archive before the rename below: Windows refuses
	// to replace a file that is still open ("Access is denied").
	_ = zr.Close()

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

func copyZipEntry(zw *zip.Writer, src *zip.File, cfg Config) error {
	hdr := &zip.FileHeader{
		Name:     src.Name,
		Comment:  src.Comment,
		NonUTF8:  src.NonUTF8,
		Extra:    append([]byte(nil), src.Extra...),
		Method:   src.Method,
		Modified: src.Modified,
	}

	if mode := src.Mode(); mode != 0 {
		hdr.SetMode(mode)
	}
	if hdr.Modified.IsZero() {
		hdr.SetModTime(src.ModTime())
	}

	if src.FileInfo().IsDir() {
		hdr.Method = zip.Store
		_, err := zw.CreateHeader(hdr)
		return err
	}

	rc, err := src.Open()
	if err != nil {
		return err
	}
	data, err := io.ReadAll(rc)
	_ = rc.Close()
	if err != nil {
		return err
	}

	hdr.Method, err = chooseZipMethod(data, src.Method, cfg)
	if err != nil {
		return err
	}

	w, err := zw.CreateHeader(hdr)
	if err != nil {
		return err
	}
	_, err = w.Write(data)
	return err
}

func createZipArchive(input string, cfg Config) (inputBytes int64, outputBytes int64, err error) {
	info, err := os.Stat(input)
	if err != nil {
		return 0, 0, err
	}

	cleanInput := filepath.Clean(input)
	outPath := cleanInput + ".zip"
	if _, err := os.Stat(outPath); err == nil {
		return 0, 0, fs.ErrExist
	}

	temp, err := os.CreateTemp(filepath.Dir(outPath), ".ect-pack-*")
	if err != nil {
		return 0, 0, err
	}
	tempName := temp.Name()

	cleanup := func() {
		_ = temp.Close()
		_ = os.Remove(tempName)
	}

	zw := zip.NewWriter(temp)
	zw.RegisterCompressor(zip.Deflate, func(w io.Writer) (io.WriteCloser, error) {
		return flate.NewWriter(w, levelToFlate(cfg.Level))
	})

	if info.IsDir() {
		dirs, files, err := collectDirectoryEntries(cleanInput)
		if err != nil {
			_ = zw.Close()
			cleanup()
			return 0, 0, err
		}
		parent := filepath.Dir(cleanInput)

		for _, dirPath := range dirs {
			rel, relErr := filepath.Rel(parent, dirPath)
			if relErr != nil {
				_ = zw.Close()
				cleanup()
				return 0, 0, relErr
			}
			rel = filepath.ToSlash(rel)
			if !strings.HasSuffix(rel, "/") {
				rel += "/"
			}
			hdr := &zip.FileHeader{Name: rel, Method: zip.Store}
			hdr.SetModTime(fakeInfo(dirPath).ModTime())
			if _, err := zw.CreateHeader(hdr); err != nil {
				_ = zw.Close()
				cleanup()
				return 0, 0, err
			}
		}

		for _, filePath := range files {
			rel, relErr := filepath.Rel(parent, filePath)
			if relErr != nil {
				_ = zw.Close()
				cleanup()
				return 0, 0, relErr
			}
			rel = filepath.ToSlash(rel)
			wrote, addErr := addFileToZipWriter(zw, filePath, rel, fakeInfo(filePath), cfg)
			if addErr != nil {
				_ = zw.Close()
				cleanup()
				return 0, 0, addErr
			}
			inputBytes += wrote
		}
	} else {
		name := filepath.Base(cleanInput)
		wrote, addErr := addFileToZipWriter(zw, cleanInput, name, info, cfg)
		if addErr != nil {
			_ = zw.Close()
			cleanup()
			return 0, 0, addErr
		}
		inputBytes += wrote
	}

	if err := zw.Close(); err != nil {
		cleanup()
		return 0, 0, err
	}
	if err := temp.Close(); err != nil {
		_ = os.Remove(tempName)
		return 0, 0, err
	}

	outInfo, err := os.Stat(tempName)
	if err != nil {
		_ = os.Remove(tempName)
		return 0, 0, err
	}
	outputBytes = outInfo.Size()

	if err := replaceWithFile(outPath, tempName, info, cfg.KeepModTime); err != nil {
		_ = os.Remove(tempName)
		return 0, 0, err
	}

	return inputBytes, outputBytes, nil
}

func collectDirectoryEntries(root string) (dirs []string, files []string, err error) {
	err = filepath.WalkDir(root, func(path string, d fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if d.Type()&fs.ModeSymlink != 0 {
			// Never follow links while packing: they could point outside the
			// directory being archived or form a cycle.
			return nil
		}
		if d.IsDir() {
			if path != root {
				dirs = append(dirs, path)
			}
			return nil
		}
		files = append(files, path)
		return nil
	})
	if err != nil {
		return nil, nil, err
	}
	sort.Strings(dirs)
	sort.Strings(files)
	return dirs, files, nil
}

func addFileToZipWriter(zw *zip.Writer, srcPath, name string, info os.FileInfo, cfg Config) (int64, error) {
	data, err := os.ReadFile(srcPath)
	if err != nil {
		return 0, err
	}

	method, err := chooseZipMethod(data, zip.Deflate, cfg)
	if err != nil {
		return 0, err
	}

	hdr, err := zip.FileInfoHeader(info)
	if err != nil {
		return 0, err
	}
	hdr.Name = filepath.ToSlash(name)
	hdr.Method = method

	w, err := zw.CreateHeader(hdr)
	if err != nil {
		return 0, err
	}
	if _, err := w.Write(data); err != nil {
		return 0, err
	}
	return int64(len(data)), nil
}

func chooseZipMethod(data []byte, original uint16, cfg Config) (uint16, error) {
	if len(data) < 64 {
		return zip.Store, nil
	}

	switch original {
	case zip.Store:
		if cfg.Strict {
			return zip.Store, nil
		}
		deflated, err := deflatedSize(data, cfg.Level)
		if err != nil {
			return 0, err
		}
		if deflated < len(data) {
			return zip.Deflate, nil
		}
		return zip.Store, nil
	case zip.Deflate:
		deflated, err := deflatedSize(data, cfg.Level)
		if err != nil {
			return 0, err
		}
		if deflated < len(data) {
			return zip.Deflate, nil
		}
		return zip.Store, nil
	default:
		return zip.Store, nil
	}
}

type countingWriter struct {
	n int
}

func (w *countingWriter) Write(p []byte) (int, error) {
	w.n += len(p)
	return len(p), nil
}

func deflatedSize(data []byte, level int) (int, error) {
	cw := &countingWriter{}
	zw, err := flate.NewWriter(cw, levelToFlate(level))
	if err != nil {
		return 0, err
	}
	if _, err := zw.Write(data); err != nil {
		_ = zw.Close()
		return 0, err
	}
	if err := zw.Close(); err != nil {
		return 0, err
	}
	return cw.n, nil
}
