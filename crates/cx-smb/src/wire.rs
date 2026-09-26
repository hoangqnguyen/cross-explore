//! The few SMB2 exchanges smb2's high-level API does not expose, built from
//! its public message types.
//!
//! smb2's `list_directory` and `stat` drop the file attributes, and we need
//! them for the hidden and read-only flags; `list_directory` also returns the
//! whole folder at once, while the UI wants the first rows immediately. There
//! is no "set basic info" either, which keeping modification times needs. So
//! this module speaks CREATE / QUERY_DIRECTORY / SET_INFO / CLOSE directly,
//! using compounds so a stat or a touch is a single round trip.

use smb2::client::connection::{CompoundOp, Connection, Frame};
use smb2::msg::close::CloseRequest;
use smb2::msg::create::{CreateDisposition, CreateRequest, CreateResponse, ImpersonationLevel, ShareAccess};
use smb2::msg::query_directory::{FileInformationClass, QueryDirectoryFlags, QueryDirectoryRequest, QueryDirectoryResponse};
use smb2::msg::query_info::InfoType;
use smb2::msg::set_info::SetInfoRequest;
use smb2::pack::{ReadCursor, Unpack};
use smb2::types::flags::FileAccessMask;
use smb2::types::status::NtStatus;
use smb2::types::{Command, CreditCharge, FileId, OplockLevel};
use smb2::{Error, Tree};

pub(crate) const ATTR_READONLY: u32 = 0x1;
pub(crate) const ATTR_HIDDEN: u32 = 0x2;
pub(crate) const ATTR_DIRECTORY: u32 = 0x10;
pub(crate) const ATTR_REPARSE_POINT: u32 = 0x400;

const FILE_DIRECTORY_FILE: u32 = 0x1;
const FILE_BASIC_INFORMATION: u8 = 4;

/// Metadata of one file or folder, straight off the wire.
#[derive(Debug, Clone)]
pub(crate) struct RawEntry {
    pub name: String,
    pub attributes: u32,
    pub size: u64,
    /// FILETIME ticks (100 ns since 1601), 0 when unknown.
    pub created: u64,
    pub modified: u64,
}

impl RawEntry {
    pub fn is_dir(&self) -> bool {
        self.attributes & ATTR_DIRECTORY != 0
    }
}

/// Windows FILETIME ticks → Unix milliseconds.
pub(crate) fn filetime_to_ms(ft: u64) -> Option<i64> {
    const EPOCH_DIFF: i64 = 116_444_736_000_000_000;
    (ft != 0).then(|| (ft as i64 - EPOCH_DIFF) / 10_000)
}

pub(crate) fn ms_to_filetime(ms: i64) -> u64 {
    const EPOCH_DIFF: i64 = 116_444_736_000_000_000;
    (ms.saturating_mul(10_000) + EPOCH_DIFF).max(0) as u64
}

/// The path as the server wants it: `\`-separated, forbidden characters
/// mapped the way macOS does (see `smb2::name`), and for DFS shares prefixed
/// with `host\share` (MS-SMB2 § 3.2.4.3).
pub(crate) fn wire_path(tree: &Tree, path: &str) -> String {
    let p = if path.trim_matches('/').is_empty() { String::new() } else { smb2::encode_path(path.trim_end_matches('/')) };
    if !tree.is_dfs {
        return p;
    }
    let host = host_of(&tree.server);
    if p.is_empty() {
        format!("{host}\\{}", tree.share_name)
    } else {
        format!("{host}\\{}\\{p}", tree.share_name)
    }
}

fn host_of(addr: &str) -> &str {
    if let Some(host) = addr.strip_prefix('[').and_then(|r| r.split_once(']')).map(|(h, _)| h) {
        return host;
    }
    match addr.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => host,
        _ => addr,
    }
}

fn open_request(name: String, access: u32, options: u32) -> CreateRequest {
    CreateRequest {
        requested_oplock_level: OplockLevel::None,
        impersonation_level: ImpersonationLevel::Impersonation,
        desired_access: FileAccessMask::new(access),
        file_attributes: 0,
        share_access: ShareAccess(ShareAccess::FILE_SHARE_READ | ShareAccess::FILE_SHARE_WRITE | ShareAccess::FILE_SHARE_DELETE),
        create_disposition: CreateDisposition::FileOpen,
        create_options: options,
        name,
        create_contexts: vec![],
    }
}

fn check(frame: &Frame, command: Command) -> smb2::Result<()> {
    if frame.header.status == NtStatus::SUCCESS {
        Ok(())
    } else {
        Err(Error::Protocol { status: frame.header.status, command })
    }
}

