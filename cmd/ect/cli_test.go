package main

import (
	"bytes"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestParseCLIFlagsAndPaths(t *testing.T) {
	in, err := parseCLI([]string{"-9", "-strip", "-recurse", "--mt-file=3", "a.png", "--", "-weird"})
	if err != nil {
		t.Fatal(err)
	}
	if in.cfg.Level != 9 || !in.cfg.Strip || !in.cfg.Recursive || in.cfg.Workers != 3 {
		t.Fatalf("unexpected config: %+v", in.cfg)
	}
	if want := []string{"a.png", "-weird"}; !reflect.DeepEqual(in.paths, want) {
		t.Fatalf("paths = %v, want %v", in.paths, want)
	}
}

func TestParseCLIZipImpliesGzip(t *testing.T) {
	in, err := parseCLI([]string{"-zip", "x"})
	if err != nil {
		t.Fatal(err)
	}
	if !in.cfg.ZipMode || !in.cfg.GzipMode {
		t.Fatalf("-zip must enable zip and gzip mode: %+v", in.cfg)
	}
}

func TestParseCLIAcceptsLegacyFlags(t *testing.T) {
	legacy := []string{"-progressive", "-autorotate", "-autorotate=force", "--mt-deflate", "--mt-deflate=4",
		"--reuse", "--allfilters", "--allfilters-b", "--allfilters-c", "--pal_sort=10"}
	if _, err := parseCLI(append(legacy, "file")); err != nil {
		t.Fatalf("legacy flags must be accepted: %v", err)
	}
}

func TestParseCLIRejectsInvalidInput(t *testing.T) {
	for _, args := range [][]string{{"--nope"}, {"--mt-file=0"}, {"--mt-file=abc"}} {
		if _, err := parseCLI(args); err == nil {
			t.Errorf("expected an error for %v", args)
		}
	}
}

func TestRunExitCodes(t *testing.T) {
	if code := run([]string{"-help"}); code != 0 {
		t.Errorf("-help exit code = %d, want 0", code)
	}
	if code := run([]string{"--nope"}); code != 2 {
		t.Errorf("invalid option exit code = %d, want 2", code)
	}
	if code := run([]string{"-quiet", filepath.Join(t.TempDir(), "missing")}); code != 2 {
		t.Errorf("missing file exit code = %d, want 2", code)
	}

	file := filepath.Join(t.TempDir(), "data.txt")
	if err := os.WriteFile(file, []byte(strings.Repeat("abc", 1000)), 0o644); err != nil {
		t.Fatal(err)
	}
	if code := run([]string{"-quiet", "-gzip", file}); code != 0 {
		t.Errorf("successful run exit code = %d, want 0", code)
	}
}

func TestFormatBytes(t *testing.T) {
	cases := map[float64]string{0: "0B", 1023: "1023B", 1024: "1.00KB", 1536: "1.50KB", 1 << 20: "1.00MB"}
	for in, want := range cases {
		if got := formatBytes(in); got != want {
			t.Errorf("formatBytes(%v) = %q, want %q", in, got, want)
		}
	}
}

func TestUsageListsAllOptions(t *testing.T) {
	var buf bytes.Buffer
	usage(&buf)
	for _, opt := range []string{"-strip", "-recurse", "-gzip", "-zip", "-keep", "-quiet", "--strict", "--mt-file", "--"} {
		if !strings.Contains(buf.String(), opt) {
			t.Errorf("usage text does not mention %s", opt)
		}
	}
}
