//! Pure helpers of the Windows adapter: path prefixes, time and attribute conversion and
//! bounds-checked parsing of the variable-length records Windows returns (directory
//! listings, file layouts, change notifications).
//!
//! Nothing here calls the OS, so the module is also compiled on other platforms for its
//! unit tests (synthetic buffers in the documented layouts).

// On non-Windows hosts the module exists only for its tests, which do not reach every
// helper the Windows code uses.
#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) mod dirinfo;
pub(crate) mod layout;
pub(crate) mod notify;

use reclaim_storage_core::entry::{Attrs, EntryMeta, Kind};

/// `FILE_ATTRIBUTE_*` values used by the adapter (winnt.h).
pub(crate) mod fa {
    pub const READONLY: u32 = 0x1;
    pub const HIDDEN: u32 = 0x2;
    pub const SYSTEM: u32 = 0x4;
    pub const DIRECTORY: u32 = 0x10;
    pub const SPARSE_FILE: u32 = 0x200;
    pub const REPARSE_POINT: u32 = 0x400;
    pub const COMPRESSED: u32 = 0x800;
    pub const OFFLINE: u32 = 0x1000;
    pub const RECALL_ON_OPEN: u32 = 0x4_0000;
    pub const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    /// NTFS-internal "has a `$I30` index" bit some layout outputs use for directories.
    pub const DUP_FILE_NAME_INDEX_PRESENT: u32 = 0x1000_0000;
}

/// `IsReparseTagNameSurrogate` (winnt.h): the reparse point names another object
/// (symbolic links, junctions, mount points, …) instead of storing data.
pub(crate) const fn is_name_surrogate(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

/// Adds the long-path prefix (`\\?\C:\…`, `\\?\UNC\server\share\…`) to an absolute path
/// for an OS call. Forward slashes become backslashes (the prefix disables the OS's own
/// normalization). Relative paths, device paths, already prefixed paths and paths with
/// `.`/`..` components (which only the OS's normalization resolves) are left alone.
pub(crate) fn long_path(p: &str) -> String {
    let p = p.replace('/', "\\");
    if p.starts_with("\\\\?\\") || p.starts_with("\\\\.\\") {
        return p;
    }
    if p.split('\\').any(|c| c == "." || c == "..") {
        return p;
    }
    if let Some(rest) = p.strip_prefix("\\\\") {
        return format!("\\\\?\\UNC\\{rest}");
    }
    let b = p.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        return format!("\\\\?\\{p}");
    }
    p
}

/// Removes a long-path prefix: `\\?\UNC\server\share` → `\\server\share`,
/// `\\?\C:\x` → `C:\x`. Other strings are returned unchanged.
pub(crate) fn strip_long_prefix(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = p.strip_prefix("\\\\?\\") {
        rest.to_owned()
    } else {
        p.to_owned()
    }
}

/// FILETIME (100 ns ticks since 1601-01-01) → seconds since the Unix epoch; 0 for an
/// unset time.
pub(crate) fn filetime_to_unix(ft: i64) -> i64 {
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    if ft <= 0 {
        0
    } else {
        ft / 10_000_000 - EPOCH_DIFF_SECS
    }
}

/// Attribute bits that do not depend on the reparse tag.
pub(crate) fn attrs_from(attr: u32) -> Attrs {
    let mut a = Attrs::default();
    let map = [
        (fa::HIDDEN, Attrs::HIDDEN),
        (fa::SYSTEM, Attrs::SYSTEM),
        (fa::COMPRESSED, Attrs::COMPRESSED),
        (fa::SPARSE_FILE, Attrs::SPARSE),
        (fa::READONLY, Attrs::READONLY),
        (fa::OFFLINE, Attrs::OFFLINE),
        (fa::RECALL_ON_DATA_ACCESS, Attrs::OFFLINE),
        (fa::RECALL_ON_OPEN, Attrs::OFFLINE),
    ];
    for (w, r) in map {
        if attr & w != 0 {
            a.set(r);
        }
    }
    a
}

/// Kind and attributes of an object from its attributes and reparse tag. A name
/// surrogate (link, junction, mount point) is a [`Kind::Link`]; any other reparse point
/// (deduplicated files, cloud placeholders, WCI, …) holds real data and keeps its kind.
pub(crate) fn classify(attr: u32, reparse_tag: u32) -> (Kind, Attrs) {
    let mut a = attrs_from(attr);
    if attr & fa::REPARSE_POINT != 0 {
        a.set(Attrs::REPARSE);
        if is_name_surrogate(reparse_tag) {
            return (Kind::Link, a);
        }
    }
    let kind = if attr & fa::DIRECTORY != 0 {
        Kind::Dir
    } else {
        Kind::File
    };
    (kind, a)
}

/// Raw facts about one object, as the different Windows queries report them.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RawMeta {
    pub attr: u32,
    pub reparse_tag: u32,
    pub end_of_file: i64,
    pub allocation: i64,
    pub last_write: i64,
    pub file_id: Option<u64>,
    pub nlink: Option<u32>,
    pub volume: Option<u64>,
}

