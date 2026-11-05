use std::fs::{self, Metadata};
use std::io;
use std::path::Path;

use crate::config::Config;
use crate::processor::Outcome;

/// WebP files are detected and parsed; lossless re-encoding is not implemented
/// in the pure Rust version (same behavior as the Go version).
pub(crate) fn optimize_webp(path: &Path, _meta: &Metadata, _cfg: &Config) -> io::Result<Outcome> {
    // The file is read so that unreadable files are still reported as errors
    // and counted, exactly like the Go implementation. Animated WebP files
    // (detected via `is_animated_webp`) would always be skipped anyway.
    let size = fs::read(path)?.len() as u64;
    Ok(Outcome::unchanged(size))
}

/// Returns true if the RIFF container holds an `ANIM` chunk.
pub fn is_animated_webp(data: &[u8]) -> bool {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return false;
    }
    let mut i = 12usize;
    while i + 8 <= data.len() {
        let kind = &data[i..i + 4];
        let size =
            u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
        let end = match (i + 8).checked_add(size) {
            Some(end) if end <= data.len() => end,
            _ => return false,
        };
        if kind == b"ANIM" {
            return true;
        }
        i = end + (size & 1);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_anim_chunk() {
        let mut v = b"RIFF\0\0\0\0WEBP".to_vec();
        v.extend_from_slice(b"VP8X\x0a\0\0\0");
        v.extend_from_slice(&[0u8; 10]);
        assert!(!is_animated_webp(&v));
        v.extend_from_slice(b"ANIM\x06\0\0\0");
        v.extend_from_slice(&[0u8; 6]);
        assert!(is_animated_webp(&v));
    }
}
