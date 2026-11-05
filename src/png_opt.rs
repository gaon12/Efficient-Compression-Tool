use std::fs::{self, Metadata};
use std::io::{self, Cursor};
use std::path::Path;

use png::{DeflateCompression, Filter};

use crate::config::Config;
use crate::processor::Outcome;
use crate::replace::replace_with_bytes;

/// Upper bound for the decoded pixel buffer (1 GiB).
const MAX_DECODED_BYTES: usize = 1 << 30;

/// Largest canvas accepted (256 megapixels), the same limit as the C/C++ and
/// Go versions. Checked before the pixel buffer is allocated, so a few
/// hundred bytes cannot demand gigabytes of memory.
const MAX_PIXELS: u64 = 1 << 28;

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
    // The png crate skips CRC checks of ancillary chunks, so a damaged tRNS
    // or text chunk would be re-encoded with a fresh, valid CRC and the
    // damage would become invisible. Verify every chunk up front.
    parse_chunks(&original)?;
    let encoded = reencode_png(&original, cfg.level)?;
    // The encoder writes no metadata: put gAMA/iCCP/pHYs/text/... back
    // unless -strip was given.
    let encoded = if cfg.strip {
        encoded
    } else {
        copy_png_metadata(&original, &encoded)?
    };
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
        // The decoder normalizes an 8-bit RGB/gray color key to 8-bit
        // samples (3 or 1 bytes), but the encoder writes `trns` verbatim and
        // the PNG spec requires 16-bit samples there (6 or 2 bytes). Copy the
        // chunk payload from the file instead so it round-trips exactly.
        trns: raw_chunk(data, b"tRNS").map(<[u8]>::to_vec),
        pixels: Vec::new(),
    };

    let pixels = u64::from(info.width) * u64::from(info.height);
    if pixels > MAX_PIXELS {
        return Err(to_io(format!(
            "PNG dimensions {}x{} exceed the {MAX_PIXELS} pixel limit",
            info.width, info.height
        )));
    }
    let size = reader
        .output_buffer_size()
        .filter(|&s| s <= MAX_DECODED_BYTES)
        .ok_or_else(|| to_io("PNG image is too large"))?;
    // try_reserve reports allocation failure as an error instead of aborting
    // the whole process.
    image.pixels.try_reserve_exact(size).map_err(to_io)?;
    image.pixels.resize(size, 0);
    let frame = reader.next_frame(&mut image.pixels).map_err(to_io)?;
    image.pixels.truncate(frame.buffer_size());
    if image.color == png::ColorType::Indexed {
        let entries = image.palette.as_ref().map_or(0, |p| p.len() / 3);
        if !indices_in_range(
            &image.pixels,
            image.width,
            image.depth as u8,
            frame.line_size,
            entries,
        ) {
            return Err(to_io("palette index out of range of the PLTE chunk"));
        }
    }
    Ok(Some(image))
}

/// True if every palette index in the (deinterlaced) indexed pixel rows is
/// below `entries`. Out-of-range indices make a PNG invalid; decoders differ
/// in how they show them, so such files are left alone.
fn indices_in_range(pixels: &[u8], width: u32, bits: u8, line_size: usize, entries: usize) -> bool {
    if entries >= 1usize << bits || line_size == 0 {
        return true;
    }
    let width = width as usize;
    let bits = usize::from(bits);
    let mask = (1usize << bits) - 1;
    pixels.chunks(line_size).all(|row| {
        (0..width).all(|x| {
            let bit = x * bits;
            let Some(&byte) = row.get(bit / 8) else {
                return true;
            };
            let index = (usize::from(byte) >> (8 - bits - bit % 8)) & mask;
            index < entries
        })
    })
}

/// Ancillary chunks that must precede PLTE; placed right after IHDR.
const EARLY_CHUNKS: [&[u8; 4]; 8] = [
    b"cHRM", b"gAMA", b"iCCP", b"sBIT", b"sRGB", b"cICP", b"mDCV", b"cLLI",
];

