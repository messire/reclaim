//! Parsing of `FILE_ID_BOTH_DIR_INFO` records (`GetFileInformationByHandleEx` with
//! `FileIdBothDirectoryInfo`). The offsets are checked against the `windows-sys`
//! definition at compile time on Windows (`win::fs`).

use reclaim_storage_core::entry::EntryMeta;

use super::{RawMeta, entry_from, fa, rd_i64, rd_u32, rd_utf16};

/// Byte offsets inside `FILE_ID_BOTH_DIR_INFO` (x86-64 and AArch64 layout).
pub(crate) mod off {
    pub const NEXT_ENTRY_OFFSET: usize = 0;
    pub const LAST_WRITE_TIME: usize = 24;
    pub const END_OF_FILE: usize = 40;
    pub const ALLOCATION_SIZE: usize = 48;
    pub const FILE_ATTRIBUTES: usize = 56;
    pub const FILE_NAME_LENGTH: usize = 60;
    pub const EA_SIZE: usize = 64;
    pub const FILE_ID: usize = 96;
    pub const FILE_NAME: usize = 104;
}

/// Appends the entries of one buffer (a `NextEntryOffset` chain) to `out`, skipping
/// `.` and `..`. Returns `None` if a record is truncated (the entries before it are
/// kept).
pub(crate) fn parse(buf: &[u8], volume: Option<u64>, out: &mut Vec<EntryMeta>) -> Option<()> {
    let mut at = 0usize;
    loop {
        let next = rd_u32(buf, at + off::NEXT_ENTRY_OFFSET)? as usize;
        let name_len = rd_u32(buf, at + off::FILE_NAME_LENGTH)? as usize;
        let name = rd_utf16(buf, at + off::FILE_NAME, name_len)?;
        if name != "." && name != ".." {
            let attr = rd_u32(buf, at + off::FILE_ATTRIBUTES)?;
            // For a reparse point, EaSize holds the reparse tag (documented for the
            // directory information classes).
            let reparse_tag = if attr & fa::REPARSE_POINT != 0 {
                rd_u32(buf, at + off::EA_SIZE)?
            } else {
                0
            };
            let raw = RawMeta {
                attr,
                reparse_tag,
                end_of_file: rd_i64(buf, at + off::END_OF_FILE)?,
                allocation: rd_i64(buf, at + off::ALLOCATION_SIZE)?,
                last_write: rd_i64(buf, at + off::LAST_WRITE_TIME)?,
                file_id: Some(rd_i64(buf, at + off::FILE_ID)? as u64),
                // Directory listings do not report link counts.
                nlink: None,
                volume,
            };
            out.push(entry_from(name, &raw));
        }
        if next == 0 {
            return Some(());
        }
        at = at.checked_add(next)?;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::common::utf16le;
    use reclaim_storage_core::entry::{Attrs, Kind};

    /// One synthetic record, padded to 8 bytes.
    pub(crate) fn record(name: &str, attr: u32, ea: u32, eof: i64, alloc: i64, id: i64) -> Vec<u8> {
        let n = utf16le(name);
        let mut r = vec![0u8; off::FILE_NAME + n.len()];
        r[off::LAST_WRITE_TIME..][..8].copy_from_slice(&116_444_736_100_000_000i64.to_le_bytes());
        r[off::END_OF_FILE..][..8].copy_from_slice(&eof.to_le_bytes());
        r[off::ALLOCATION_SIZE..][..8].copy_from_slice(&alloc.to_le_bytes());
        r[off::FILE_ATTRIBUTES..][..4].copy_from_slice(&attr.to_le_bytes());
        r[off::FILE_NAME_LENGTH..][..4].copy_from_slice(&(n.len() as u32).to_le_bytes());
        r[off::EA_SIZE..][..4].copy_from_slice(&ea.to_le_bytes());
        r[off::FILE_ID..][..8].copy_from_slice(&id.to_le_bytes());
        r[off::FILE_NAME..].copy_from_slice(&n);
        while !r.len().is_multiple_of(8) {
            r.push(0);
        }
        r
    }

    fn chain(records: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, r) in records.iter().enumerate() {
            let mut r = r.clone();
            if i + 1 < records.len() {
                let len = r.len() as u32;
                r[..4].copy_from_slice(&len.to_le_bytes());
            }
            out.extend(r);
        }
        out
    }

    #[test]
    fn parses_a_listing() {
        let buf = chain(&[
            record(".", fa::DIRECTORY, 0, 0, 0, 1),
            record("..", fa::DIRECTORY, 0, 0, 0, 2),
            record("a.txt", fa::HIDDEN, 0, 10, 4096, 0x0001_0000_0000_0042),
            record("sub", fa::DIRECTORY, 0, 0, 0, 7),
            record("j", fa::DIRECTORY | fa::REPARSE_POINT, 0xA000_0003, 0, 0, 8),
            record(
                "cloud.docx",
                fa::REPARSE_POINT | fa::RECALL_ON_DATA_ACCESS,
                0x9000_001A,
                500,
                0,
                9,
            ),
        ]);
        let mut out = Vec::new();
        assert_eq!(parse(&buf, Some(77), &mut out), Some(()));
        assert_eq!(out.len(), 4);
        let a = &out[0];
        assert_eq!(a.name, "a.txt");
        assert_eq!(
            (a.kind, a.logical, a.allocated),
            (Kind::File, 10, Some(4096))
        );
        assert_eq!(a.file_id, Some(0x0001_0000_0000_0042));
        assert_eq!((a.volume, a.nlink, a.mtime), (Some(77), None, 10));
        assert!(a.attrs.has(Attrs::HIDDEN));
        assert_eq!(out[1].kind, Kind::Dir);
        assert_eq!(out[2].kind, Kind::Link);
        assert_eq!(out[3].kind, Kind::File);
        assert_eq!(out[3].logical, 500);
        assert!(out[3].attrs.has(Attrs::REPARSE | Attrs::OFFLINE));
    }

    #[test]
    fn truncated_record_stops() {
        let mut buf = chain(&[record("a", 0, 0, 1, 1, 1), record("bbbb", 0, 0, 1, 1, 2)]);
        buf.truncate(buf.len() - 12);
        let mut out = Vec::new();
        assert_eq!(parse(&buf, None, &mut out), None);
        assert_eq!(out.len(), 1);
    }
}
