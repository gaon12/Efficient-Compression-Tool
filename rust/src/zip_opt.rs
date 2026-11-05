use std::fs::{self, File, Metadata};
use std::io::{self, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::write::DeflateEncoder;
use zip::write::{FileOptionExtension, FileOptions, SimpleFileOptions};
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::config::Config;
use crate::gzip::compression;
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
    let method = choose_zip_method(&data, entry.compression(), cfg)?;
    drop(entry);

    let options = apply_method(options, method, cfg, data.len() as u64);
    writer.start_file(name, options).map_err(zip_err)?;
    writer.write_all(&data)
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
        options.compression_level(Some(i64::from(cfg.level.clamp(1, 9))))
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
    let method = choose_zip_method(&data, CompressionMethod::Deflated, cfg)?;
    let mut options =
        SimpleFileOptions::default().last_modified_time(to_zip_time(meta.modified().ok()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        options = options.unix_permissions(meta.permissions().mode() & 0o7777);
    }
    options = apply_method(options, method, cfg, data.len() as u64);
    writer.start_file(name, options).map_err(zip_err)?;
    writer.write_all(&data)?;
    Ok(data.len() as u64)
}

pub(crate) fn choose_zip_method(
    data: &[u8],
    original: CompressionMethod,
    cfg: &Config,
) -> io::Result<CompressionMethod> {
    if data.len() < 64 {
        return Ok(CompressionMethod::Stored);
    }
    let try_deflate = match original {
        CompressionMethod::Stored => !cfg.strict,
        CompressionMethod::Deflated => true,
        _ => false,
    };
    if try_deflate && deflated_size(data, cfg.level)? < data.len() as u64 {
        Ok(CompressionMethod::Deflated)
    } else {
        Ok(CompressionMethod::Stored)
    }
}

struct CountingWriter(u64);

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn deflated_size(data: &[u8], level: u32) -> io::Result<u64> {
    let mut enc = DeflateEncoder::new(CountingWriter(0), compression(level));
    enc.write_all(data)?;
    Ok(enc.finish()?.0)
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
    fn method_choice() {
        let cfg = Config::default();
        let compressible = vec![b'a'; 1000];
        assert_eq!(
            choose_zip_method(b"tiny", CompressionMethod::Deflated, &cfg).unwrap(),
            CompressionMethod::Stored
        );
        assert_eq!(
            choose_zip_method(&compressible, CompressionMethod::Stored, &cfg).unwrap(),
            CompressionMethod::Deflated
        );
        let strict = Config {
            strict: true,
            ..Config::default()
        };
        assert_eq!(
            choose_zip_method(&compressible, CompressionMethod::Stored, &strict).unwrap(),
            CompressionMethod::Stored
        );
    }
}
