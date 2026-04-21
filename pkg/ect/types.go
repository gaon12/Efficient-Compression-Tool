package ect

type FileType int

const (
	FileUnknown FileType = iota
	FilePNG
	FileJPEG
	FileGZIP
	FileZIP
	FileWEBP
)

type Config struct {
	Level       int
	Strip       bool
	Recursive   bool
	Quiet       bool
	KeepModTime bool
	GzipMode    bool
	ZipMode     bool
	DisablePNG  bool
	DisableJPEG bool
	DisableWEBP bool
	Strict      bool
	Workers     int
	Logf        func(format string, args ...any)
}

type FileError struct {
	Path string
	Err  error
}

type Result struct {
	ProcessedFiles int64
	TotalBytes     int64
	SavedBytes     int64
	Errors         []FileError
}

func (r Result) HasErrors() bool {
	return len(r.Errors) > 0
}
