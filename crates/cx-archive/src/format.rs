//! Which archive formats we understand, and how to recognise them.
//!
//! The file name decides what the UI offers ("Open as folder", "Extract"),
//! because that has to be instant for every row of a listing. When a file is
//! actually opened we also look at its first bytes: a `.zip` that is really
//! a 7z (or a `.tgz` that is a plain tar) is common enough in the wild.

use cx_core::{CxError, Result};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    SevenZ,
}

impl ArchiveFormat {
    /// Only zip archives can be edited in place (see `zipedit`).
    pub fn writable(self) -> bool {
        self == ArchiveFormat::Zip
    }

    pub fn is_tar(self) -> bool {
        matches!(self, ArchiveFormat::Tar | ArchiveFormat::TarGz | ArchiveFormat::TarBz2 | ArchiveFormat::TarXz | ArchiveFormat::TarZst)
    }

    /// The extension to give a cached copy, so it stays recognisable on disk.
    pub fn extension(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::Tar => "tar",
            ArchiveFormat::TarGz => "tar.gz",
            ArchiveFormat::TarBz2 => "tar.bz2",
            ArchiveFormat::TarXz => "tar.xz",
            ArchiveFormat::TarZst => "tar.zst",
            ArchiveFormat::SevenZ => "7z",
        }
    }

    /// Detect from a file name alone (case-insensitive).
    pub fn from_name(name: &str) -> Option<ArchiveFormat> {
        let n = name.to_ascii_lowercase();
        let ends = |suffixes: &[&str]| suffixes.iter().any(|s| n.len() > s.len() && n.ends_with(s));
        Some(if ends(&[".zip"]) {
            ArchiveFormat::Zip
        } else if ends(&[".7z"]) {
            ArchiveFormat::SevenZ
        } else if ends(&[".tar.gz", ".tgz", ".taz"]) {
            ArchiveFormat::TarGz
        } else if ends(&[".tar.bz2", ".tbz", ".tbz2", ".tb2"]) {
            ArchiveFormat::TarBz2
        } else if ends(&[".tar.xz", ".txz"]) {
            ArchiveFormat::TarXz
        } else if ends(&[".tar.zst", ".tzst", ".tar.zstd"]) {
            ArchiveFormat::TarZst
        } else if ends(&[".tar"]) {
            ArchiveFormat::Tar
        } else {
            return None;
        })
    }

    /// Detect from the first bytes of a file (at least 262 for plain tar).
    /// Compressed streams are assumed to hold a tar, which is the only thing
    /// we can present as a folder.
    pub fn from_magic(head: &[u8]) -> Option<ArchiveFormat> {
        let starts = |m: &[u8]| head.starts_with(m);
        Some(if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"PK\x07\x08") {
            ArchiveFormat::Zip
        } else if starts(b"7z\xBC\xAF\x27\x1C") {
            ArchiveFormat::SevenZ
        } else if starts(&[0x1f, 0x8b]) {
            ArchiveFormat::TarGz
        } else if starts(b"BZh") {
            ArchiveFormat::TarBz2
        } else if starts(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
            ArchiveFormat::TarXz
        } else if starts(&[0x28, 0xB5, 0x2F, 0xFD]) {
            ArchiveFormat::TarZst
        } else if head.len() >= 262 && &head[257..262] == b"ustar" {
            ArchiveFormat::Tar
        } else {
            return None;
        })
    }

    /// Magic bytes win over the name; the name is the fallback for formats
    /// without a reliable signature (old v7 tars, empty files).
    pub fn detect(name: &str, head: &[u8]) -> Option<ArchiveFormat> {
        Self::from_magic(head).or_else(|| Self::from_name(name))
    }

    /// Detect a local file by name and content.
    pub fn detect_file(name: &str, path: &Path) -> Result<ArchiveFormat> {
        let mut head = Vec::with_capacity(512);
        File::open(path)
            .and_then(|f| f.take(512).read_to_end(&mut head))
            .map_err(|e| CxError::from_io(e, path.display()))?;
        Self::detect(name, &head).ok_or_else(|| CxError::Unsupported(format!("{name} is not a supported archive")))
    }
}

