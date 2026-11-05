//! End-to-end tests that run the public `ect::process` API against real files
//! in a temporary directory.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ect::Config;

/// Creates a fresh, empty directory under the system temp dir.
fn scratch_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ect-rs-test-{}-{name}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn compressible_text() -> Vec<u8> {
    "Efficient Compression Tool test payload. "
        .repeat(500)
        .into_bytes()
}

fn quiet() -> Config {
    Config {
        quiet: true,
        ..Config::default()
    }
}

#[test]
fn gzip_mode_creates_gz_and_roundtrips() {
    let dir = scratch_dir("gzip");
    let src = dir.join("data.txt");
    fs::write(&src, compressible_text()).unwrap();

    let cfg = Config {
        gzip_mode: true,
        ..quiet()
    };
    let report = ect::process(cfg.clone(), &[&src]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert_eq!(report.processed_files, 1);
    assert!(report.saved_bytes > 0);

    let gz = dir.join("data.txt.gz");
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(fs::File::open(&gz).unwrap())
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, compressible_text());

    // A second run must refuse to overwrite the existing .gz output.
    let again = ect::process(cfg, &[&src]);
    assert!(again.has_errors());
}

#[test]
fn existing_gzip_is_recompressed_losslessly() {
    let dir = scratch_dir("regzip");
    let path = dir.join("weak.gz");
    let mut enc = flate2::GzBuilder::new()
        .filename("weak.txt")
        .write(Vec::new(), flate2::Compression::none());
    enc.write_all(&compressible_text()).unwrap();
    fs::write(&path, enc.finish().unwrap()).unwrap();

    let cfg = Config {
        gzip_mode: true,
        level: 9,
        ..quiet()
    };
    let report = ect::process(cfg, &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert!(report.saved_bytes > 0);

    let mut dec = flate2::read::GzDecoder::new(fs::File::open(&path).unwrap());
    let mut decoded = Vec::new();
    dec.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, compressible_text());
    assert_eq!(dec.header().unwrap().filename(), Some(&b"weak.txt"[..]));
}

fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn read_zip(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    (0..archive.len())
        .map(|i| {
            let mut f = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap();
            (f.name().unwrap().into_owned(), data)
        })
        .collect()
}

#[test]
fn stored_zip_entries_are_deflated_unless_strict() {
    let text = compressible_text();
    let dir = scratch_dir("zip");
    let path = dir.join("a.zip");
    fs::write(&path, stored_zip(&[("a.txt", &text), ("tiny", b"x")])).unwrap();

    // --strict keeps stored entries stored, so nothing shrinks.
    let strict = Config {
        strict: true,
        ..quiet()
    };
    let report = ect::process(strict, &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert_eq!(report.saved_bytes, 0);

    let report = ect::process(quiet(), &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert!(report.saved_bytes > 0);
    assert_eq!(
        read_zip(&path),
        vec![
            ("a.txt".to_string(), text),
            ("tiny".to_string(), b"x".to_vec())
        ]
    );
}

#[test]
fn zip_mode_packs_directory() {
    let dir = scratch_dir("pack");
    let root = dir.join("bundle");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("one.txt"), compressible_text()).unwrap();
    fs::write(root.join("sub").join("two.txt"), b"two").unwrap();

    let cfg = Config {
        zip_mode: true,
        gzip_mode: true,
        ..quiet()
    };
    let report = ect::process(cfg, &[&root]);
    assert!(!report.has_errors(), "{:?}", report.errors);

    let names: Vec<String> = read_zip(&dir.join("bundle.zip"))
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(
        names,
        vec!["bundle/sub/", "bundle/one.txt", "bundle/sub/two.txt"]
    );
}

#[test]
fn png_is_reencoded_losslessly() {
    let dir = scratch_dir("png");
    let path = dir.join("img.png");
    let pixels: Vec<u8> = (0..128 * 128 * 4).map(|i| (i / 4 % 128) as u8).collect();
    let mut raw = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut raw, 128, 128);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::NoCompression);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&pixels).unwrap();
    }
    fs::write(&path, &raw).unwrap();

    let report = ect::process(quiet(), &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert!(report.saved_bytes > 0);

    let data = fs::read(&path).unwrap();
    let mut reader = png::Decoder::new(Cursor::new(data)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let n = reader.next_frame(&mut buf).unwrap().buffer_size();
    assert_eq!(&buf[..n], &pixels[..]);
}

#[test]
fn directories_require_recurse() {
    let dir = scratch_dir("recurse");
    fs::write(dir.join("x.txt"), b"x").unwrap();
    let report = ect::process(quiet(), &[&dir]);
    assert_eq!(report.errors.len(), 1);
}

#[cfg(unix)]
#[test]
fn symlinks_are_never_replaced() {
    let dir = scratch_dir("symlink");
    let target = dir.join("real.txt");
    fs::write(&target, compressible_text()).unwrap();
    let link = dir.join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let cfg = Config {
        gzip_mode: true,
        recursive: true,
        ..quiet()
    };
    // Explicit symlink argument: reported as an error, link left untouched.
    let report = ect::process(cfg.clone(), &[&link]);
    assert_eq!(report.errors.len(), 1);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    // During directory traversal links are silently skipped.
    let report = ect::process(cfg, &[&dir]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert!(!dir.join("link.txt.gz").exists());
    assert!(dir.join("real.txt.gz").exists());
}

#[test]
fn zip_optimization_preserves_modification_time() {
    let modified = zip::DateTime::from_date_and_time(2021, 6, 15, 10, 30, 42).unwrap();
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(modified);
    w.start_file("a.txt", opts).unwrap();
    w.write_all(&compressible_text()).unwrap();
    let dir = scratch_dir("zip-mtime");
    let path = dir.join("ts.zip");
    fs::write(&path, w.finish().unwrap().into_inner()).unwrap();

    let report = ect::process(quiet(), &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert!(report.saved_bytes > 0);

    let mut archive = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    let entry = archive.by_index(0).unwrap();
    assert_eq!(entry.compression(), zip::CompressionMethod::Deflated);
    assert_eq!(entry.last_modified(), Some(modified));
}

#[test]
fn zip_mode_reoptimizes_existing_archive_in_place() {
    // Same semantics as the C/C++ CLI: `ect -zip archive.zip` rewrites the
    // archive instead of creating `archive.zip.zip`.
    let text = compressible_text();
    let dir = scratch_dir("zip-in-place");
    let path = dir.join("a.zip");
    fs::write(&path, stored_zip(&[("a.txt", &text)])).unwrap();
    let before = fs::metadata(&path).unwrap().len();

    let cfg = Config {
        zip_mode: true,
        gzip_mode: true,
        ..quiet()
    };
    let report = ect::process(cfg, &[&path]);
    assert!(!report.has_errors(), "{:?}", report.errors);
    assert!(fs::metadata(&path).unwrap().len() < before);
    assert!(!dir.join("a.zip.zip").exists());
    assert_eq!(read_zip(&path), vec![("a.txt".to_string(), text)]);
}