/// Ancillary chunks defined by the PNG specification. Other (private)
/// chunks are copied only if their "safe to copy" bit is set, as the
/// specification requires once the image data has been rewritten.
const KNOWN_ANCILLARY: [&[u8; 4]; 21] = [
    b"cHRM", b"gAMA", b"iCCP", b"sBIT", b"sRGB", b"cICP", b"mDCV", b"cLLI", b"bKGD", b"hIST",
    b"pHYs", b"sPLT", b"eXIf", b"tIME", b"tEXt", b"zTXt", b"iTXt", b"oFFs", b"pCAL", b"sCAL",
    b"sTER",
];

/// Splits a PNG stream into (type, payload) chunks up to and including IEND,
/// verifying every chunk's CRC.
fn parse_chunks(data: &[u8]) -> io::Result<Vec<([u8; 4], &[u8])>> {
    let bad = || to_io("malformed PNG chunk structure");
    let mut chunks = Vec::new();
    let mut pos = 8;
    while pos + 12 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().map_err(|_| bad())?) as usize;
        let kind: [u8; 4] = data[pos + 4..pos + 8].try_into().map_err(|_| bad())?;
        let body = data.get(pos + 8..pos + 8 + len).ok_or_else(bad)?;
        let stored = data.get(pos + 8 + len..pos + 12 + len).ok_or_else(bad)?;
        let mut crc = flate2::Crc::new();
        crc.update(&kind);
        crc.update(body);
        if crc.sum().to_be_bytes() != stored {
            return Err(to_io(format!(
                "CRC error in the {} chunk",
                String::from_utf8_lossy(&kind)
            )));
        }
        chunks.push((kind, body));
        pos += 12 + len;
        if &kind == b"IEND" {
            return Ok(chunks);
        }
    }
    Err(bad())
}

/// Returns `encoded` with the ancillary chunks of `original` re-inserted
/// (the encoder writes only IHDR, PLTE, tRNS, IDAT and IEND). Chunk order
/// rules are kept: color-space chunks go right after IHDR, chunks that
/// preceded IDAT go before IDAT, chunks that followed it go before IEND.
fn copy_png_metadata(original: &[u8], encoded: &[u8]) -> io::Result<Vec<u8>> {
    let orig = parse_chunks(original)?;
    let enc = parse_chunks(encoded)?;

    let (mut early, mut before_idat, mut after_idat) = (Vec::new(), Vec::new(), Vec::new());
    let mut seen_idat = false;
    for &(kind, body) in &orig {
        match &kind {
            b"IHDR" | b"PLTE" | b"tRNS" | b"IEND" => continue,
            b"IDAT" => {
                seen_idat = true;
                continue;
            }
            _ => {}
        }
        let ancillary = kind[0] & 0x20 != 0;
        let safe_to_copy = kind[3] & 0x20 != 0;
        if !ancillary || (!KNOWN_ANCILLARY.contains(&&kind) && !safe_to_copy) {
            continue;
        }
        if EARLY_CHUNKS.contains(&&kind) {
            early.push((kind, body));
        } else if seen_idat {
            after_idat.push((kind, body));
        } else {
            before_idat.push((kind, body));
        }
    }
    if early.is_empty() && before_idat.is_empty() && after_idat.is_empty() {
        return Ok(encoded.to_vec());
    }

    let mut out = Vec::with_capacity(encoded.len() + original.len() / 4);
    out.extend_from_slice(&encoded[..8]);
    let mut write = |kind: &[u8; 4], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(body);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    };
    let mut inserted = false;
    for &(kind, body) in &enc {
        match &kind {
            b"IDAT" if !inserted => {
                before_idat.iter().for_each(|(k, b)| write(k, b));
                inserted = true;
            }
            b"IEND" => after_idat.iter().for_each(|(k, b)| write(k, b)),
            _ => {}
        }
        write(&kind, body);
        if &kind == b"IHDR" {
            early.iter().for_each(|(k, b)| write(k, b));
        }
    }
    Ok(out)
}

/// Returns the payload of the first `kind` chunk before IDAT, if any.
fn raw_chunk<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut pos = 8; // PNG signature
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
        let name = &data[pos + 4..pos + 8];
        if name == b"IDAT" {
            return None;
        }
        let body = data.get(pos + 8..pos + 8 + len)?;
        if name == kind {
            return Some(body);
        }
        pos = pos + 12 + len;
    }
    None
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
