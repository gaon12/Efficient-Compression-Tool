package main

import (
	"errors"
	"fmt"
	"strconv"
	"strings"

	"github.com/fhanau/efficient-compression-tool/pkg/ect"
)

type cliInput struct {
	cfg      ect.Config
	paths    []string
	showHelp bool
}

func parseCLI(args []string) (cliInput, error) {
	cfg := ect.DefaultConfig()
	var paths []string
	positionalOnly := false

	for _, arg := range args {
		if !positionalOnly && arg == "--" {
			positionalOnly = true
			continue
		}

		if !positionalOnly && strings.HasPrefix(arg, "-") {
			switch {
			case len(arg) == 2 && arg[0] == '-' && arg[1] >= '1' && arg[1] <= '9':
				cfg.Level = int(arg[1] - '0')
			case arg == "-strip":
				cfg.Strip = true
			case arg == "-recurse":
				cfg.Recursive = true
			case arg == "-quiet":
				cfg.Quiet = true
			case arg == "-keep":
				cfg.KeepModTime = true
			case arg == "-gzip":
				cfg.GzipMode = true
			case arg == "-zip":
				cfg.ZipMode = true
				cfg.GzipMode = true
			case arg == "-help" || arg == "--help" || arg == "-h":
				return cliInput{cfg: cfg, showHelp: true}, nil
			case arg == "--disable-png":
				cfg.DisablePNG = true
			case arg == "--disable-jpg" || arg == "--disable-jpeg":
				cfg.DisableJPEG = true
			case arg == "--disable-webp":
				cfg.DisableWEBP = true
			case arg == "--strict":
				cfg.Strict = true
			case arg == "--mt-file":
				// default worker count from ect.DefaultConfig()
			case strings.HasPrefix(arg, "--mt-file="):
				v := strings.TrimPrefix(arg, "--mt-file=")
				n, err := strconv.Atoi(v)
				if err != nil || n < 1 {
					return cliInput{}, fmt.Errorf("invalid --mt-file value: %s", v)
				}
				cfg.Workers = n
			case arg == "--mt-deflate", strings.HasPrefix(arg, "--mt-deflate="):
				// Accepted for CLI compatibility.
			case arg == "-progressive", arg == "-autorotate", arg == "-autorotate=force":
				// Accepted for CLI compatibility.
			case arg == "--reuse", arg == "--allfilters", arg == "--allfilters-b", arg == "--allfilters-c":
				// Accepted for CLI compatibility.
			case strings.HasPrefix(arg, "--pal_sort="):
				// Accepted for CLI compatibility.
			default:
				return cliInput{}, errors.New("unknown option: " + arg)
			}
			continue
		}

		paths = append(paths, arg)
	}

	return cliInput{cfg: cfg, paths: paths}, nil
}
