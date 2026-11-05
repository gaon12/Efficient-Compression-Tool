//! Efficient Compression Tool (ECT), pure Rust implementation.
//!
//! This crate mirrors the behavior of the Go implementation (`pkg/ect`) so the
//! three implementations in this repository (C/C++, Go, Rust) can be used
//! interchangeably from the command line.

mod config;
mod detect;
mod gzip;
mod jpeg;
mod png_opt;
mod processor;
mod replace;
pub(crate) mod webp;
mod zip_opt;

pub use config::{Config, LogFn};
pub use detect::FileType;
pub use processor::{FileError, Processor, Report, process};
pub use webp::is_animated_webp;