fn collect(frames: Vec<smb2::Result<Frame>>, expected: usize) -> smb2::Result<Vec<Frame>> {
    let frames = frames.into_iter().collect::<smb2::Result<Vec<_>>>()?;
    if frames.len() != expected {
        return Err(Error::invalid_data(format!("compound answered with {} frames, expected {expected}", frames.len())));
    }
    Ok(frames)
}

/// Stat by opening for attributes and closing, in one compound round trip:
/// the CREATE response already carries times, size and attributes.
pub(crate) async fn stat(conn: &Connection, tree: &Tree, path: &str) -> smb2::Result<RawEntry> {
    let create = open_request(wire_path(tree, path), FileAccessMask::FILE_READ_ATTRIBUTES | FileAccessMask::SYNCHRONIZE, 0);
    let close = CloseRequest { flags: 0, file_id: FileId::SENTINEL };
    let ops = [CompoundOp::new(Command::Create, &create, Some(tree.tree_id)), CompoundOp::new(Command::Close, &close, Some(tree.tree_id))];
    let frames = collect(conn.execute_compound(&ops).await?, 2)?;
    check(&frames[0], Command::Create)?;
    let resp = CreateResponse::unpack(&mut ReadCursor::new(&frames[0].body))?;
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
    Ok(RawEntry {
        name,
        attributes: resp.file_attributes,
        size: resp.end_of_file,
        created: resp.creation_time.0,
        modified: resp.last_write_time.0,
    })
}

/// Set the last-write time (FileBasicInformation; zero fields mean "leave
/// as is"), in one CREATE + SET_INFO + CLOSE compound.
pub(crate) async fn set_modified(conn: &mut Connection, tree: &Tree, path: &str, filetime: u64) -> smb2::Result<()> {
    let create = open_request(wire_path(tree, path), FileAccessMask::FILE_WRITE_ATTRIBUTES | FileAccessMask::SYNCHRONIZE, 0);
    let mut basic = Vec::with_capacity(40);
    basic.extend_from_slice(&0u64.to_le_bytes()); // CreationTime
    basic.extend_from_slice(&0u64.to_le_bytes()); // LastAccessTime
    basic.extend_from_slice(&filetime.to_le_bytes()); // LastWriteTime
    basic.extend_from_slice(&0u64.to_le_bytes()); // ChangeTime
    basic.extend_from_slice(&0u32.to_le_bytes()); // FileAttributes (0 = unchanged)
    basic.extend_from_slice(&0u32.to_le_bytes()); // Reserved
    let set = SetInfoRequest {
        info_type: InfoType::File,
        file_info_class: FILE_BASIC_INFORMATION,
        additional_information: 0,
        file_id: FileId::SENTINEL,
        buffer: basic,
    };
    let close = CloseRequest { flags: 0, file_id: FileId::SENTINEL };
    let ops = [
        CompoundOp::new(Command::Create, &create, Some(tree.tree_id)),
        CompoundOp::new(Command::SetInfo, &set, Some(tree.tree_id)),
        CompoundOp::new(Command::Close, &close, Some(tree.tree_id)),
    ];
    let frames = collect(conn.execute_compound(&ops).await?, 3)?;
    check(&frames[0], Command::Create)?;
    if let Err(e) = check(&frames[1], Command::SetInfo) {
        // The CLOSE cascaded with the failed SET_INFO: close explicitly so
        // the handle doesn't leak.
        if let Ok(resp) = CreateResponse::unpack(&mut ReadCursor::new(&frames[0].body)) {
            let _ = tree.close_handle(conn, resp.file_id).await;
        }
        return Err(e);
    }
    Ok(())
}

/// An open directory handle that yields its entries one server page at a
/// time, so a listing can be streamed to the UI while it is still arriving.
pub(crate) struct DirReader {
    conn: Connection,
    tree: std::sync::Arc<Tree>,
    file_id: FileId,
    restart: bool,
    buffer_len: u32,
    done: bool,
}

impl DirReader {
    pub(crate) async fn open(conn: Connection, tree: std::sync::Arc<Tree>, path: &str) -> smb2::Result<DirReader> {
        let req = open_request(
            wire_path(&tree, path),
            FileAccessMask::FILE_READ_DATA | FileAccessMask::FILE_READ_ATTRIBUTES | FileAccessMask::SYNCHRONIZE,
            FILE_DIRECTORY_FILE,
        );
        let frame = conn.execute(Command::Create, &req, Some(tree.tree_id)).await?;
        check(&frame, Command::Create)?;
        let resp = CreateResponse::unpack(&mut ReadCursor::new(&frame.body))?;
        // 64 KiB is one credit and the common max transact size floor.
        let buffer_len = conn.params().map(|p| p.max_transact_size).unwrap_or(65536).min(65536);
        Ok(DirReader { conn, tree, file_id: resp.file_id, restart: true, buffer_len, done: false })
    }