/// Builds the entry the scanner sees. Directories and links have logical size 0; links
/// also allocated 0 (their target is never counted).
pub(crate) fn entry_from(name: String, r: &RawMeta) -> EntryMeta {
    let (kind, attrs) = classify(r.attr, r.reparse_tag);
    let (logical, allocated) = match kind {
        Kind::File => (
            u64::try_from(r.end_of_file).unwrap_or(0),
            u64::try_from(r.allocation).unwrap_or(0),
        ),
        Kind::Dir => (0, u64::try_from(r.allocation).unwrap_or(0)),
        Kind::Link | Kind::Other => (0, 0),
    };
    EntryMeta {
        name,
        kind,
        logical,
        allocated: Some(allocated),
        mtime: filetime_to_unix(r.last_write),
        attrs,
        file_id: r.file_id,
        nlink: r.nlink,
        volume: r.volume,
    }
}

/// Little-endian reads from a byte buffer; `None` when out of bounds.
pub(crate) fn rd_u32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes(s.try_into().ok()?))
}

/// See [`rd_u32`].
pub(crate) fn rd_u64(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off.checked_add(8)?)?;
    Some(u64::from_le_bytes(s.try_into().ok()?))
}

/// See [`rd_u32`].
pub(crate) fn rd_i64(b: &[u8], off: usize) -> Option<i64> {
    rd_u64(b, off).map(|v| v as i64)
}

/// A UTF-16LE string of `len_bytes` bytes at `off` (unpaired surrogates become U+FFFD).
pub(crate) fn rd_utf16(b: &[u8], off: usize, len_bytes: usize) -> Option<String> {
    let s = b.get(off..off.checked_add(len_bytes)?)?;
    let units: Vec<u16> = s
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Encodes a UTF-16 test string (tests of the parsers).
#[cfg(test)]
pub(crate) fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_path_prefixes() {
        assert_eq!(long_path("C:\\a\\b"), "\\\\?\\C:\\a\\b");
        assert_eq!(long_path("c:/a/b"), "\\\\?\\c:\\a\\b");
        assert_eq!(long_path("C:\\"), "\\\\?\\C:\\");
        assert_eq!(long_path("\\\\srv\\share\\x"), "\\\\?\\UNC\\srv\\share\\x");
        assert_eq!(long_path("\\\\?\\C:\\x"), "\\\\?\\C:\\x");
        assert_eq!(long_path("\\\\.\\C:"), "\\\\.\\C:");
        assert_eq!(long_path("rel\\x"), "rel\\x");
        assert_eq!(long_path("C:\\a\\..\\b"), "C:\\a\\..\\b");
        assert_eq!(strip_long_prefix("\\\\?\\C:\\x"), "C:\\x");
        assert_eq!(strip_long_prefix("\\\\?\\UNC\\s\\h\\x"), "\\\\s\\h\\x");
        assert_eq!(strip_long_prefix("C:\\x"), "C:\\x");
    }

    #[test]
    fn times_and_attrs() {
        assert_eq!(filetime_to_unix(0), 0);
        assert_eq!(filetime_to_unix(116_444_736_000_000_000), 0);
        assert_eq!(filetime_to_unix(116_444_736_000_000_000 + 10_000_000), 1);
        let a = attrs_from(fa::HIDDEN | fa::SYSTEM | fa::COMPRESSED | fa::READONLY);
        assert!(a.has(Attrs::HIDDEN | Attrs::SYSTEM | Attrs::COMPRESSED | Attrs::READONLY));
        assert!(attrs_from(fa::RECALL_ON_DATA_ACCESS).has(Attrs::OFFLINE));
        assert!(attrs_from(fa::SPARSE_FILE).has(Attrs::SPARSE));
    }

    #[test]
    fn reparse_classification() {
        const SYMLINK: u32 = 0xA000_000C;
        const MOUNT_POINT: u32 = 0xA000_0003;
        const DEDUP: u32 = 0x8000_0013;
        const CLOUD: u32 = 0x9000_001A;
        let rp = fa::REPARSE_POINT;
        assert_eq!(classify(rp, SYMLINK).0, Kind::Link);
        assert_eq!(classify(rp | fa::DIRECTORY, MOUNT_POINT).0, Kind::Link);
        let (k, a) = classify(rp, DEDUP);
        assert_eq!(k, Kind::File);
        assert!(a.has(Attrs::REPARSE));
        assert_eq!(classify(rp | fa::DIRECTORY, CLOUD).0, Kind::Dir);
        assert_eq!(classify(fa::DIRECTORY, 0).0, Kind::Dir);
        let e = entry_from(
            "l".into(),
            &RawMeta {
                attr: rp,
                reparse_tag: SYMLINK,
                end_of_file: 99,
                allocation: 4096,
                ..RawMeta::default()
            },
        );
        assert_eq!((e.kind, e.logical, e.allocated), (Kind::Link, 0, Some(0)));
    }

    #[test]
    fn readers_are_bounds_checked() {
        let b = [1u8, 0, 0, 0, 2, 0, 0, 0];
        assert_eq!(rd_u32(&b, 0), Some(1));
        assert_eq!(rd_u64(&b, 0), Some(0x2_0000_0001));
        assert_eq!(rd_u32(&b, 6), None);
        assert_eq!(rd_u32(&b, usize::MAX), None);
        let s = utf16le("hé");
        assert_eq!(rd_utf16(&s, 0, 4).as_deref(), Some("hé"));
        assert_eq!(rd_utf16(&s, 0, 6), None);
    }
}
