package ect

import (
	"archive/zip"
	"bytes"
	"compress/flate"
	"encoding/binary"
	"hash/crc32"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode/utf8"
)

// minDeflateSize is the entry size below which entries are always stored:
// for such tiny inputs the deflate framing overhead cannot pay off.
const minDeflateSize = 64

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
		Name:    src.Name,
		Comment: src.Comment,
		NonUTF8: src.NonUTF8,
		// The extended timestamp is regenerated from Modified on write
		// (by CreateHeader or prepareRawTimestamps); keeping the copied one
		// would put two 0x5455 fields into the new archive.
		Extra:    removeExtraField(src.Extra, extTimeExtraID),
		Modified: src.Modified,
	}

	if mode := src.Mode(); mode != 0 {
		hdr.SetMode(mode)
	}
	if hdr.Modified.IsZero() {
		hdr.Modified = src.ModTime()
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

	return writeZipEntry(zw, hdr, data, mayDeflate(len(data), src.Method, cfg), cfg.Level)
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
			hdr := &zip.FileHeader{Name: rel, Method: zip.Store, Modified: fakeInfo(dirPath).ModTime()}
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

	hdr, err := zip.FileInfoHeader(info)
	if err != nil {
		return 0, err
	}
	hdr.Name = filepath.ToSlash(name)

	if err := writeZipEntry(zw, hdr, data, mayDeflate(len(data), zip.Deflate, cfg), cfg.Level); err != nil {
		return 0, err
	}
	return int64(len(data)), nil
}

// mayDeflate reports whether an entry that was originally stored with
// `original` is allowed to be (re)written with Deflate.
//   - tiny entries are always stored,
//   - stored entries stay stored in strict mode,
//   - entries using other methods (bzip2, LZMA, ...) are stored as-is after
//     decompression, matching the previous behavior.
func mayDeflate(size int, original uint16, cfg Config) bool {
	if size < minDeflateSize {
		return false
	}
	switch original {
	case zip.Store:
		return !cfg.Strict
	case zip.Deflate:
		return true
	default:
		return false
	}
}

// writeZipEntry writes data as a single entry, compressing it at most once.
//
// The data is deflated into memory first; if that is smaller than the input
// the already-compressed bytes are written with CreateRaw (CRC and sizes are
// supplied by us). Otherwise the entry is stored. Previously every entry was
// compressed twice: once to measure the deflated size and once more by the
// zip.Writer itself.
func writeZipEntry(zw *zip.Writer, hdr *zip.FileHeader, data []byte, allowDeflate bool, level int) error {
	if allowDeflate {
		compressed, err := deflateBytes(data, level)
		if err != nil {
			return err
		}
		if len(compressed) < len(data) {
			hdr.Method = zip.Deflate
			hdr.CRC32 = crc32.ChecksumIEEE(data)
			hdr.CompressedSize64 = uint64(len(compressed))
			hdr.UncompressedSize64 = uint64(len(data))
			prepareRawTimestamps(hdr)
			prepareRawUTF8Flag(hdr)
			w, err := zw.CreateRaw(hdr)
			if err != nil {
				return err
			}
			_, err = w.Write(compressed)
			return err
		}
	}

	hdr.Method = zip.Store
	w, err := zw.CreateHeader(hdr)
	if err != nil {
		return err
	}
	_, err = w.Write(data)
	return err
}

// extTimeExtraID is the "extended timestamp" extra field (0x5455) that
// zip.Writer.CreateHeader adds automatically when Modified is set.
const extTimeExtraID = 0x5455

