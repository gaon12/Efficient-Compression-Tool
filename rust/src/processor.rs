use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::thread;

use crate::config::Config;
use crate::detect::{FileType, detect_file_type};
use crate::{gzip, jpeg, png_opt, webp, zip_opt};

/// An error that occurred while processing one path.
#[derive(Debug)]
pub struct FileError {
    pub path: PathBuf,
    pub error: io::Error,
}

/// Aggregate result of a run.
#[derive(Debug, Default)]
pub struct Report {
    pub processed_files: i64,
    pub total_bytes: i64,
    pub saved_bytes: i64,
    pub errors: Vec<FileError>,
}

impl Report {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Size before/after a single optimization.
pub(crate) struct Outcome {
    pub before: u64,
    pub after: u64,
    pub changed: bool,
}

impl Outcome {
    pub(crate) fn unchanged(size: u64) -> Self {
        Outcome {
            before: size,
            after: size,
            changed: false,
        }
    }
    pub(crate) fn changed(before: u64, after: u64) -> Self {
        Outcome {
            before,
            after,
            changed: true,
        }
    }
}

#[derive(Default)]
struct ResultBuilder {
    processed: AtomicI64,
    total: AtomicI64,
    saved: AtomicI64,
    errors: Mutex<Vec<FileError>>,
}

impl ResultBuilder {
    fn add_processed(&self, bytes: u64) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(bytes as i64, Ordering::Relaxed);
    }
    fn add_saved(&self, delta: i64) {
        self.saved.fetch_add(delta, Ordering::Relaxed);
    }
    fn add_error(&self, path: &Path, error: io::Error) {
        self.errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(FileError {
                path: path.to_path_buf(),
                error,
            });
    }
    fn finish(self) -> Report {
        Report {
            processed_files: self.processed.into_inner(),
            total_bytes: self.total.into_inner(),
            saved_bytes: self.saved.into_inner(),
            errors: self.errors.into_inner().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

/// Convenience wrapper: `Processor::new(cfg).process_paths(paths)`.
pub fn process<P: AsRef<Path>>(cfg: Config, paths: &[P]) -> Report {
    Processor::new(cfg).process_paths(paths)
}

pub struct Processor {
    cfg: Config,
}

impl Processor {
    pub fn new(cfg: Config) -> Self {
        Processor {
            cfg: cfg.normalized(),
        }
    }

    pub fn process_paths<P: AsRef<Path>>(&self, paths: &[P]) -> Report {
        let rb = ResultBuilder::default();
        let mut normalized: Vec<PathBuf> = paths.iter().map(|p| p.as_ref().to_path_buf()).collect();
        normalized.sort();

        if self.cfg.zip_mode {
            self.run_parallel(&normalized, |path| {
                match zip_opt::create_zip_archive(path, &self.cfg) {
                    Ok((input, output)) => {
                        rb.add_processed(input);
                        rb.add_saved(input as i64 - output as i64);
                        self.cfg.logf(|| {
                            format!(
                                "{}: created ZIP archive ({input} -> {output})",
                                path.display()
                            )
                        });
                    }
                    Err(e) => rb.add_error(path, e),
                }
            });
        } else {
            let files = self.collect_files(&normalized, &rb);
            self.run_parallel(&files, |path| self.process_file(path, &rb));
        }
        rb.finish()
    }

    fn run_parallel<F: Fn(&Path) + Sync>(&self, items: &[PathBuf], f: F) {
        let workers = self.cfg.workers.min(items.len()).max(1);
        let next = AtomicUsize::new(0);
        let work = || {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = items.get(i) else { break };
                f(path);
            }
        };
        if workers == 1 {
            work();
            return;
        }
        thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(work);
            }
        });
    }

    fn collect_files(&self, paths: &[PathBuf], rb: &ResultBuilder) -> Vec<PathBuf> {
        let mut files = BTreeSet::new();
        for path in paths {
            let meta = match fs::metadata(path) {
                Ok(m) => m,
                Err(e) => {
                    rb.add_error(path, e);
                    continue;
                }
            };
            if !meta.is_dir() {
                files.insert(path.clone());
                continue;
            }
            if !self.cfg.recursive {
                rb.add_error(
                    path,
                    io::Error::other("is a directory (use -recurse to process its files)"),
                );
                continue;
            }
            walk_dir(path, &mut files, rb);
        }
        files.into_iter().collect()
    }

    fn process_file(&self, path: &Path, rb: &ResultBuilder) {
        let cfg = &self.cfg;
        let result = (|| -> io::Result<()> {
            // `symlink_metadata` does not follow links: replacing a symlink via
            // rename would silently turn it into a regular file, so links are
            // rejected instead of being optimized "through".
            let meta = fs::symlink_metadata(path)?;
            if meta.file_type().is_symlink() {
                return Err(io::Error::other("symbolic links are not processed"));
            }
            let kind = detect_file_type(path)?;
            let (label, outcome) = match kind {
                FileType::Png if !cfg.disable_png => {
                    ("PNG optimized", png_opt::optimize_png(path, &meta, cfg)?)
                }
                FileType::Jpeg if !cfg.disable_jpeg => {
                    ("JPEG optimized", jpeg::optimize_jpeg(path, &meta, cfg)?)
                }
                FileType::Webp if !cfg.disable_webp => {
                    ("WebP optimized", webp::optimize_webp(path, &meta, cfg)?)
                }
                FileType::Gzip if cfg.gzip_mode => (
                    "gzip optimized",
                    gzip::optimize_existing_gzip(path, &meta, cfg)?,
                ),
                FileType::Zip => (
                    "ZIP optimized",
                    zip_opt::optimize_existing_zip(path, &meta, cfg)?,
                ),
                FileType::Unknown if cfg.gzip_mode => {
                    let out = gzip::compress_file_to_gzip(path, &meta, cfg)?;
                    ("compressed to gzip", Outcome::changed(meta.len(), out))
                }
                _ => return Ok(()),
            };
            rb.add_processed(outcome.before);
            if outcome.changed {
                rb.add_saved(outcome.before as i64 - outcome.after as i64);
                cfg.logf(|| {
                    format!(
                        "{}: {label} ({} -> {})",
                        path.display(),
                        outcome.before,
                        outcome.after
                    )
                });
            }
            Ok(())
        })();
        if let Err(e) = result {
            rb.add_error(path, e);
        }
    }
}

fn walk_dir(dir: &Path, files: &mut BTreeSet<PathBuf>, rb: &ResultBuilder) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            rb.add_error(dir, e);
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                rb.add_error(dir, e);
                continue;
            }
        };
        let path = entry.path();
        match entry.file_type() {
            // Symlinks are skipped during traversal: following them could
            // escape the requested tree or loop forever.
            Ok(t) if t.is_symlink() => {}
            Ok(t) if t.is_dir() => walk_dir(&path, files, rb),
            Ok(_) => {
                files.insert(path);
            }
            Err(e) => rb.add_error(&path, e),
        }
    }
}
