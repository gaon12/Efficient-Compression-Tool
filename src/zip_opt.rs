use std::fs::{self, File, Metadata};
use std::io::{self, BufReader, Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use zip::write::{FileOptionExtension, FileOptions, SimpleFileOptions};
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::config::{Config, deflate_level};
use crate::processor::Outcome;
use crate::replace::{TempFile, parent_dir};

const ZIP64_THRESHOLD: u64 = 0xFFFF_FFFF;

fn zip_err(e: zip::result::ZipError) -> io::Error {
    match e {
        zip::result::ZipError::Io(e) => e,
        other => io::Error::new(io::ErrorKind::InvalidData, other.to_string()),
    }
}

/// Rewrites an existing ZIP archive, re-deflating every entry.
pub(crate) fn optimize_existing_zip(
    path: &Path,
    meta: &Metadata,
    cfg: &Config,
) -> io::Result<Outcome> {
    let before = meta.len();
    let mut archive = ZipArchive::new(BufReader::new(File::open(path)?)).map_err(zip_err)?;

    let mut tmp = TempFile::new_in(parent_dir(path), "zip")?;
    {
        let mut writer = ZipWriter::new(io::BufWriter::new(tmp.file()));
        let comment = archive.comment().to_vec();
        if !comment.is_empty() {
            writer
                .set_raw_comment(comment.into_boxed_slice())
                .map_err(zip_err)?;
        }
        for i in 0..archive.len() {
            copy_entry(&mut archive, i, &mut writer, cfg)?;
        }
        writer.finish().map_err(zip_err)?.flush()?;
    }
    // Close the source before it is replaced. Rust opens files with
    // FILE_SHARE_DELETE on Windows so the rename would usually succeed
    // anyway, but other processes (antivirus, indexers) and network file
    // systems are less forgiving; never rely on replacing an open file.
    drop(archive);
    let after = tmp.close()?;
    if after >= before {
        return Ok(Outcome::unchanged(before));
    }
    tmp.persist(path, Some(meta), cfg.keep_mod_time)?;
    Ok(Outcome::changed(before, after))
}

fn copy_entry<R: Read + Seek, W: Write + Seek>(
    archive: &mut ZipArchive<R>,
    index: usize,
    writer: &mut ZipWriter<W>,
    cfg: &Config,
) -> io::Result<()> {
    let mut entry = archive.by_index(index).map_err(zip_err)?;
    if entry.encrypted() {
        return Err(io::Error::other("encrypted ZIP entries are not supported"));
    }
    let name = entry.name().map_err(zip_err)?.into_owned();
    let mut options = entry.options().into_full_options();
    if !entry.comment().is_empty() {
        options = options.with_file_comment(entry.comment());
    }

    if entry.is_dir() {
        return writer.add_directory(name, options).map_err(zip_err);
    }

    let mut data = Vec::with_capacity(entry.size().min(1 << 30) as usize);
    entry.read_to_end(&mut data)?;
    let allow_deflate = may_deflate(data.len(), entry.compression(), cfg);
    drop(entry);

    write_entry(writer, &name, options, &data, allow_deflate, cfg)
}

fn apply_method<'k, 'n, T: FileOptionExtension>(
    options: FileOptions<'k, 'n, T>,
    method: CompressionMethod,
    cfg: &Config,
    size: u64,
) -> FileOptions<'k, 'n, T> {
    let options = options
        .compression_method(method)
        .large_file(size >= ZIP64_THRESHOLD);
    if method == CompressionMethod::Deflated {
        options.compression_level(Some(i64::from(deflate_level(cfg.level))))
    } else {
        options.compression_level(None)
    }
}

/// Packs a file or directory into `<path>.zip`, returning (input bytes, output bytes).
pub(crate) fn create_zip_archive(input: &Path, cfg: &Config) -> io::Result<(u64, u64)> {
    let meta = fs::metadata(input)?;
    let clean: PathBuf = input.components().collect();
    let mut out_path = PathBuf::from(clean.as_os_str());
    out_path.as_mut_os_string().push(".zip");
    if out_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "file already exists",
        ));
    }

    let mut input_bytes = 0u64;
    let mut tmp = TempFile::new_in(parent_dir(&out_path), "pack")?;
    {
        let mut writer = ZipWriter::new(io::BufWriter::new(tmp.file()));
        if meta.is_dir() {
            let (dirs, files) = collect_directory_entries(&clean)?;
            let base = clean.parent().map(Path::to_path_buf).unwrap_or_default();
            for dir in &dirs {
                let mut rel = relative_name(&base, dir)?;
                if !rel.ends_with('/') {
                    rel.push('/');
                }
                let mtime = fs::metadata(dir).ok().and_then(|m| m.modified().ok());
                let options = SimpleFileOptions::default().last_modified_time(to_zip_time(mtime));
                writer.add_directory(rel, options).map_err(zip_err)?;
            }
            for file in &files {
                let rel = relative_name(&base, file)?;
                input_bytes += add_file(&mut writer, file, &rel, cfg)?;
            }
        } else {
            let name = clean
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            input_bytes += add_file(&mut writer, &clean, &name, cfg)?;
        }
        writer.finish().map_err(zip_err)?.flush()?;
    }
    let output_bytes = tmp.close()?;
    if out_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "file already exists",
        ));
    }
    tmp.persist(&out_path, Some(&meta), cfg.keep_mod_time)?;
    Ok((input_bytes, output_bytes))
}

fn relative_name(base: &Path, path: &Path) -> io::Result<String> {
    let rel = path
        .strip_prefix(base)
        .map_err(|e| io::Error::other(e.to_string()))?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Ok(parts.join("/"))
}