// prepareRawTimestamps reproduces the timestamp handling that CreateHeader
// performs but CreateRaw skips: it fills the MS-DOS date/time fields from
// Modified and appends an extended timestamp extra field unless one is
// already present (e.g. copied from the source archive).
func prepareRawTimestamps(hdr *zip.FileHeader) {
	if hdr.Modified.IsZero() {
		return
	}
	// CreateRaw writes these legacy fields verbatim (it does not derive them
	// from Modified the way CreateHeader does), so they must be set here.
	//lint:ignore SA1019 CreateRaw needs the MS-DOS fields filled in explicitly.
	hdr.ModifiedDate, hdr.ModifiedTime = msDosTime(hdr.Modified)
	if hasExtraField(hdr.Extra, extTimeExtraID) || hdr.Modified.Unix() < 0 || hdr.Modified.Unix() > 1<<32-1 {
		return
	}
	var field [9]byte
	binary.LittleEndian.PutUint16(field[0:], extTimeExtraID)
	binary.LittleEndian.PutUint16(field[2:], 5) // payload size
	field[4] = 1                                // flags: modification time present
	binary.LittleEndian.PutUint32(field[5:], uint32(hdr.Modified.Unix()))
	hdr.Extra = append(hdr.Extra, field[:]...)
}

// zipUTF8Flag is general purpose bit 11: name and comment are UTF-8.
const zipUTF8Flag = 0x800

// prepareRawUTF8Flag reproduces the other step CreateHeader performs but
// CreateRaw skips: marking non-ASCII UTF-8 names as UTF-8. Without the flag
// readers decode the name as CP437 ("이름" turns into mojibake).
func prepareRawUTF8Flag(hdr *zip.FileHeader) {
	if hdr.NonUTF8 {
		hdr.Flags &^= zipUTF8Flag
		return
	}
	needsUTF8 := func(s string) bool {
		for i := 0; i < len(s); i++ {
			if s[i] >= 0x80 {
				return true
			}
		}
		return false
	}
	if (needsUTF8(hdr.Name) || needsUTF8(hdr.Comment)) && utf8.ValidString(hdr.Name) && utf8.ValidString(hdr.Comment) {
		hdr.Flags |= zipUTF8Flag
	}
}

func msDosTime(t time.Time) (date, clock uint16) {
	if t.Year() < 1980 {
		t = time.Date(1980, 1, 1, 0, 0, 0, 0, t.Location())
	}
	date = uint16(t.Day() + int(t.Month())<<5 + (t.Year()-1980)<<9)
	clock = uint16(t.Second()/2 + t.Minute()<<5 + t.Hour()<<11)
	return date, clock
}

func hasExtraField(extra []byte, id uint16) bool {
	for len(extra) >= 4 {
		fieldID := binary.LittleEndian.Uint16(extra[0:])
		size := int(binary.LittleEndian.Uint16(extra[2:]))
		if fieldID == id {
			return true
		}
		if 4+size > len(extra) {
			return false
		}
		extra = extra[4+size:]
	}
	return false
}

// removeExtraField returns a copy of extra without any field with the given
// ID. Malformed trailing bytes are kept unchanged.
func removeExtraField(extra []byte, id uint16) []byte {
	out := make([]byte, 0, len(extra))
	for len(extra) >= 4 {
		size := int(binary.LittleEndian.Uint16(extra[2:]))
		if 4+size > len(extra) {
			break
		}
		if binary.LittleEndian.Uint16(extra[0:]) != id {
			out = append(out, extra[:4+size]...)
		}
		extra = extra[4+size:]
	}
	return append(out, extra...)
}

// flateWriterPools caches one pool of *flate.Writer per compression level.
// A flate.Writer allocates roughly 1 MiB of internal state, so reusing it
// across entries and files avoids a large allocation per ZIP entry.
var flateWriterPools [flate.BestCompression + 1]sync.Pool

func deflateBytes(data []byte, level int) ([]byte, error) {
	lvl := levelToFlate(level)
	var buf bytes.Buffer
	buf.Grow(len(data) / 2)

	fw, _ := flateWriterPools[lvl].Get().(*flate.Writer)
	if fw == nil {
		var err error
		if fw, err = flate.NewWriter(&buf, lvl); err != nil {
			return nil, err
		}
	} else {
		fw.Reset(&buf)
	}
	defer flateWriterPools[lvl].Put(fw)

	if _, err := fw.Write(data); err != nil {
		return nil, err
	}
	if err := fw.Close(); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}
