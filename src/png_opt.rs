use std::fs::{self, Metadata};
use std::io::{self, Cursor};
use std::path::Path;

use crate::config::Config;
use crate::processor::Outcome;
use crate::replace::replace_with_bytes;

/// Upper bound for the decoded pixel buffer (1 GiB).
const MAX_DECODED_BYTES: usize = 1 << 30;

fn to_io(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Losslessly re-encodes a PNG (same pixel format, palette and transparency),
/// keeping the result only if it is smaller.
pub(crate) fn optimize_png(path: &Path, meta: &Metadata, cfg: &Config) -> io::Result<Outcome> {
    let original = fs::read(path)?;
    let before = original.len() as u64;
    let encoded = reencode_png(&original)?;
    let after = encoded.len() as u64;
    if after >= before {
        return Ok(Outcome::unchanged(before));
    }
    replace_with_bytes(path, &encoded, meta, cfg.keep_mod_time)?;
    Ok(Outcome::changed(before, after))
}

pub(crate) fn reencode_png(data: &[u8]) -> io::Result<Vec<u8>> {
    // Bound the decoded image size so a tiny, maliciously crafted PNG cannot
    // force a multi-gigabyte allocation.
    let limits = png::Limits {
        bytes: MAX_DECODED_BYTES,
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(data), limits);
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().map_err(to_io)?;

    let info = reader.info();
    if info.animation_control.is_some() {
        // APNG: re-encoding only the first frame would lose data.
        return Ok(data.to_vec());
    }
    let (width, height) = (info.width, info.height);
    let color = info.color_type;
    let depth = info.bit_depth;
    let palette = info.palette.as_ref().map(|p| p.to_vec());
    let trns = info.trns.as_ref().map(|t| t.to_vec());

    let size = reader
        .output_buffer_size()
        .ok_or_else(|| to_io("PNG image is too large"))?;
    let mut pixels = vec![0u8; size];
    let frame = reader.next_frame(&mut pixels).map_err(to_io)?;
    pixels.truncate(frame.buffer_size());

    let mut out = Vec::with_capacity(data.len());
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        encoder.set_compression(png::Compression::High);
        if let Some(p) = palette {
            encoder.set_palette(p);
        }
        if let Some(t) = trns {
            encoder.set_trns(t);
        }
        let mut writer = encoder.write_header().map_err(to_io)?;
        writer.write_image_data(&pixels).map_err(to_io)?;
        writer.finish().map_err(to_io)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_png(compression: png::Compression) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 64, 64);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(compression);
        let mut w = enc.write_header().unwrap();
        let data: Vec<u8> = (0..64 * 64 * 3).map(|i| (i / 3 % 64) as u8).collect();
        w.write_image_data(&data).unwrap();
        w.finish().unwrap();
        out
    }

    fn decode(data: &[u8]) -> Vec<u8> {
        let mut r = png::Decoder::new(Cursor::new(data)).read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let n = r.next_frame(&mut buf).unwrap().buffer_size();
        buf.truncate(n);
        buf
    }

    #[test]
    fn reencode_is_lossless_and_smaller() {
        let input = make_png(png::Compression::NoCompression);
        let output = reencode_png(&input).unwrap();
        assert!(output.len() < input.len());
        assert_eq!(decode(&input), decode(&output));
    }
}
