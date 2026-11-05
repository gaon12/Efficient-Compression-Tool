package ect

import (
	"archive/zip"
	"bytes"
	"compress/gzip"
	"encoding/binary"
	"hash/crc32"
	"image"
	"image/color"
	"image/png"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// quietConfig returns the default configuration without log output.
func quietConfig() Config {
	cfg := DefaultConfig()
	cfg.Quiet = true
	return cfg
}

func compressibleText() []byte {
	return []byte(strings.Repeat("Efficient Compression Tool test payload. ", 500))
}

func TestDetectFileTypeBySignature(t *testing.T) {
	dir := t.TempDir()
	cases := map[string]struct {
		name string
		data []byte
		want FileType
	}{
		"png":       {"x.bin", []byte("\x89PNG\r\n\x1a\n...."), FilePNG},
		"jpeg":      {"x.bin", []byte{0xff, 0xd8, 0xff, 0xe0}, FileJPEG},
		"gzip":      {"x.bin", []byte{0x1f, 0x8b, 0x08, 0x00}, FileGZIP},
		"zip":       {"x.bin", []byte("PK\x03\x04"), FileZIP},
		"webp":      {"x.bin", []byte("RIFF\x00\x00\x00\x00WEBP"), FileWEBP},
		"extension": {"photo.JPEG", []byte("not really"), FileJPEG},
		"unknown":   {"notes.txt", []byte("hello"), FileUnknown},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			path := filepath.Join(dir, name+"-"+tc.name)
			if err := os.WriteFile(path, tc.data, 0o644); err != nil {
				t.Fatal(err)
			}
			got, err := detectFileType(path)
			if err != nil {
				t.Fatal(err)
			}
			if got != tc.want {
				t.Fatalf("got %v, want %v", got, tc.want)
			}
		})
	}
}

func TestStripJPEGMetadata(t *testing.T) {
	in := []byte{
		0xff, 0xd8, // SOI
		0xff, 0xe1, 0x00, 0x04, 0xaa, 0xbb, // APP1 (metadata)
		0xff, 0xfe, 0x00, 0x03, 0x41, // COM (metadata)
		0xff, 0xdb, 0x00, 0x03, 0x01, // DQT (kept)
		0xff, 0xda, 0x00, 0x02, 0x11, 0x22, // SOS + entropy-coded data
		0xff, 0xd9, // EOI
	}
	want := []byte{0xff, 0xd8, 0xff, 0xdb, 0x00, 0x03, 0x01, 0xff, 0xda, 0x00, 0x02, 0x11, 0x22, 0xff, 0xd9}
	got, err := stripJPEGMetadata(in)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Fatalf("got % x, want % x", got, want)
	}
	if _, err := stripJPEGMetadata([]byte("nope")); err == nil {
		t.Fatal("expected an error for invalid input")
	}
}

func TestIsAnimatedWebP(t *testing.T) {
	data := []byte("RIFF\x00\x00\x00\x00WEBP")
	data = append(data, []byte("VP8X\x0a\x00\x00\x00")...)
	data = append(data, make([]byte, 10)...)
	if isAnimatedWebP(data) {
		t.Fatal("static WebP reported as animated")
	}
	data = append(data, []byte("ANIM\x06\x00\x00\x00")...)
	data = append(data, make([]byte, 6)...)
	if !isAnimatedWebP(data) {
		t.Fatal("animated WebP not detected")
	}
}

func TestGzipModeCreatesArchiveAndRefusesOverwrite(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "data.txt")
	if err := os.WriteFile(src, compressibleText(), 0o644); err != nil {
		t.Fatal(err)
	}

	cfg := quietConfig()
	cfg.GzipMode = true
	res := Process(cfg, []string{src})
	if res.HasErrors() {
		t.Fatalf("unexpected errors: %v", res.Errors)
	}
	if res.SavedBytes <= 0 {
		t.Fatalf("expected savings, got %d", res.SavedBytes)
	}

	f, err := os.Open(src + ".gz")
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	zr, err := gzip.NewReader(f)
	if err != nil {
		t.Fatal(err)
	}
	got, err := io.ReadAll(zr)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, compressibleText()) {
		t.Fatal("gzip round-trip mismatch")
	}

	// The existing .gz must be reported, never truncated.
	before, _ := os.ReadFile(src + ".gz")
	if res := Process(cfg, []string{src}); !res.HasErrors() {
		t.Fatal("expected an error when the .gz output already exists")
	}
	after, _ := os.ReadFile(src + ".gz")
	if !bytes.Equal(before, after) {
		t.Fatal("existing .gz file was modified")
	}
}

