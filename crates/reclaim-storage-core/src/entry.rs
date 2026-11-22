//! What a filesystem adapter reports for one directory entry, and scan errors.

use serde::{Deserialize, Serialize};

/// Kind of a filesystem object as the scanner sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Kind {
    /// Regular file.
    File = 0,
    /// Directory (descended into).
    Dir = 1,
    /// Symbolic link, junction, mount point or another reparse point. Never followed:
    /// its target is not counted and never visited (ARCHITECTURE §12.1).
    Link = 2,
    /// Device, socket, FIFO or anything else without data.
    Other = 3,
}

/// Attribute bits of an entry. Stored per node in the index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Attrs(pub u16);

impl Attrs {
    /// Hidden (Windows attribute or POSIX dot name is *not* implied).
    pub const HIDDEN: u16 = 1 << 0;
    /// System file (Windows).
    pub const SYSTEM: u16 = 1 << 1;
    /// Filesystem-compressed: allocated size can be far below the logical size.
    pub const COMPRESSED: u16 = 1 << 2;
    /// Sparse file: unallocated ranges read as zeros.
    pub const SPARSE: u16 = 1 << 3;
    /// Reparse point (Windows) or symlink (POSIX).
    pub const REPARSE: u16 = 1 << 4;
    /// Cloud placeholder or offline data: the bytes are not on this disk.
    pub const OFFLINE: u16 = 1 << 5;
    /// Read-only.
    pub const READONLY: u16 = 1 << 6;
    /// Another hard link to the same file was already counted (index-only flag).
    pub const HARDLINK_DUP: u16 = 1 << 8;
    /// The file has more than one hard link (index-only flag).
    pub const HARDLINK: u16 = 1 << 9;
    /// Directory could not be read: its size is unknown (index-only flag).
    pub const ERROR: u16 = 1 << 10;
    /// Directory was not (fully) read because the scan was cancelled (index-only flag).
    pub const INCOMPLETE: u16 = 1 << 11;
    /// Excluded by a user rule; not descended (index-only flag).
    pub const EXCLUDED: u16 = 1 << 12;
    /// Another volume mounted below the scan root; not descended (index-only flag).
    pub const OTHER_VOLUME: u16 = 1 << 13;
    /// Removed from the index by a refresh or a delete (index-only flag).
    pub const DEAD: u16 = 1 << 15;

    /// Whether every bit of `f` is set.
    pub fn has(self, f: u16) -> bool {
        self.0 & f == f
    }

    /// Sets bits.
    pub fn set(&mut self, f: u16) {
        self.0 |= f;
    }

    /// Clears bits.
    pub fn clear(&mut self, f: u16) {
        self.0 &= !f;
    }
}

/// One directory entry as reported by an adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryMeta {
    /// File name (one path component).
    pub name: String,
    /// Kind.
    pub kind: Kind,
    /// Logical size (end of file) in bytes; 0 for directories.
    pub logical: u64,
    /// Bytes allocated on disk, when the adapter knows it.
    pub allocated: Option<u64>,
    /// Last write time, seconds since the Unix epoch (0 when unknown).
    pub mtime: i64,
    /// Attributes.
    pub attrs: Attrs,
    /// File identity on its volume (NTFS file reference, POSIX inode), when known.
    pub file_id: Option<u64>,
    /// Number of hard links, when known (POSIX `st_nlink`; Windows directory listings do
    /// not report it).
    pub nlink: Option<u32>,
    /// Volume identity (POSIX `st_dev`, Windows volume serial), when known.
    pub volume: Option<u64>,
}

impl EntryMeta {
    /// A minimal file entry (tests and fakes).
    pub fn file(name: &str, size: u64) -> Self {
        Self {
            name: name.to_owned(),
            kind: Kind::File,
            logical: size,
            allocated: Some(size.div_ceil(4096) * 4096),
            mtime: 0,
            attrs: Attrs::default(),
            file_id: None,
            nlink: None,
            volume: None,
        }
    }

    /// A minimal directory entry (tests and fakes).
    pub fn dir(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: Kind::Dir,
            logical: 0,
            allocated: Some(0),
            mtime: 0,
            attrs: Attrs::default(),
            file_id: None,
            nlink: None,
            volume: None,
        }
    }
}

/// Why reading a path failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorKind {
    /// Permission denied.
    AccessDenied,
    /// Disappeared between listing and reading (or never existed).
    NotFound,
    /// Path longer than the platform can address.
    TooLong,
    /// Locked by another process (sharing violation).
    InUse,
    /// The object changed since it was scanned (identity or type mismatch).
    Changed,
    /// Any other OS error, with its code.
    Os(i32),
}

impl ErrorKind {
    /// Short stable identifier (exports, logs).
    pub fn id(self) -> String {
        match self {
            ErrorKind::AccessDenied => "access_denied".into(),
            ErrorKind::NotFound => "not_found".into(),
            ErrorKind::TooLong => "too_long".into(),
            ErrorKind::InUse => "in_use".into(),
            ErrorKind::Changed => "changed".into(),
            ErrorKind::Os(c) => format!("os_{c}"),
        }
    }

    /// Maps a `std::io::Error` (adapters may map raw OS codes more precisely).
    pub fn from_io(e: &std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        match e.kind() {
            K::PermissionDenied => ErrorKind::AccessDenied,
            K::NotFound => ErrorKind::NotFound,
            K::InvalidFilename => ErrorKind::TooLong,
            K::ResourceBusy => ErrorKind::InUse,
            _ => ErrorKind::Os(e.raw_os_error().unwrap_or(-1)),
        }
    }
}

/// A path that could not be read during a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanError {
    /// Full path (display form).
    pub path: String,
    /// What went wrong.
    pub kind: ErrorKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attrs_bits() {
        let mut a = Attrs::default();
        a.set(Attrs::HIDDEN | Attrs::SPARSE);
        assert!(a.has(Attrs::HIDDEN));
        assert!(a.has(Attrs::HIDDEN | Attrs::SPARSE));
        a.clear(Attrs::HIDDEN);
        assert!(!a.has(Attrs::HIDDEN));
        assert!(a.has(Attrs::SPARSE));
    }

    #[test]
    fn error_ids() {
        assert_eq!(ErrorKind::Os(5).id(), "os_5");
        assert_eq!(ErrorKind::AccessDenied.id(), "access_denied");
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(ErrorKind::from_io(&e), ErrorKind::AccessDenied);
        let e = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(ErrorKind::from_io(&e), ErrorKind::NotFound);
        assert_eq!(EntryMeta::file("a", 1).allocated, Some(4096));
        assert_eq!(EntryMeta::dir("d").kind, Kind::Dir);
    }
}