/// True when `name` looks like an archive we can open as a folder.
pub fn is_archive(name: &str) -> bool {
    ArchiveFormat::from_name(name).is_some()
}

/// The name without its archive extension ("photos.tar.gz" → "photos"),
/// used to name the folder an archive is extracted into.
pub fn strip_archive_ext(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    for ext in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.zstd", ".zip", ".7z", ".tgz", ".taz", ".tbz", ".tbz2", ".tb2", ".txz", ".tzst", ".tar"] {
        if lower.len() > ext.len() && lower.ends_with(ext) {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

/// A decoded tar stream for any of the tar flavours.
pub(crate) fn tar_reader<'a, R: Read + Send + 'a>(format: ArchiveFormat, raw: R) -> io::Result<Box<dyn Read + Send + 'a>> {
    let buffered = BufReader::with_capacity(256 * 1024, raw);
    Ok(match format {
        ArchiveFormat::Tar => Box::new(buffered),
        // Multi-member gzip (pigz, concatenated streams) is valid .tar.gz.
        ArchiveFormat::TarGz => Box::new(flate2::bufread::MultiGzDecoder::new(buffered)),
        ArchiveFormat::TarBz2 => Box::new(bzip2::bufread::MultiBzDecoder::new(buffered)),
        ArchiveFormat::TarXz => Box::new(lzma_rust2::XzReader::new(buffered, true)),
        ArchiveFormat::TarZst => Box::new(ZstdFrames::new(buffered)?),
        ArchiveFormat::Zip | ArchiveFormat::SevenZ => return Err(io::Error::other("not a tar format")),
    })
}

type ZstdDecoder<R> = ruzstd::decoding::StreamingDecoder<R, ruzstd::decoding::FrameDecoder>;

/// `ruzstd` decodes one frame at a time; parallel compressors (pzstd,
/// `zstd -T`) write several, so keep going until the input ends.
struct ZstdFrames<R: BufRead> {
    dec: Option<ZstdDecoder<R>>,
}

impl<R: BufRead> ZstdFrames<R> {
    fn new(source: R) -> io::Result<Self> {
        let dec = ZstdDecoder::new(source).map_err(io::Error::other)?;
        Ok(ZstdFrames { dec: Some(dec) })
    }
}

impl<R: BufRead> Read for ZstdFrames<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let Some(dec) = self.dec.as_mut() else { return Ok(0) };
            let n = dec.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            let (mut source, frame) = self.dec.take().expect("decoder present").into_parts();
            if source.fill_buf()?.is_empty() {
                return Ok(0);
            }
            // `new_with_decoder` re-initialises the decoder with the next frame's header.
            self.dec = Some(ruzstd::decoding::StreamingDecoder::new_with_decoder(source, frame).map_err(io::Error::other)?);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_magic() {
        assert_eq!(ArchiveFormat::from_name("A.ZIP"), Some(ArchiveFormat::Zip));
        assert_eq!(ArchiveFormat::from_name("x.tar.gz"), Some(ArchiveFormat::TarGz));
        assert_eq!(ArchiveFormat::from_name("x.tgz"), Some(ArchiveFormat::TarGz));
        assert_eq!(ArchiveFormat::from_name("x.tar.zst"), Some(ArchiveFormat::TarZst));
        assert_eq!(ArchiveFormat::from_name(".zip"), None);
        assert_eq!(ArchiveFormat::from_name("notes.txt"), None);
        assert!(is_archive("backup.7z"));
        assert_eq!(ArchiveFormat::detect("fake.zip", b"7z\xBC\xAF\x27\x1C\0\x04"), Some(ArchiveFormat::SevenZ));
        assert_eq!(strip_archive_ext("Photos 2024.tar.gz"), "Photos 2024");
        assert_eq!(strip_archive_ext("a.ZIP"), "a");
        assert_eq!(strip_archive_ext("plain"), "plain");
    }

    #[test]
    fn zstd_two_frames() {
        let data: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
        let mut all = Vec::new();
        let (a, b) = data.split_at(200_000);
        for p in [a, b] {
            all.extend(ruzstd::encoding::compress_to_vec(p, ruzstd::encoding::CompressionLevel::Fastest));
        }
        let mut r = ZstdFrames::new(std::io::BufReader::new(all.as_slice())).unwrap();
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }
}
