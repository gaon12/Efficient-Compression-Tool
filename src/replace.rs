use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary file next to its final destination; removed on drop unless persisted.
pub(crate) struct TempFile {
    path: PathBuf,
    file: Option<File>,
}

impl TempFile {
    pub(crate) fn new_in(dir: &Path, prefix: &str) -> io::Result<TempFile> {
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        loop {
            let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!(".ect-{prefix}-{}-{n}", std::process::id()));
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            // Keep the temporary file private until it receives the original
            // file's permissions right before the final rename.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
            match opts.open(&path) {
                Ok(file) => {
                    return Ok(TempFile {
                        path,
                        file: Some(file),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    pub(crate) fn file(&mut self) -> &mut File {
        self.file.as_mut().expect("temp file already closed")
    }

    /// Flushes and closes the handle, returning the file size.
    pub(crate) fn close(&mut self) -> io::Result<u64> {
        if let Some(mut f) = self.file.take() {
            f.flush()?;
        }
        Ok(fs::metadata(&self.path)?.len())
    }

    /// Moves the temp file over `target`.
    pub(crate) fn persist(
        mut self,
        target: &Path,
        original: Option<&Metadata>,
        keep_mod_time: bool,
    ) -> io::Result<()> {
        self.close()?;
        replace_with_file(target, &self.path, original, keep_mod_time)?;
        self.path = PathBuf::new();
        Ok(())
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        self.file.take();
        if !self.path.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Atomically replaces `path` with `data`.
pub(crate) fn replace_with_bytes(
    path: &Path,
    data: &[u8],
    original: &Metadata,
    keep_mod_time: bool,
) -> io::Result<()> {
    let mut tmp = TempFile::new_in(parent_dir(path), "replace")?;
    tmp.file().write_all(data)?;
    tmp.persist(path, Some(original), keep_mod_time)
}

fn replace_with_file(
    target: &Path,
    source: &Path,
    original: Option<&Metadata>,
    keep_mod_time: bool,
) -> io::Result<()> {
    if let Some(meta) = original {
        let _ = fs::set_permissions(source, meta.permissions());
    }
    if cfg!(windows) {
        let _ = fs::remove_file(target);
    }
    fs::rename(source, target)?;
    if keep_mod_time && let Some(meta) = original {
        set_mod_time(target, meta);
    }
    Ok(())
}

pub(crate) fn set_mod_time(path: &Path, meta: &Metadata) {
    if let Ok(mtime) = meta.modified()
        && let Ok(f) = OpenOptions::new().write(true).open(path)
    {
        let _ = f.set_modified(mtime);
    }
}
