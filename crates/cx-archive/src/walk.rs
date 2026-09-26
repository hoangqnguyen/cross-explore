//! Blocking, format-specific reading: headers for the index, one member for
//! `open_read`, every member in order for extraction.
//!
//! Everything here runs on the blocking pool and reads a *local* file (a
//! remote container has been downloaded to the cache first).

use crate::format::{tar_reader, ArchiveFormat};
use crate::index::{ArchiveIndex, MemberHeader, MemberKind};
use crate::time::{dos_to_ms, secs_to_ms};
use cx_core::{CxError, Entry, Result};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use zip::result::ZipError;

pub(crate) enum Flow {
    Continue,
    Stop,
}

pub(crate) type Visit<'a> = dyn FnMut(&MemberHeader, &mut dyn Read) -> io::Result<Flow> + 'a;

fn open(path: &Path) -> Result<File> {
    File::open(path).map_err(|e| CxError::from_io(e, path.display()))
}

pub(crate) fn zip_err(e: ZipError, ctx: impl std::fmt::Display) -> CxError {
    match e {
        ZipError::Io(e) => CxError::from_io(e, ctx),
        ZipError::UnsupportedArchive(m) => CxError::Unsupported(format!("{ctx}: {m}")),
        ZipError::CompressionMethodNotSupported(m) => CxError::Unsupported(format!("{ctx}: compression method {m}")),
        ZipError::InvalidPassword => CxError::Unsupported(format!("{ctx}: encrypted")),
        ZipError::FileNotFound => CxError::NotFound(ctx.to_string()),
        other => CxError::io(ctx, other),
    }
}

fn sz_err(e: sevenz_rust2::Error, ctx: impl std::fmt::Display) -> CxError {
    match e {
        sevenz_rust2::Error::Io(e, _) => CxError::from_io(e, ctx),
        sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) => CxError::Unsupported(format!("{ctx}: encrypted")),
        sevenz_rust2::Error::UnsupportedCompressionMethod(m) => CxError::Unsupported(format!("{ctx}: compression method {m}")),
        other => CxError::io(ctx, other),
    }
}

/// Keeps the bytes-read position of the archive file, which is how
/// extraction reports progress uniformly for every format (the uncompressed
/// total of a tar.gz is unknown until the end).
pub(crate) struct Counting<R> {
    inner: R,
    pos: Arc<AtomicU64>,
}

impl<R> Counting<R> {
    pub fn new(inner: R, pos: Arc<AtomicU64>) -> Self {
        Counting { inner, pos }
    }
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.pos.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: Seek> Seek for Counting<R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = self.inner.seek(to)?;
        self.pos.store(p, Ordering::Relaxed);
        Ok(p)
    }
}

// ---- zip ------------------------------------------------------------------

fn zip_header<R: Read>(f: &zip::read::ZipFile<'_, R>, ordinal: usize) -> MemberHeader {
    let kind = if f.is_dir() {
        MemberKind::Dir
    } else if f.is_symlink() {
        MemberKind::Symlink
    } else {
        MemberKind::File
    };
    // The "UT" extra field holds a real UTC timestamp; the DOS time every
    // entry has is zone-less and only 2-second precise.
    let ut = f.extra_data_fields().find_map(|x| match x {
        zip::extra_fields::ExtraField::ExtendedTimestamp(t) => t.mod_time(),
        _ => None,
    });
    let modified = ut.map(|s| secs_to_ms(s as i64)).or_else(|| f.last_modified().map(dos_to_ms));
    MemberHeader { raw_name: f.name().to_string(), kind, size: f.size(), modified, ordinal }
}

fn zip_archive<R: Read + Seek>(r: R, ctx: &Path) -> Result<zip::ZipArchive<R>> {
    zip::ZipArchive::new(r).map_err(|e| zip_err(e, ctx.display()))
}

// ---- 7z -------------------------------------------------------------------

fn sz_header(e: &sevenz_rust2::ArchiveEntry, ordinal: usize) -> MemberHeader {
    let modified = e.has_last_modified_date.then(|| {
        let t: std::time::SystemTime = e.last_modified_date.into();
        crate::time::system_to_ms(t)
    });
    let kind = if e.is_directory { MemberKind::Dir } else { MemberKind::File };
    MemberHeader { raw_name: e.name.clone(), kind, size: e.size, modified, ordinal }
}

// ---- tar ------------------------------------------------------------------

fn tar_header<R: Read>(e: &tar::Entry<'_, R>, ordinal: usize) -> Option<MemberHeader> {
    use tar::EntryType as T;
    let h = e.header();
    let kind = match h.entry_type() {
        T::Directory => MemberKind::Dir,
        T::Regular | T::Continuous | T::GNUSparse => MemberKind::File,
        T::Symlink | T::Link => MemberKind::Symlink,
        // Metadata records, not members.
        T::XGlobalHeader | T::XHeader | T::GNULongName | T::GNULongLink => return None,
        _ => MemberKind::Other,
    };
    let raw_name = String::from_utf8_lossy(&e.path_bytes()).into_owned();
    let modified = h.mtime().ok().map(|s| secs_to_ms(s as i64));
    Some(MemberHeader { raw_name, kind, size: e.size(), modified, ordinal })
}