    /// The next page of entries (without `.` and `..`), or `None` at the end.
    pub(crate) async fn next_page(&mut self) -> smb2::Result<Option<Vec<RawEntry>>> {
        if self.done {
            return Ok(None);
        }
        let req = QueryDirectoryRequest {
            file_information_class: FileInformationClass::FileDirectoryInformation,
            flags: QueryDirectoryFlags(if self.restart { QueryDirectoryFlags::RESTART_SCANS } else { 0 }),
            file_index: 0,
            file_id: self.file_id,
            output_buffer_length: self.buffer_len,
            file_name: "*".to_string(),
        };
        self.restart = false;
        let charge = CreditCharge(self.buffer_len.div_ceil(65536).max(1) as u16);
        let frame = self.conn.execute_with_credits(Command::QueryDirectory, &req, Some(self.tree.tree_id), charge).await?;
        if frame.header.status == NtStatus::NO_MORE_FILES {
            self.done = true;
            return Ok(None);
        }
        check(&frame, Command::QueryDirectory)?;
        let resp = QueryDirectoryResponse::unpack(&mut ReadCursor::new(&frame.body))?;
        parse_directory_information(&resp.output_buffer).map(Some)
    }

    pub(crate) async fn close(mut self) {
        let _ = self.tree.close_handle(&mut self.conn, self.file_id).await;
    }
}

/// Parse a chain of FILE_DIRECTORY_INFORMATION records (MS-FSCC 2.4.10).
fn parse_directory_information(buf: &[u8]) -> smb2::Result<Vec<RawEntry>> {
    let mut out = Vec::new();
    let mut off = 0usize;
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let u64_at = |b: &[u8], i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
    loop {
        let rec = buf.get(off..).filter(|r| r.len() >= 64).ok_or_else(|| Error::invalid_data("short directory record"))?;
        let next = u32_at(rec, 0) as usize;
        let name_len = u32_at(rec, 60) as usize;
        let name_bytes = rec.get(64..64 + name_len).ok_or_else(|| Error::invalid_data("directory name overruns record"))?;
        let units: Vec<u16> = name_bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let raw_name = String::from_utf16_lossy(&units);
        if raw_name != "." && raw_name != ".." {
            out.push(RawEntry {
                name: smb2::decode_name(&raw_name).into_owned(),
                created: u64_at(rec, 8),
                modified: u64_at(rec, 24),
                size: u64_at(rec, 40),
                attributes: u32_at(rec, 56),
            });
        }
        if next == 0 {
            break;
        }
        off += next;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, attrs: u32, size: u64, last: bool) -> Vec<u8> {
        let units: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let len = (64 + units.len()).div_ceil(8) * 8;
        let mut r = vec![0u8; len];
        if !last {
            r[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        }
        r[24..32].copy_from_slice(&ms_to_filetime(1_000).to_le_bytes());
        r[40..48].copy_from_slice(&size.to_le_bytes());
        r[56..60].copy_from_slice(&attrs.to_le_bytes());
        r[60..64].copy_from_slice(&(units.len() as u32).to_le_bytes());
        r[64..64 + units.len()].copy_from_slice(&units);
        r
    }

    #[test]
    fn parses_records_and_skips_dot_entries() {
        let mut buf = record(".", ATTR_DIRECTORY, 0, false);
        buf.extend(record("..", ATTR_DIRECTORY, 0, false));
        buf.extend(record("Ünï cødé.txt", ATTR_HIDDEN, 42, false));
        buf.extend(record("sub", ATTR_DIRECTORY, 0, true));
        let e = parse_directory_information(&buf).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "Ünï cødé.txt");
        assert_eq!(e[0].size, 42);
        assert_eq!(e[0].attributes & ATTR_HIDDEN, ATTR_HIDDEN);
        assert_eq!(filetime_to_ms(e[0].modified), Some(1_000));
        assert!(e[1].is_dir());
    }

    #[test]
    fn host_of_strips_ports() {
        assert_eq!(host_of("nas:1445"), "nas");
        assert_eq!(host_of("[::1]:445"), "::1");
        assert_eq!(host_of("nas"), "nas");
    }
}
