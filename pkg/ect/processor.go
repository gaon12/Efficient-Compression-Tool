package ect

import (
	"errors"
	"os"
	"path/filepath"
	"sort"
	"sync"
)

func Process(cfg Config, paths []string) Result {
	return NewProcessor(cfg).ProcessPaths(paths)
}

type Processor struct {
	cfg Config
}

func NewProcessor(cfg Config) *Processor {
	return &Processor{cfg: normalizeConfig(cfg)}
}

func (p *Processor) ProcessPaths(paths []string) Result {
	rb := &resultBuilder{}
	if len(paths) == 0 {
		return rb.result()
	}

	normalized := append([]string(nil), paths...)
	sort.Strings(normalized)

	if p.cfg.ZipMode {
		p.processZipTargets(normalized, rb)
		return rb.result()
	}

	files := p.collectFiles(normalized, rb)
	p.processFiles(files, rb)
	return rb.result()
}

func (p *Processor) processZipTargets(paths []string, rb *resultBuilder) {
	workers := p.cfg.Workers
	if workers > len(paths) {
		workers = len(paths)
	}
	if workers < 1 {
		workers = 1
	}

	jobs := make(chan string)
	var wg sync.WaitGroup

	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for path := range jobs {
				inputBytes, outputBytes, err := createZipArchive(path, p.cfg)
				if err != nil {
					rb.addError(path, err)
					continue
				}
				rb.addProcessed(inputBytes)
				rb.addSaved(inputBytes - outputBytes)
				p.cfg.logf("%s: created ZIP archive (%d -> %d)", path, inputBytes, outputBytes)
			}
		}()
	}

	for _, path := range paths {
		jobs <- path
	}
	close(jobs)
	wg.Wait()
}

func (p *Processor) collectFiles(paths []string, rb *resultBuilder) []string {
	filesMap := make(map[string]struct{})

	for _, path := range paths {
		info, err := os.Stat(path)
		if err != nil {
			rb.addError(path, err)
			continue
		}

		if !info.IsDir() {
			filesMap[path] = struct{}{}
			continue
		}

		if !p.cfg.Recursive {
			rb.addError(path, errors.New("is a directory (use -recurse to process its files)"))
			continue
		}

		err = filepath.WalkDir(path, func(filePath string, d os.DirEntry, walkErr error) error {
			if walkErr != nil {
				rb.addError(filePath, walkErr)
				return nil
			}
			if d.IsDir() {
				return nil
			}
			filesMap[filePath] = struct{}{}
			return nil
		})
		if err != nil {
			rb.addError(path, err)
		}
	}

	files := make([]string, 0, len(filesMap))
	for filePath := range filesMap {
		files = append(files, filePath)
	}
	sort.Strings(files)
	return files
}

func (p *Processor) processFiles(files []string, rb *resultBuilder) {
	if len(files) == 0 {
		return
	}

	workers := p.cfg.Workers
	if workers > len(files) {
		workers = len(files)
	}
	if workers < 1 {
		workers = 1
	}

	jobs := make(chan string)
	var wg sync.WaitGroup

	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for path := range jobs {
				p.processFile(path, rb)
			}
		}()
	}

	for _, path := range files {
		jobs <- path
	}
	close(jobs)
	wg.Wait()
}

func (p *Processor) processFile(path string, rb *resultBuilder) {
	info, err := os.Stat(path)
	if err != nil {
		rb.addError(path, err)
		return
	}

	kind, err := detectFileType(path)
	if err != nil {
		rb.addError(path, err)
		return
	}

	switch kind {
	case FilePNG:
		if p.cfg.DisablePNG {
			return
		}
		before, after, changed, err := optimizePNG(path, info, p.cfg)
		if err != nil {
			rb.addError(path, err)
			return
		}
		rb.addProcessed(before)
		if changed {
			rb.addSaved(before - after)
			p.cfg.logf("%s: PNG optimized (%d -> %d)", path, before, after)
		}
	case FileJPEG:
		if p.cfg.DisableJPEG {
			return
		}
		before, after, changed, err := optimizeJPEG(path, info, p.cfg)
		if err != nil {
			rb.addError(path, err)
			return
		}
		rb.addProcessed(before)
		if changed {
			rb.addSaved(before - after)
			p.cfg.logf("%s: JPEG optimized (%d -> %d)", path, before, after)
		}
	case FileWEBP:
		if p.cfg.DisableWEBP {
			return
		}
		before, after, changed, err := optimizeWEBP(path, info, p.cfg)
		if err != nil {
			rb.addError(path, err)
			return
		}
		rb.addProcessed(before)
		if changed {
			rb.addSaved(before - after)
			p.cfg.logf("%s: WebP optimized (%d -> %d)", path, before, after)
		}
	case FileGZIP:
		if !p.cfg.GzipMode {
			return
		}
		before, after, changed, err := optimizeExistingGzip(path, info, p.cfg)
		if err != nil {
			rb.addError(path, err)
			return
		}
		rb.addProcessed(before)
		if changed {
			rb.addSaved(before - after)
			p.cfg.logf("%s: gzip optimized (%d -> %d)", path, before, after)
		}
	case FileZIP:
		before, after, changed, err := optimizeExistingZIP(path, info, p.cfg)
		if err != nil {
			rb.addError(path, err)
			return
		}
		rb.addProcessed(before)
		if changed {
			rb.addSaved(before - after)
			p.cfg.logf("%s: ZIP optimized (%d -> %d)", path, before, after)
		}
	default:
		if !p.cfg.GzipMode {
			return
		}
		outSize, err := compressFileToGzip(path, info, p.cfg)
		if err != nil {
			rb.addError(path, err)
			return
		}
		rb.addProcessed(info.Size())
		rb.addSaved(info.Size() - outSize)
		p.cfg.logf("%s: compressed to gzip (%d -> %d)", path, info.Size(), outSize)
	}
}
