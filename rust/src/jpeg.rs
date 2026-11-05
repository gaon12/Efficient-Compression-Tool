use std::fs::{self, Metadata};
use std::io;
use std::path::Path;

use crate::config::Config;
use crate::processor::Outcome;
use crate::replace::replace_with_bytes;

pub(crate) fn optimize_jpeg(path: &Path, meta: &Metadata, cfg: &Config) -> io::Result<Outcome> {
    let original = fs::read(path)?;
    let before = original.len() as u64;
    if !cfg.strip {
        return Ok(Outcome::unchanged(before));
    }
    let stripped = strip_jpeg_metadata(&original)?;
    let after = stripped.len() as u64;
    if after >= before {
        return Ok(Outcome::unchanged(before));
    }
    replace_with_bytes(path, &stripped, meta, cfg.keep_mod_time)?;
    Ok(Outcome::changed(before, after))
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Removes APPn and COM segments; pixel data is left untouched.
pub(crate) fn strip_jpeg_metadata(data: &[u8]) -> io::Result<Vec<u8>> {
    if data.len() < 4 || data[0] != 0xff || data[1] != 0xd8 {
        return Err(invalid("invalid JPEG"));
    }
    let seg_len = |i: usize| -> io::Result<usize> {
        if i + 2 > data.len() {
            return Err(invalid("invalid JPEG segment"));
        }
        let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
        if len < 2 || i + len > data.len() {
            return Err(invalid("invalid JPEG segment length"));
        }
        Ok(len)
    };

    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&[0xff, 0xd8]);
    let mut i = 2;
    while i < data.len() {
        if data[i] != 0xff {
            return Err(invalid("invalid JPEG marker stream"));
        }
        let marker_start = i;
        while i < data.len() && data[i] == 0xff {
            i += 1;
        }
        if i >= data.len() {
            break;
        }
        let marker = data[i];
        i += 1;

        match marker {
            0xd9 => {
                out.extend_from_slice(&data[marker_start..i]);
                break;
            }
            0xda => {
                let end = i + seg_len(i)?;
                out.extend_from_slice(&data[marker_start..end]);
                out.extend_from_slice(&data[end..]);
                return Ok(out);
            }
            0x01 | 0xd0..=0xd7 => out.extend_from_slice(&data[marker_start..i]),
            _ => {
                let end = i + seg_len(i)?;
                let is_metadata = (0xe0..=0xef).contains(&marker) || marker == 0xfe;
                if !is_metadata {
                    out.extend_from_slice(&data[marker_start..end]);
                }
                i = end;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_app_and_com() {
        let jpeg = [
            0xff, 0xd8, // SOI
            0xff, 0xe1, 0x00, 0x04, 0xaa, 0xbb, // APP1
            0xff, 0xfe, 0x00, 0x03, 0x41, // COM
            0xff, 0xdb, 0x00, 0x03, 0x01, // DQT
            0xff, 0xda, 0x00, 0x02, 0x11, 0x22, // SOS + scan data
            0xff, 0xd9,
        ];
        let out = strip_jpeg_metadata(&jpeg).unwrap();
        assert_eq!(
            out,
            vec![
                0xff, 0xd8, 0xff, 0xdb, 0x00, 0x03, 0x01, 0xff, 0xda, 0x00, 0x02, 0x11, 0x22, 0xff,
                0xd9
            ]
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(strip_jpeg_metadata(b"nope").is_err());
    }
}
