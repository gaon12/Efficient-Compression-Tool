package main

import (
	"fmt"
	"io"
	"os"
	"strconv"

	"github.com/fhanau/efficient-compression-tool/pkg/ect"
)

func run(args []string) int {
	parsed, err := parseCLI(args)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		usage(os.Stderr)
		return 2
	}

	if parsed.showHelp {
		usage(os.Stdout)
		return 0
	}

	if len(parsed.paths) == 0 {
		usage(os.Stdout)
		return 1
	}

	cfg := parsed.cfg
	cfg.Logf = func(format string, args ...any) {
		fmt.Printf(format+"\n", args...)
	}

	result := ect.Process(cfg, parsed.paths)
	for _, fileErr := range result.Errors {
		fmt.Fprintf(os.Stderr, "%s: %v\n", fileErr.Path, fileErr.Err)
	}

	printSummary(result, cfg.Quiet)

	if result.HasErrors() {
		return 2
	}
	return 0
}

func usage(w io.Writer) {
	fmt.Fprintln(w, "Efficient Compression Tool (Go rewrite)")
	fmt.Fprintln(w, "Usage: ect [Options] files/folders...")
	fmt.Fprintln(w, "Options:")
	fmt.Fprintln(w, " -1 to -9          Set compression level (default: 3)")
	fmt.Fprintln(w, " -strip            Strip metadata where format allows")
	fmt.Fprintln(w, " -recurse          Recursively process input directories")
	fmt.Fprintln(w, " -gzip             Compress non-gzip files to .gz and optimize .gz")
	fmt.Fprintln(w, " -zip              Create .zip archives for each given path")
	fmt.Fprintln(w, " -keep             Keep original modification times")
	fmt.Fprintln(w, " -quiet            Print only error messages")
	fmt.Fprintln(w, " -help             Show help")
	fmt.Fprintln(w, " --disable-png     Disable PNG optimization")
	fmt.Fprintln(w, " --disable-jpg     Disable JPEG optimization")
	fmt.Fprintln(w, " --disable-webp    Disable WebP optimization")
	fmt.Fprintln(w, " --strict          Keep stricter behavior for gzip/zip")
	fmt.Fprintln(w, " --mt-file[=N]     Number of file workers")
	fmt.Fprintln(w, " --                Treat all following arguments as positional paths")
}

func printSummary(result ect.Result, quiet bool) {
	if quiet {
		return
	}

	if result.ProcessedFiles == 0 {
		fmt.Println("No compatible files found")
		return
	}

	fmt.Printf("Processed %d file", result.ProcessedFiles)
	if result.ProcessedFiles != 1 {
		fmt.Print("s")
	}
	fmt.Println()

	if result.TotalBytes == 0 {
		fmt.Println("No compatible files found")
		return
	}

	if result.SavedBytes < 0 {
		bigger := formatBytes(float64(-result.SavedBytes))
		total := formatBytes(float64(result.TotalBytes))
		pct := 100.0 * float64(result.SavedBytes) / float64(result.TotalBytes)
		fmt.Printf("Result is bigger by %s out of %s (%.4f%%)\n", bigger, total, pct)
		return
	}

	saved := formatBytes(float64(result.SavedBytes))
	total := formatBytes(float64(result.TotalBytes))
	pct := 100.0 * float64(result.SavedBytes) / float64(result.TotalBytes)
	fmt.Printf("Saved %s out of %s (%.4f%%)\n", saved, total, pct)
}

func formatBytes(v float64) string {
	units := []string{"B", "KB", "MB", "GB", "TB", "PB"}
	i := 0
	for v >= 1024.0 && i < len(units)-1 {
		v /= 1024.0
		i++
	}
	if i == 0 {
		return strconv.FormatFloat(v, 'f', 0, 64) + units[i]
	}
	return strconv.FormatFloat(v, 'f', 2, 64) + units[i]
}
