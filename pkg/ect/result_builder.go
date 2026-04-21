package ect

import (
	"sync"
	"sync/atomic"
)

type resultBuilder struct {
	processed atomic.Int64
	total     atomic.Int64
	saved     atomic.Int64

	mu     sync.Mutex
	errors []FileError
}

func (r *resultBuilder) addProcessed(bytes int64) {
	r.processed.Add(1)
	r.total.Add(bytes)
}

func (r *resultBuilder) addSaved(delta int64) {
	r.saved.Add(delta)
}

func (r *resultBuilder) addError(path string, err error) {
	if err == nil {
		return
	}
	r.mu.Lock()
	r.errors = append(r.errors, FileError{Path: path, Err: err})
	r.mu.Unlock()
}

func (r *resultBuilder) result() Result {
	r.mu.Lock()
	errCopy := append([]FileError(nil), r.errors...)
	r.mu.Unlock()

	return Result{
		ProcessedFiles: r.processed.Load(),
		TotalBytes:     r.total.Load(),
		SavedBytes:     r.saved.Load(),
		Errors:         errCopy,
	}
}
