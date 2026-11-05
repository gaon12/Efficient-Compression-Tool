package ect

import (
	"archive/zip"
	"bytes"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"testing"
)

// Regression tests for problems found by the cross-implementation
// conformance harness.

// uncompressedPNG returns a smooth gradient PNG written at the lowest
// compression level, so re-encoding always shrinks it.
func uncompressedPNG(t *testing.T) []byte {
	t.Helper()
	img := image.NewRGBA(image.Rect(0, 0, 32, 32))
	for y := 0; y < 32; y++ {
		for x := 0; x < 32; x++ {
			img.Set(x, y, color.RGBA{uint8(x * 8), uint8(y * 8), 100, 255})
		}
	}
	var buf bytes.Buffer
	enc := png.Encoder{CompressionLevel: png.NoCompression}
	if err := enc.Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

// withChunks inserts extra chunks right after IHDR.
func withChunks(t *testing.T, data []byte, extra ...pngChunk) []byte {
	t.Helper()
	chunks, err := parsePNGChunks(data)
	if err != nil {
		t.Fatal(err)
	}
	out := append([]pngChunk{chunks[0]}, extra...)
	return writePNGChunks(append(out, chunks[1:]...))
}

func chunkTypes(t *testing.T, data []byte) []string {
	t.Helper()
	chunks, err := parsePNGChunks(data)
	if err != nil {
		t.Fatal(err)
	}
	var types []string
	for _, c := range chunks {
		types = append(types, c.typ)
	}
	return types
}

func TestPNGMetadataKeptUnlessStrip(t *testing.T) {
	meta := []pngChunk{
		{"gAMA", []byte{0, 0, 0xb1, 0x8f}},
		{"pHYs", []byte{0, 0, 0x0b, 0x13, 0, 0, 0x0b, 0x13, 1}},
		{"tEXt", []byte("Comment\x00hello")},
		{"prVT", []byte("private, unsafe to copy")},
	}
	src := withChunks(t, uncompressedPNG(t), meta...)
	path := filepath.Join(t.TempDir(), "a.png")
	_ = os.WriteFile(path, src, 0o644)

	if res := Process(quietConfig(), []string{path}); res.HasErrors() || res.SavedBytes <= 0 {
		t.Fatalf("errors=%v saved=%d", res.Errors, res.SavedBytes)
	}
	got, _ := os.ReadFile(path)
	want := []string{"IHDR", "gAMA", "pHYs", "tEXt", "IDAT", "IEND"}
	if types := chunkTypes(t, got); !equalStrings(types, want) {
		t.Fatalf("chunks %v, want %v", types, want)
	}

	cfg := quietConfig()
	cfg.Strip = true
	_ = os.WriteFile(path, src, 0o644)
	if res := Process(cfg, []string{path}); res.HasErrors() {
		t.Fatal(res.Errors)
	}
	got, _ = os.ReadFile(path)
	if types := chunkTypes(t, got); !equalStrings(types, []string{"IHDR", "IDAT", "IEND"}) {
		t.Fatalf("-strip kept metadata: %v", types)
	}
}

func equalStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func TestAnimatedPNGIsLeftAlone(t *testing.T) {
	// An acTL chunk marks an APNG; image/png would decode only the
	// default image, so the file must not be rewritten.
	src := withChunks(t, uncompressedPNG(t), pngChunk{"acTL", []byte{0, 0, 0, 1, 0, 0, 0, 0}})
	path := filepath.Join(t.TempDir(), "anim.png")
	_ = os.WriteFile(path, src, 0o644)
	if res := Process(quietConfig(), []string{path}); res.HasErrors() || res.SavedBytes != 0 {
		t.Fatalf("errors=%v saved=%d", res.Errors, res.SavedBytes)
	}
	if got, _ := os.ReadFile(path); !bytes.Equal(got, src) {
		t.Fatal("APNG was modified")
	}
}

func TestPaletteIndexOutOfRangeIsRejected(t *testing.T) {
	img := image.NewPaletted(image.Rect(0, 0, 16, 16), color.Palette{
		color.RGBA{255, 0, 0, 255}, color.RGBA{0, 255, 0, 255}, color.RGBA{0, 0, 255, 255},
	})
	for i := range img.Pix {
		img.Pix[i] = uint8(i % 3)
	}
	img.Pix[5] = 2
	var buf bytes.Buffer
	if err := (&png.Encoder{CompressionLevel: png.NoCompression}).Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	// Shrink PLTE to two entries so index 2 points past its end.
	chunks, _ := parsePNGChunks(buf.Bytes())
	for i := range chunks {
		if chunks[i].typ == "PLTE" {
			chunks[i].data = chunks[i].data[:6]
		}
	}
	src := writePNGChunks(chunks)
	path := filepath.Join(t.TempDir(), "bad.png")
	_ = os.WriteFile(path, src, 0o644)
	if res := Process(quietConfig(), []string{path}); len(res.Errors) != 1 {
		t.Fatalf("expected one error, got %v", res.Errors)
	}
	if got, _ := os.ReadFile(path); !bytes.Equal(got, src) {
		t.Fatal("invalid PNG was rewritten")
	}
}

func TestGzipModeCompressesImagesToGz(t *testing.T) {
	path := filepath.Join(t.TempDir(), "a.png")
	src := uncompressedPNG(t)
	_ = os.WriteFile(path, src, 0o644)
	cfg := quietConfig()
	cfg.GzipMode = true
	if res := Process(cfg, []string{path}); res.HasErrors() {
		t.Fatal(res.Errors)
	}
	if _, err := os.Stat(path + ".gz"); err != nil {
		t.Fatal("-gzip did not create a.png.gz")
	}
	if got, _ := os.ReadFile(path); !bytes.Equal(got, src) {
		t.Fatal("-gzip modified the PNG itself")
	}
}

func TestZipModeOnEmptyZipDoesNotPackIt(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "a.zip")
	_ = os.WriteFile(path, nil, 0o644)
	cfg := quietConfig()
	cfg.ZipMode = true
	cfg.GzipMode = true
	if res := Process(cfg, []string{path}); len(res.Errors) != 1 {
		t.Fatalf("expected one error, got %v", res.Errors)
	}
	if _, err := os.Stat(path + ".zip"); err == nil {
		t.Fatal("empty a.zip was packed into a.zip.zip")
	}
}

func TestZipKeepsUTF8NamesWhenRecompressing(t *testing.T) {
	name := "이름/유니코드.txt"
	path := filepath.Join(t.TempDir(), "u.zip")
	_ = os.WriteFile(path, storedZip(t, map[string][]byte{name: compressibleText()}, []string{name}), 0o644)
	cfg := quietConfig()
	cfg.ZipMode = true
	if res := Process(cfg, []string{path}); res.HasErrors() || res.SavedBytes <= 0 {
		t.Fatalf("errors=%v saved=%d", res.Errors, res.SavedBytes)
	}
	zr, err := zip.OpenReader(path)
	if err != nil {
		t.Fatal(err)
	}
	defer zr.Close()
	f := zr.File[0]
	if f.Name != name || f.Flags&zipUTF8Flag == 0 || f.Method != zip.Deflate {
		t.Fatalf("name=%q flags=%#x method=%d", f.Name, f.Flags, f.Method)
	}
}
