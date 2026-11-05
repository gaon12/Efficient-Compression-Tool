use std::fs::{self, File, Metadata};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use flate2::read::MultiGzDecoder;
use flate2::{Compression, GzBuilder};

use crate::config::Config;
use crate::processor::Outcome;
use crate::replace::{TempFile, parent_dir, set_mod_time};

const FEXTRA: u8 = 0x04;
const FCOMMENT: u8 = 0x10;

fn other(msg: &str) -> io::Error {
    io::Error::other(msg.to_string())
}

pub(crate) fn compression(level: u32) -> Compression {
    Compression::new(level.clamp(1, 9))
}

/// Re-compresses an existing gzip file, keeping its header fields.
pub(crate) fn optimize_existing_gzip(
    path: &Path,
    meta: &Metadata,
    cfg: &Config,
) -> io::Result<Outcome> {
    let before = meta.len();
    if before == 0 {
        return Err(other("compression of empty files is not supported"));
    }

    let mut input = File::open(path)?;
    let mut head = [0u8; 10];
    input
        .read_exact(&mut head)
        .map_err(|_| other("invalid gzip header"))?;
    let flags = head[3];
    if cfg.strict && (flags & FEXTRA != 0 || flags & FCOMMENT != 0) {
        return Err(other(
            "file includes extra field or comment, can't be optimized in strict mode",
        ));
    }
    drop(input);

    let mut decoder = MultiGzDecoder::new(BufReader::new(File::open(path)?));
    // Force the header to be parsed before reading its fields.
    let mut first = Vec::new();
    (&mut decoder).take(1).read_to_end(&mut first)?;
    let header = decoder
        .header()
        .ok_or_else(|| other("invalid gzip header"))?
        .clone();

    let mut builder = GzBuilder::new()
        .mtime(header.mtime())
        .operating_system(header.operating_system());
    if let Some(name) = header.filename() {
        builder = builder.filename(name);
    }
    if let Some(comment) = header.comment() {
        builder = builder.comment(comment);
    }
    if let Some(extra) = header.extra() {
        builder = builder.extra(extra);
    }

    let mut tmp = TempFile::new_in(parent_dir(path), "gzip")?;
    {
        let mut encoder = builder.write(tmp.file(), compression(cfg.level));
        io::Write::write_all(&mut encoder, &first)?;
        io::copy(&mut decoder, &mut encoder)?;
        encoder.finish()?;
    }
    let after = tmp.close()?;
    if after >= before {
        return Ok(Outcome::unchanged(before));
    }
    tmp.persist(path, Some(meta), cfg.keep_mod_time)?;
    Ok(Outcome::changed(before, after))
}

/// Compresses a regular file to `<path>.gz`, returning the output size.
pub(crate) fn compress_file_to_gzip(path: &Path, meta: &Metadata, cfg: &Config) -> io::Result<u64> {
    if meta.len() == 0 {
        return Err(other("compression of empty files is not supported"));
    }
    let mut out_path = PathBuf::from(path.as_os_str());
    out_path.as_mut_os_string().push(".gz");
    if out_path.exists() {
        return Err(other("compressed file already exists"));
    }

    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(0));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut input = BufReader::new(File::open(path)?);
    let mut tmp = TempFile::new_in(parent_dir(&out_path), "gzip")?;
    {
        let mut encoder = GzBuilder::new()
            .filename(name)
            .mtime(mtime)
            .write(tmp.file(), compression(cfg.level));
        io::copy(&mut input, &mut encoder)?;
        encoder.finish()?;
    }
    tmp.close()?;
    if out_path.exists() {
        return Err(other("compressed file already exists"));
    }
    tmp.persist(&out_path, Some(meta), false)?;
    if cfg.keep_mod_time {
        set_mod_time(&out_path, meta);
    }
    Ok(fs::metadata(&out_path)?.len())
}
