use std::sync::Arc;
use std::thread;

/// Logging callback used for per-file progress messages.
pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

/// Options controlling how files are optimized.
#[derive(Clone)]
pub struct Config {
    /// Compression level, 1 (fastest) to 9 (strongest).
    pub level: u32,
    pub strip: bool,
    pub recursive: bool,
    pub quiet: bool,
    pub keep_mod_time: bool,
    pub gzip_mode: bool,
    pub zip_mode: bool,
    pub disable_png: bool,
    pub disable_jpeg: bool,
    pub disable_webp: bool,
    pub strict: bool,
    /// Number of parallel file workers (0 = number of CPUs).
    pub workers: usize,
    pub log: Option<LogFn>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            level: 3,
            strip: false,
            recursive: false,
            quiet: false,
            keep_mod_time: false,
            gzip_mode: false,
            zip_mode: false,
            disable_png: false,
            disable_jpeg: false,
            disable_webp: false,
            strict: false,
            workers: 0,
            log: None,
        }
        .normalized()
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("level", &self.level)
            .field("strip", &self.strip)
            .field("recursive", &self.recursive)
            .field("quiet", &self.quiet)
            .field("keep_mod_time", &self.keep_mod_time)
            .field("gzip_mode", &self.gzip_mode)
            .field("zip_mode", &self.zip_mode)
            .field("disable_png", &self.disable_png)
            .field("disable_jpeg", &self.disable_jpeg)
            .field("disable_webp", &self.disable_webp)
            .field("strict", &self.strict)
            .field("workers", &self.workers)
            .finish_non_exhaustive()
    }
}

impl Config {
    pub(crate) fn normalized(mut self) -> Self {
        self.level = self.level.clamp(1, 9);
        if self.workers == 0 {
            self.workers = thread::available_parallelism().map_or(1, |n| n.get());
        }
        self
    }

    pub(crate) fn logf(&self, msg: impl FnOnce() -> String) {
        if self.quiet {
            return;
        }
        if let Some(log) = &self.log {
            log(&msg());
        }
    }
}

/// Maps an ECT level (1-9) to the zlib level used for Deflate streams.
///
/// zlib-rs implements zlib level 1 with the "quick" strategy, which is only
/// marginally faster than level 2 but much weaker: on the Canterbury Corpus
/// level 1 produced 1,151,148 bytes versus 816,644 bytes for level 2
/// (+41%) while saving under 10% of the time. ECT is an optimizer, so `-1`
/// uses zlib level 2; levels 2-9 map one-to-one.
pub(crate) fn deflate_level(level: u32) -> u32 {
    level.clamp(2, 9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deflate_level_mapping() {
        assert_eq!(deflate_level(1), 2);
        assert_eq!(deflate_level(3), 3);
        assert_eq!(deflate_level(9), 9);
        assert_eq!(deflate_level(42), 9);
    }

    #[test]
    fn normalization_clamps_level_and_sets_workers() {
        let cfg = Config {
            level: 0,
            workers: 0,
            ..Config::default()
        }
        .normalized();
        assert_eq!(cfg.level, 1);
        assert!(cfg.workers >= 1);
    }
}
