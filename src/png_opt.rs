use std::fs::{self, Metadata};
use std::io::{self, Cursor};
use std::path::Path;

use png::{DeflateCompression, Filter};

use crate::config::Config;
use crate::processor::Outcome;
use crate::replace::replace_with_bytes;

/// Upper bound for the decoded pixel buffer (1 GiB).
const MAX_DECODED_BYTES: usize = 1 << 30;

/// Every scanline filter strategy the encoder supports.
const ALL_FILTERS: [Filter; 7] = [
    Filter::Adaptive,
    Filter::MinEntropy,
    Filter::NoFilter,
    Filter::Sub,
    Filter::Up,
    Filter::Avg,
    Filter::Paeth,
];

/// Filters that win most often; tried at the medium levels.
const COMMON_FILTERS: [Filter; 4] = [
    Filter::Adaptive,
    Filter::MinEntropy,
    Filter::Up,
    Filter::Paeth,
];

fn to_io(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Losslessly re-encodes a PNG (same pixel format, palette and transparency),
/// keeping the result only if it is smaller.
pub(crate) fn optimize_png(path: &Path, meta: &Metadata, cfg: &Config) -> io::Result<Outcome> {
    let original = fs::read(path)?;
    let before = original.len() as u64;
    let encoded = reencode_png(&original, cfg.level)?;
    let after = encoded.len() as u64;
    if after >= before {
        return Ok(Outcome::unchanged(before));
    }
    replace_with_bytes(path, &encoded, meta, cfg.keep_mod_time)?;
    Ok(Outcome::changed(before, after))
}

/// The (filter, zlib level) combinations tried for an ECT level.
///
/// No single filter is best for every image (measured: `Up` beat `Adaptive`
/// by 15% on a noisy pattern, `MinEntropy` won on an RGBA icon), and zlib
/// level 9 is occasionally *larger* than level 6. Higher ECT levels therefore
/// try more combinations and keep the smallest output:
/// - 1:   adaptive filtering at zlib level 6 (one encode),
/// - 2-4: the four most successful filters at level 6,
/// - 5-7: all seven filters at level 6,
/// - 8-9: all seven filters at levels 6 and 9.
fn candidates(level: u32) -> Vec<(Filter, u8)> {
    let (filters, zlib_levels): (&[Filter], &[u8]) = match level {
        0 | 1 => (&[Filter::Adaptive], &[6]),
        2..=4 => (&COMMON_FILTERS, &[6]),
        5..=7 => (&ALL_FILTERS, &[6]),
        _ => (&ALL_FILTERS, &[6, 9]),
    };
    zlib_levels
        .iter()
        .flat_map(|&z| filters.iter().map(move |&f| (f, z)))
        .collect()
}

/// Decoded image plus the header fields needed to write it again.
struct Image {
    width: u32,
    height: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    palette: Option<Vec<u8>>,
    trns: Option<Vec<u8>>,
    pixels: Vec<u8>,
}

/// Re-encodes `data` with every candidate for `level` and returns the
/// smallest encoding (which may still be larger than `data`; the caller
/// decides whether to keep it).
pub(crate) fn reencode_png(data: &[u8], level: u32) -> io::Result<Vec<u8>> {
    let Some(image) = decode(data)? else {
        // APNG: re-encoding only the first frame would lose data.
        return Ok(data.to_vec());
    };
    let mut best: Option<Vec<u8>> = None;
    for (filter, zlib_level) in candidates(level) {
        let encoded = encode(&image, filter, zlib_level)?;
        if best.as_ref().is_none_or(|b| encoded.len() < b.len()) {
            best = Some(encoded);
        }
    }
    Ok(best.expect("at least one candidate"))
}

/// Decodes the first (and only) frame; returns `None` for animated PNGs.
fn decode(data: &[u8]) -> io::Result<Option<Image>> {
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
        return Ok(None);
    }
    let mut image = Image {
        width: info.width,
        height: info.height,
        color: info.color_type,
        depth: info.bit_depth,
        palette: info.palette.as_ref().map(|p| p.to_vec()),
        trns: info.trns.as_ref().map(|t| t.to_vec()),
        pixels: Vec::new(),
    };

    let size = reader
        .output_buffer_size()
        .ok_or_else(|| to_io("PNG image is too large"))?;
    image.pixels = vec![0u8; size];
    let frame = reader.next_frame(&mut image.pixels).map_err(to_io)?;
    image.pixels.truncate(frame.buffer_size());
    Ok(Some(image))
}

fn encode(image: &Image, filter: Filter, zlib_level: u8) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(image.pixels.len() / 2);
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(image.color);
    encoder.set_depth(image.depth);
    encoder.set_deflate_compression(DeflateCompression::Level(zlib_level));
    encoder.set_filter(filter);
    if let Some(p) = &image.palette {
        encoder.set_palette(p.as_slice());
    }
    if let Some(t) = &image.trns {
        encoder.set_trns(t.as_slice());
    }
    let mut writer = encoder.write_header().map_err(to_io)?;
    writer.write_image_data(&image.pixels).map_err(to_io)?;
    writer.finish().map_err(to_io)?;
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

    fn decode_pixels(data: &[u8]) -> Vec<u8> {
        let mut r = png::Decoder::new(Cursor::new(data)).read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let n = r.next_frame(&mut buf).unwrap().buffer_size();
        buf.truncate(n);
        buf
    }

    #[test]
    fn reencode_is_lossless_and_smaller_at_every_level() {
        let input = make_png(png::Compression::NoCompression);
        for level in 1..=9 {
            let output = reencode_png(&input, level).unwrap();
            assert!(output.len() < input.len(), "level {level}");
            assert_eq!(
                decode_pixels(&input),
                decode_pixels(&output),
                "level {level}"
            );
        }
    }

    #[test]
    fn higher_levels_never_produce_larger_output() {
        let input = make_png(png::Compression::Fast);
        let sizes: Vec<usize> = [1, 3, 6, 9]
            .iter()
            .map(|&l| reencode_png(&input, l).unwrap().len())
            .collect();
        assert!(sizes.windows(2).all(|w| w[1] <= w[0]), "{sizes:?}");
    }

    #[test]
    fn candidate_counts_grow_with_level() {
        assert_eq!(candidates(1).len(), 1);
        assert_eq!(candidates(3).len(), 4);
        assert_eq!(candidates(6).len(), 7);
        assert_eq!(candidates(9).len(), 14);
    }
}