func TestExistingGzipIsRecompressed(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "weak.gz")
	var buf bytes.Buffer
	zw, _ := gzip.NewWriterLevel(&buf, gzip.NoCompression)
	zw.Name = "weak.txt"
	_, _ = zw.Write(compressibleText())
	_ = zw.Close()
	if err := os.WriteFile(path, buf.Bytes(), 0o644); err != nil {
		t.Fatal(err)
	}

	cfg := quietConfig()
	cfg.GzipMode = true
	cfg.Level = 9
	res := Process(cfg, []string{path})
	if res.HasErrors() || res.SavedBytes <= 0 {
		t.Fatalf("errors=%v saved=%d", res.Errors, res.SavedBytes)
	}

	f, _ := os.Open(path)
	defer f.Close()
	zr, err := gzip.NewReader(f)
	if err != nil {
		t.Fatal(err)
	}
	if zr.Name != "weak.txt" {
		t.Fatalf("header name not preserved: %q", zr.Name)
	}
	got, _ := io.ReadAll(zr)
	if !bytes.Equal(got, compressibleText()) {
		t.Fatal("content mismatch after re-compression")
	}
}

// storedZip builds an archive whose entries all use the Store method.
func storedZip(t *testing.T, entries map[string][]byte, order []string) []byte {
	t.Helper()
	var buf bytes.Buffer
	zw := zip.NewWriter(&buf)
	for _, name := range order {
		w, err := zw.CreateHeader(&zip.FileHeader{Name: name, Method: zip.Store})
		if err != nil {
			t.Fatal(err)
		}
		_, _ = w.Write(entries[name])
	}
	if err := zw.Close(); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func readZip(t *testing.T, path string) map[string][]byte {
	t.Helper()
	zr, err := zip.OpenReader(path)
	if err != nil {
		t.Fatal(err)
	}
	defer zr.Close()
	out := map[string][]byte{}
	for _, f := range zr.File {
		rc, err := f.Open()
		if err != nil {
			t.Fatal(err)
		}
		data, err := io.ReadAll(rc)
		_ = rc.Close()
		if err != nil {
			t.Fatalf("%s: %v", f.Name, err)
		}
		out[f.Name] = data
	}
	return out
}

func TestZipStoredEntriesDeflatedUnlessStrict(t *testing.T) {
	entries := map[string][]byte{"a.txt": compressibleText(), "tiny": []byte("x")}
	path := filepath.Join(t.TempDir(), "a.zip")
	if err := os.WriteFile(path, storedZip(t, entries, []string{"a.txt", "tiny"}), 0o644); err != nil {
		t.Fatal(err)
	}

	strict := quietConfig()
	strict.Strict = true
	if res := Process(strict, []string{path}); res.HasErrors() || res.SavedBytes != 0 {
		t.Fatalf("strict: errors=%v saved=%d", res.Errors, res.SavedBytes)
	}

	res := Process(quietConfig(), []string{path})
	if res.HasErrors() || res.SavedBytes <= 0 {
		t.Fatalf("errors=%v saved=%d", res.Errors, res.SavedBytes)
	}
	got := readZip(t, path)
	for name, data := range entries {
		if !bytes.Equal(got[name], data) {
			t.Fatalf("entry %s changed", name)
		}
	}
}

func TestZipModePacksDirectory(t *testing.T) {
	dir := t.TempDir()
	root := filepath.Join(dir, "bundle")
	if err := os.MkdirAll(filepath.Join(root, "sub"), 0o755); err != nil {
		t.Fatal(err)
	}
	_ = os.WriteFile(filepath.Join(root, "one.txt"), compressibleText(), 0o644)
	_ = os.WriteFile(filepath.Join(root, "sub", "two.txt"), []byte("two"), 0o644)

	cfg := quietConfig()
	cfg.ZipMode = true
	cfg.GzipMode = true
	if res := Process(cfg, []string{root}); res.HasErrors() {
		t.Fatalf("unexpected errors: %v", res.Errors)
	}
	got := readZip(t, root+".zip")
	if !bytes.Equal(got["bundle/one.txt"], compressibleText()) || string(got["bundle/sub/two.txt"]) != "two" {
		t.Fatalf("unexpected archive contents: %v", keys(got))
	}
	if _, ok := got["bundle/sub/"]; !ok {
		t.Fatalf("directory entry missing: %v", keys(got))
	}
}

func keys(m map[string][]byte) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	return out
}