fn tar_walk(path: &Path, format: ArchiveFormat, counter: Option<Arc<AtomicU64>>, visit: &mut Visit<'_>) -> Result<()> {
    let file = open(path)?;
    let ctx = path.display();
    let raw: Box<dyn Read + Send> = match counter {
        Some(c) => Box::new(Counting::new(file, c)),
        None => Box::new(file),
    };
    let decoded = tar_reader(format, raw).map_err(|e| CxError::from_io(e, &ctx))?;
    let mut archive = tar::Archive::new(decoded);
    let entries = archive.entries().map_err(|e| CxError::from_io(e, &ctx))?;
    for (ordinal, entry) in entries.enumerate() {
        let mut entry = entry.map_err(|e| CxError::io(&ctx, e))?;
        let Some(h) = tar_header(&entry, ordinal) else { continue };
        match visit(&h, &mut entry).map_err(|e| CxError::from_io(e, &ctx))? {
            Flow::Continue => {}
            Flow::Stop => break,
        }
    }
    Ok(())
}

// ---- public entry points --------------------------------------------------

/// Read the archive's headers into a tree. For zip and 7z only the central
/// directory is read; a tar has no directory, so its stream is decoded once
/// (the data itself is skipped, not kept).
pub(crate) fn build_index(path: &Path, format: ArchiveFormat, root: Entry) -> Result<ArchiveIndex> {
    let mut index = ArchiveIndex::new(root, !format.writable());
    match format {
        ArchiveFormat::Zip => {
            let mut z = zip_archive(BufReader::new(open(path)?), path)?;
            for i in 0..z.len() {
                let f = z.by_index_raw(i).map_err(|e| zip_err(e, path.display()))?;
                index.insert(&zip_header(&f, i));
            }
        }
        ArchiveFormat::SevenZ => {
            let mut f = BufReader::new(open(path)?);
            let archive = sevenz_rust2::Archive::read(&mut f, &sevenz_rust2::Password::empty()).map_err(|e| sz_err(e, path.display()))?;
            // "Anti-items" (deletion markers of update archives) are not
            // skipped: sevenz-rust2 up to 0.20 flags every empty file and
            // folder it writes as one, and real anti-items are very rare.
            for (i, e) in archive.files.iter().enumerate() {
                index.insert(&sz_header(e, i));
            }
        }
        _ => tar_walk(path, format, None, &mut |h, _| {
            index.insert(h);
            Ok(Flow::Continue)
        })?,
    }
    Ok(index)
}

/// Copy member `ordinal` into `out`.
pub(crate) fn read_member(path: &Path, format: ArchiveFormat, ordinal: usize, out: &mut dyn Write) -> Result<()> {
    let ctx = path.display();
    match format {
        ArchiveFormat::Zip => {
            let mut z = zip_archive(BufReader::new(open(path)?), path)?;
            let mut f = z.by_index(ordinal).map_err(|e| zip_err(e, &ctx))?;
            io::copy(&mut f, out).map_err(|e| CxError::from_io(e, &ctx))?;
        }
        ArchiveFormat::SevenZ => {
            let mut f = BufReader::new(open(path)?);
            let pw = sevenz_rust2::Password::empty();
            let archive = sevenz_rust2::Archive::read(&mut f, &pw).map_err(|e| sz_err(e, &ctx))?;
            let Some(block) = archive.stream_map.file_block_index.get(ordinal).copied().flatten() else {
                return Ok(()); // no data stream: an empty file
            };
            let target: *const sevenz_rust2::ArchiveEntry = &archive.files[ordinal];
            // Only the block holding the member is decoded. In a solid block
            // the members before it must still be decoded (and drained, or
            // the next reader would start mid-way through their data).
            sevenz_rust2::BlockDecoder::new(1, block, &archive, &pw, &mut f)
                .for_each_entries(&mut |e, r| {
                    if std::ptr::eq(e, target) {
                        io::copy(r, out)?;
                        Ok(false)
                    } else {
                        io::copy(r, &mut io::sink())?;
                        Ok(true)
                    }
                })
                .map_err(|e| sz_err(e, &ctx))?;
        }
        _ => {
            let mut found = false;
            tar_walk(path, format, None, &mut |h, r| {
                if h.ordinal == ordinal {
                    found = true;
                    io::copy(r, out)?;
                    return Ok(Flow::Stop);
                }
                Ok(Flow::Continue)
            })?;
            if !found {
                return Err(CxError::NotFound(format!("{ctx} member #{ordinal}")));
            }
        }
    }
    Ok(())
}

/// Visit every member, in archive order, with its data.
pub(crate) fn walk_all(path: &Path, format: ArchiveFormat, counter: Arc<AtomicU64>, visit: &mut Visit<'_>) -> Result<()> {
    let ctx = path.display();
    match format {
        ArchiveFormat::Zip => {
            let file = Counting::new(open(path)?, counter);
            let mut z = zip_archive(BufReader::new(file), path)?;
            for i in 0..z.len() {
                let mut f = z.by_index(i).map_err(|e| zip_err(e, &ctx))?;
                let h = zip_header(&f, i);
                if let Flow::Stop = visit(&h, &mut f).map_err(|e| CxError::from_io(e, &ctx))? {
                    break;
                }
            }
        }
        ArchiveFormat::SevenZ => {
            let file = BufReader::new(Counting::new(open(path)?, counter));
            let mut reader = sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty()).map_err(|e| sz_err(e, &ctx))?;
            let mut failure: Option<io::Error> = None;
            let res = reader.for_each_entries(|e, r| {
                match visit(&sz_header(e, 0), r) {
                    Ok(Flow::Continue) => {
                        // Drain whatever the visitor skipped (solid blocks).
                        io::copy(r, &mut io::sink())?;
                        Ok(true)
                    }
                    Ok(Flow::Stop) => Ok(false),
                    Err(err) => {
                        failure = Some(err);
                        Ok(false)
                    }
                }
            });
            if let Some(e) = failure {
                return Err(CxError::from_io(e, &ctx));
            }
            res.map_err(|e| sz_err(e, &ctx))?;
        }
        _ => tar_walk(path, format, Some(counter), visit)?,
    }
    Ok(())
}
