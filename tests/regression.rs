//! Regression tests for problems found by the cross-implementation
//! conformance harness.

use std::fs;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use ect::Config;

fn scratch_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("ect-rs-regress-{}-{name}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn quiet() -> Config {
    Config {
        quiet: true,
        ..Config::default()
    }
}

/// An uncompressed 8-bit RGB gradient, optionally with a tRNS color key.
fn rgb_png(trns: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 32, 32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::NoCompression);
    if let Some(t) = trns {
        enc.set_trns(t);
    }
    let mut w = enc.write_header().unwrap();
    let data: Vec<u8> = (0..32 * 32 * 3).map(|i| (i / 3 % 32 * 8) as u8).collect();
    w.write_image_data(&data).unwrap();
    w.finish().unwrap();
    out
}

/// (type, payload) of every chunk.
fn chunks(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pos = 8;
    while pos + 12 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let kind = String::from_utf8_lossy(&data[pos + 4..pos + 8]).into_owned();
        out.push((kind, data[pos + 8..pos + 8 + len].to_vec()));
        pos += 12 + len;
    }
    out
}

/// Inserts a chunk right after IHDR (IHDR is always 25 bytes into the file).
fn insert_chunk(data: &[u8], kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let at = 8 + 25;
    let mut out = data[..at].to_vec();
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(body);
    out.extend_from_slice(&crc.sum().to_be_bytes());
    out.extend_from_slice(&data[at..]);
    out
}

#[test]
fn rgb_color_key_survives_reencoding() {
    // The decoder reports an 8-bit color key as 3 bytes; the tRNS chunk
    // must still be written with 16-bit samples (6 bytes).
    let trns = [0u8, 10, 0, 20, 0, 30];
    let dir = scratch_dir("trns");
    let path = dir.join("a.png");
    fs::write(&path, rgb_png(Some(&trns))).unwrap();

    let report = ect::process(quiet(), &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert!(report.saved_bytes > 0);
    let out = fs::read(&path).unwrap();
    let got = chunks(&out).into_iter().find(|(k, _)| k == "tRNS");
    assert_eq!(got.map(|(_, d)| d), Some(trns.to_vec()));
}

#[test]
fn png_metadata_kept_unless_strip() {
    let mut src = rgb_png(None);
    src = insert_chunk(&src, b"tEXt", b"Comment\0hello");
    src = insert_chunk(&src, b"pHYs", &[0, 0, 0x0b, 0x13, 0, 0, 0x0b, 0x13, 1]);
    src = insert_chunk(&src, b"prVT", b"private, unsafe to copy");
    src = insert_chunk(&src, b"gAMA", &[0, 0, 0xb1, 0x8f]);
    let dir = scratch_dir("meta");
    let path = dir.join("a.png");
    fs::write(&path, &src).unwrap();

    let report = ect::process(quiet(), &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    let kinds: Vec<String> = chunks(&fs::read(&path).unwrap())
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(kinds, ["IHDR", "gAMA", "pHYs", "tEXt", "IDAT", "IEND"]);

    fs::write(&path, &src).unwrap();
    let strip = Config {
        strip: true,
        ..quiet()
    };
    let report = ect::process(strip, &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    let kinds: Vec<String> = chunks(&fs::read(&path).unwrap())
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(kinds, ["IHDR", "IDAT", "IEND"]);
}

#[test]
fn palette_index_out_of_range_is_rejected() {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 8, 8);
    enc.set_color(png::ColorType::Indexed);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::NoCompression);
    enc.set_palette(vec![255u8, 0, 0, 0, 255, 0]); // two entries
    let mut w = enc.write_header().unwrap();
    let mut data = vec![0u8; 64];
    data[10] = 1;
    data[20] = 5; // past the end of PLTE
    w.write_image_data(&data).unwrap();
    w.finish().unwrap();

    let dir = scratch_dir("palette");
    let path = dir.join("bad.png");
    fs::write(&path, &out).unwrap();
    let report = ect::process(quiet(), &[&path]);
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(fs::read(&path).unwrap(), out);
}

#[test]
fn huge_png_dimensions_are_an_error_not_an_abort() {
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&100_000u32.to_be_bytes());
    ihdr.extend_from_slice(&100_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut src = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, body) in [
        (b"IHDR", ihdr.as_slice()),
        (b"IDAT", &[][..]),
        (b"IEND", &[][..]),
    ] {
        src.extend_from_slice(&(body.len() as u32).to_be_bytes());
        src.extend_from_slice(kind);
        src.extend_from_slice(body);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(body);
        src.extend_from_slice(&crc.sum().to_be_bytes());
    }
    let dir = scratch_dir("bomb");
    let path = dir.join("bomb.png");
    fs::write(&path, &src).unwrap();
    let report = ect::process(quiet(), &[&path]);
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
}

#[test]
fn gzip_mode_compresses_images_to_gz() {
    let dir = scratch_dir("gzpng");
    let path = dir.join("a.png");
    let src = rgb_png(None);
    fs::write(&path, &src).unwrap();
    let cfg = Config {
        gzip_mode: true,
        ..quiet()
    };
    let report = ect::process(cfg, &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert!(dir.join("a.png.gz").exists());
    assert_eq!(fs::read(&path).unwrap(), src);
}

#[test]
fn zip_with_duplicate_names_is_left_unchanged() {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, text) in [("aaa.txt", "first "), ("bbb.txt", "second ")] {
        w.start_file(name, opts).unwrap();
        w.write_all(text.repeat(200).as_bytes()).unwrap();
    }
    let mut data = w.finish().unwrap().into_inner();
    // Rename bbb.txt to aaa.txt in both headers (names are not covered by
    // the CRC, so the archive stays otherwise valid).
    let mut i = 0;
    while let Some(p) = data[i..].windows(7).position(|w| w == b"bbb.txt") {
        data[i + p..i + p + 3].copy_from_slice(b"aaa");
        i += p + 7;
    }

    let dir = scratch_dir("dupes");
    let path = dir.join("d.zip");
    fs::write(&path, &data).unwrap();
    let cfg = Config {
        zip_mode: true,
        ..quiet()
    };
    let report = ect::process(cfg, &[&path]);
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(fs::read(&path).unwrap(), data);
}