func TestPNGReencodeIsLossless(t *testing.T) {
	img := image.NewNRGBA(image.Rect(0, 0, 128, 128))
	for y := 0; y < 128; y++ {
		for x := 0; x < 128; x++ {
			img.SetNRGBA(x, y, color.NRGBA{uint8(x), uint8(y), 0, 255})
		}
	}
	var buf bytes.Buffer
	enc := png.Encoder{CompressionLevel: png.NoCompression}
	if err := enc.Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(t.TempDir(), "img.png")
	_ = os.WriteFile(path, buf.Bytes(), 0o644)

	res := Process(quietConfig(), []string{path})
	if res.HasErrors() || res.SavedBytes <= 0 {
		t.Fatalf("errors=%v saved=%d", res.Errors, res.SavedBytes)
	}
	data, _ := os.ReadFile(path)
	out, err := png.Decode(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	for y := 0; y < 128; y++ {
		for x := 0; x < 128; x++ {
			// Compare normalized RGBA values: the decoder may return a
			// different (but equivalent) color model than the source.
			r1, g1, b1, a1 := out.At(x, y).RGBA()
			r2, g2, b2, a2 := img.At(x, y).RGBA()
			if r1 != r2 || g1 != g2 || b1 != b2 || a1 != a2 {
				t.Fatalf("pixel (%d,%d) differs", x, y)
			}
		}
	}
}

// pngHeaderOnly returns a PNG signature plus an IHDR chunk claiming the given
// dimensions, without any image data.
func pngHeaderOnly(w, h uint32) []byte {
	ihdr := make([]byte, 13)
	binary.BigEndian.PutUint32(ihdr[0:], w)
	binary.BigEndian.PutUint32(ihdr[4:], h)
	ihdr[8], ihdr[9] = 8, 6 // 8-bit RGBA
	chunk := append([]byte("IHDR"), ihdr...)
	var out bytes.Buffer
	out.WriteString("\x89PNG\r\n\x1a\n")
	_ = binary.Write(&out, binary.BigEndian, uint32(len(ihdr)))
	out.Write(chunk)
	_ = binary.Write(&out, binary.BigEndian, crc32.ChecksumIEEE(chunk))
	return out.Bytes()
}

func TestPNGDimensionLimit(t *testing.T) {
	path := filepath.Join(t.TempDir(), "bomb.png")
	_ = os.WriteFile(path, pngHeaderOnly(100000, 100000), 0o644)
	res := Process(quietConfig(), []string{path})
	if len(res.Errors) != 1 || !strings.Contains(res.Errors[0].Err.Error(), "pixel limit") {
		t.Fatalf("expected a pixel limit error, got %v", res.Errors)
	}
}

func TestDirectoryRequiresRecurse(t *testing.T) {
	dir := t.TempDir()
	_ = os.WriteFile(filepath.Join(dir, "x.txt"), []byte("x"), 0o644)
	if res := Process(quietConfig(), []string{dir}); len(res.Errors) != 1 {
		t.Fatalf("expected one error, got %v", res.Errors)
	}
}

func TestSymlinksAreNeverReplaced(t *testing.T) {
	dir := t.TempDir()
	target := filepath.Join(dir, "real.txt")
	_ = os.WriteFile(target, compressibleText(), 0o644)
	link := filepath.Join(dir, "link.txt")
	if err := os.Symlink(target, link); err != nil {
		t.Skipf("symlinks unavailable: %v", err)
	}

	cfg := quietConfig()
	cfg.GzipMode = true
	cfg.Recursive = true

	// Explicit symlink argument: error, link untouched.
	if res := Process(cfg, []string{link}); len(res.Errors) != 1 {
		t.Fatalf("expected one error, got %v", res.Errors)
	}
	if fi, err := os.Lstat(link); err != nil || fi.Mode()&os.ModeSymlink == 0 {
		t.Fatal("symlink was modified")
	}

	// During traversal links are skipped silently.
	if res := Process(cfg, []string{dir}); res.HasErrors() {
		t.Fatalf("unexpected errors: %v", res.Errors)
	}
	if _, err := os.Stat(link + ".gz"); err == nil {
		t.Fatal("symlink was followed during traversal")
	}
	if _, err := os.Stat(target + ".gz"); err != nil {
		t.Fatal("regular file was not processed")
	}
}
