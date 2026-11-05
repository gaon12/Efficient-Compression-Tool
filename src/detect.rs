use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// File formats recognized by ECT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Unknown,
    Png,
    Jpeg,
    Gzip,
    Zip,
    Webp,
}

/// Detects the file type by signature first and extension second.
pub(crate) fn detect_file_type(path: &Path) -> io::Result<FileType> {
    let mut f = File::open(path)?;
    let mut buf = [0u8; 12];
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(detect_from_bytes(&buf[..n]).unwrap_or_else(|| type_by_extension(path)))
}

pub(crate) fn detect_from_bytes(b: &[u8]) -> Option<FileType> {
    if b.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) {
        Some(FileType::Png)
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(FileType::Jpeg)
    } else if b.starts_with(&[0x1f, 0x8b, 0x08]) {
        Some(FileType::Gzip)
    } else if b.starts_with(b"PK\x03\x04") {
        Some(FileType::Zip)
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some(FileType::Webp)
    } else {
        None
    }
}

fn type_by_extension(path: &Path) -> FileType {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("png") => FileType::Png,
        Some("jpg" | "jpeg") => FileType::Jpeg,
        Some("gz") => FileType::Gzip,
        Some("zip") => FileType::Zip,
        Some("webp") => FileType::Webp,
        _ => FileType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures() {
        assert_eq!(
            detect_from_bytes(b"\x89PNG\r\n\x1a\n...."),
            Some(FileType::Png)
        );
        assert_eq!(
            detect_from_bytes(&[0xff, 0xd8, 0xff, 0xe0]),
            Some(FileType::Jpeg)
        );
        assert_eq!(
            detect_from_bytes(&[0x1f, 0x8b, 0x08, 0]),
            Some(FileType::Gzip)
        );
        assert_eq!(detect_from_bytes(b"PK\x03\x04"), Some(FileType::Zip));
        assert_eq!(detect_from_bytes(b"RIFF\0\0\0\0WEBP"), Some(FileType::Webp));
        assert_eq!(detect_from_bytes(b"hello"), None);
        assert_eq!(type_by_extension(Path::new("a/B.JPEG")), FileType::Jpeg);
    }
}