fn collect_directory_entries(root: &Path) -> io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                // Never follow links while packing: they could point outside
                // the directory being archived (or form a cycle).
                continue;
            }
            if file_type.is_dir() {
                dirs.push(path.clone());
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    dirs.sort();
    files.sort();
    Ok((dirs, files))
}

fn add_file<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    src: &Path,
    name: &str,
    cfg: &Config,
) -> io::Result<u64> {
    let data = fs::read(src)?;
    let meta = fs::metadata(src)?;
    let options = file_permissions(
        SimpleFileOptions::default().last_modified_time(to_zip_time(meta.modified().ok())),
        &meta,
    );
    let allow_deflate = may_deflate(data.len(), CompressionMethod::Deflated, cfg);
    write_entry(writer, name, options, &data, allow_deflate, cfg)?;
    Ok(data.len() as u64)
}

/// Records the Unix permission bits of a packed file (no-op elsewhere).
#[cfg(unix)]
fn file_permissions(options: SimpleFileOptions, meta: &Metadata) -> SimpleFileOptions {
    use std::os::unix::fs::PermissionsExt;
    options.unix_permissions(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn file_permissions(options: SimpleFileOptions, _meta: &Metadata) -> SimpleFileOptions {
    options
}

/// Entries smaller than this are always stored: the deflate framing
/// overhead cannot pay off for such tiny inputs.
const MIN_DEFLATE_SIZE: usize = 64;

/// Whether an entry originally written with `original` may be (re)written
/// with Deflate:
/// - tiny entries are always stored,
/// - stored entries stay stored in strict mode,
/// - entries using other methods are stored after decompression.
pub(crate) fn may_deflate(size: usize, original: CompressionMethod, cfg: &Config) -> bool {
    if size < MIN_DEFLATE_SIZE {
        return false;
    }
    match original {
        CompressionMethod::Stored => !cfg.strict,
        CompressionMethod::Deflated => true,
        _ => false,
    }
}

/// Writes `data` as one entry, compressing it at most once.
///
/// The data is deflated into a single-entry in-memory archive. If that is
/// smaller than the input, the compressed entry is copied into `writer`
/// with `raw_copy_file` (no second compression pass); otherwise the entry
/// is stored. The previous implementation deflated every entry twice: once
/// to measure the size and once more inside `ZipWriter`.
fn write_entry<W: Write + Seek, T: FileOptionExtension + Clone>(
    writer: &mut ZipWriter<W>,
    name: &str,
    options: FileOptions<'_, '_, T>,
    data: &[u8],
    allow_deflate: bool,
    cfg: &Config,
) -> io::Result<()> {
    let size = data.len() as u64;
    if allow_deflate {
        let deflated = apply_method(options.clone(), CompressionMethod::Deflated, cfg, size);
        let mut staging = ZipWriter::new(Cursor::new(Vec::with_capacity(data.len() / 2 + 256)));
        staging.start_file(name, deflated).map_err(zip_err)?;
        staging.write_all(data)?;
        let mut staged = ZipArchive::new(staging.finish().map_err(zip_err)?).map_err(zip_err)?;
        let entry = staged.by_index_raw(0).map_err(zip_err)?;
        if entry.compressed_size() < size {
            return writer.raw_copy_file(entry).map_err(zip_err);
        }
    }

    let stored = apply_method(options, CompressionMethod::Stored, cfg, size);
    writer.start_file(name, stored).map_err(zip_err)?;
    writer.write_all(data)
}

/// Converts a timestamp to a ZIP (MS-DOS, UTC) date/time.
fn to_zip_time(t: Option<SystemTime>) -> DateTime {
    let Some(secs) = t
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
    else {
        return DateTime::default();
    };
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    if !(1980..=2107).contains(&year) {
        return DateTime::default();
    }
    let date = ((year - 1980) as u16) << 9 | (month as u16) << 5 | day as u16;
    let time =
        ((rem / 3600) as u16) << 11 | (((rem % 3600) / 60) as u16) << 5 | ((rem % 60) / 2) as u16;
    DateTime::try_from_msdos(date, time).unwrap_or_default()
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn deflate_eligibility() {
        let cfg = Config::default();
        let strict = Config {
            strict: true,
            ..Config::default()
        };
        assert!(!may_deflate(10, CompressionMethod::Deflated, &cfg));
        assert!(may_deflate(1000, CompressionMethod::Stored, &cfg));
        assert!(!may_deflate(1000, CompressionMethod::Stored, &strict));
        assert!(may_deflate(1000, CompressionMethod::Deflated, &strict));
    }

    fn write_one(data: &[u8], allow_deflate: bool) -> (CompressionMethod, Vec<u8>) {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        write_entry(&mut w, "x", opts, data, allow_deflate, &Config::default()).unwrap();
        let mut ar = ZipArchive::new(w.finish().unwrap()).unwrap();
        let mut f = ar.by_index(0).unwrap();
        let mut out = Vec::new();
        f.read_to_end(&mut out).unwrap();
        (f.compression(), out)
    }

    #[test]
    fn write_entry_deflates_only_when_smaller() {
        let text = vec![b'a'; 4096];
        assert_eq!(
            write_one(&text, true),
            (CompressionMethod::Deflated, text.clone())
        );
        assert_eq!(write_one(&text, false), (CompressionMethod::Stored, text));

        // Pseudo-random bytes do not compress and must be stored.
        let mut x = 0x2545_f491_u32;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        assert_eq!(write_one(&noise, true), (CompressionMethod::Stored, noise));
    }
}
