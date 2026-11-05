package ect

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"sync"
)

// errSymlink is reported for symbolic links passed explicitly on the command
// line. Optimized output replaces files via rename, which would silently turn
// a link into a regular file (and leave its target untouched), so links are
// refused instead of being processed "through".
var errSymlink = errors.New("symbolic links are not processed")

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
				p.processZipTarget(path, rb)
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
		// Lstat (not Stat) so that symbolic links are detected instead of
		// followed; see errSymlink for the rationale.
		info, err := os.Lstat(path)
		if err != nil {
			rb.addError(path, err)
			continue
		}
		if info.Mode()&fs.ModeSymlink != 0 {
			rb.addError(path, errSymlink)
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

		err = filepath.WalkDir(path, func(filePath string, d fs.DirEntry, walkErr error) error {
			if walkErr != nil {
				rb.addError(filePath, walkErr)
				return nil
			}
			if d.IsDir() || d.Type()&fs.ModeSymlink != 0 {
				// Links are skipped while walking: following them could
				// escape the requested tree or loop forever.
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

// processZipTarget handles one `-zip` argument the same way as the C/C++
// CLI: an existing ZIP archive is re-optimized in place, anything else (a
// file or a directory) is packed into a new "<path>.zip".
func (p *Processor) processZipTarget(path string, rb *resultBuilder) {
	if isRegularZipFile(path) {
		p.processFile(path, rb)
		return
	}
	inputBytes, outputBytes, err := createZipArchive(path, p.cfg)
	if err != nil {
		rb.addError(path, err)
		return
	}
	rb.addProcessed(inputBytes)
	rb.addSaved(inputBytes - outputBytes)
	p.cfg.logf("%s: created ZIP archive (%d -> %d)", path, inputBytes, outputBytes)
}

// isRegularZipFile reports whether path is a regular file (symlinks are not
// followed) whose content is a ZIP archive.
func isRegularZipFile(path string) bool {
	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() {
		return false
	}
	kind, err := detectFileType(path)
	return err == nil && kind == FileZIP
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
	info, err := os.Lstat(path)
	if err != nil {
		rb.addError(path, err)
		return
	}
	if info.Mode()&fs.ModeSymlink != 0 {
		rb.addError(path, errSymlink)
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
		// Like the C/C++ version, ZIP archives are only rewritten with
		// -zip: many formats are ZIP files (.jar, .apk, .docx, .epub, ...)
		// and rewriting them unasked can break whole-file signatures (APK
		// v2+) or format-specific layout rules (e.g. EPUB's stored mimetype
		// entry must come first and stay uncompressed).
		if !p.cfg.ZipMode {
			return
		}
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
