package ect

import (
	"os"
	"path/filepath"
	"runtime"
	"time"
)

func replaceWithBytes(path string, b []byte, original os.FileInfo, keepModTime bool) error {
	temp, err := os.CreateTemp(filepath.Dir(path), ".ect-replace-*")
	if err != nil {
		return err
	}
	tempName := temp.Name()

	if _, err := temp.Write(b); err != nil {
		_ = temp.Close()
		_ = os.Remove(tempName)
		return err
	}
	if err := temp.Close(); err != nil {
		_ = os.Remove(tempName)
		return err
	}

	if err := replaceWithFile(path, tempName, original, keepModTime); err != nil {
		_ = os.Remove(tempName)
		return err
	}
	return nil
}

func replaceWithFile(target, source string, original os.FileInfo, keepModTime bool) error {
	if original != nil {
		_ = os.Chmod(source, original.Mode().Perm())
	}

	if runtime.GOOS == "windows" {
		_ = os.Remove(target)
	}

	if err := os.Rename(source, target); err != nil {
		return err
	}

	if keepModTime && original != nil {
		mt := original.ModTime()
		_ = os.Chtimes(target, mt, mt)
	}

	return nil
}

func fakeInfo(path string) os.FileInfo {
	info, err := os.Stat(path)
	if err == nil {
		return info
	}
	return fakeFileInfo{name: filepath.Base(path), mod: time.Unix(0, 0)}
}

type fakeFileInfo struct {
	name string
	mod  time.Time
}

func (f fakeFileInfo) Name() string       { return f.name }
func (f fakeFileInfo) Size() int64        { return 0 }
func (f fakeFileInfo) Mode() os.FileMode  { return 0o644 }
func (f fakeFileInfo) ModTime() time.Time { return f.mod }
func (f fakeFileInfo) IsDir() bool        { return false }
func (f fakeFileInfo) Sys() any           { return nil }
