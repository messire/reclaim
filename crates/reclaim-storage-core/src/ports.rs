//! Ports implemented by platform adapters (Windows: `reclaim-storage-windows`; any OS:
//! the `std` adapter in `reclaim-storage-engine`; tests: in-memory fakes).

use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::cleanup::ItemOutcome;
use crate::entry::{EntryMeta, ErrorKind};
use crate::guard::Guard;
use crate::path::PathStyle;

/// Read-only filesystem access for scanning and hashing. Links are never followed.
pub trait Fs: Send + Sync {
    /// Path style of this filesystem.
    fn style(&self) -> PathStyle;

    /// Metadata of `path` itself (a link is reported as a link). The returned name is
    /// `path` as given.
    fn stat(&self, path: &str) -> Result<EntryMeta, ErrorKind>;

    /// Appends the entries of directory `path` to `out` (without `.` and `..`).
    fn read_dir(&self, path: &str, out: &mut Vec<EntryMeta>) -> Result<(), ErrorKind>;

    /// Opens a file for sequential reading.
    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, ErrorKind>;

    /// Joins a directory path and a name.
    fn join(&self, dir: &str, name: &str) -> String {
        let sep = self.style().sep();
        if dir.ends_with(sep) || (sep == '\\' && dir.ends_with('/')) {
            format!("{dir}{name}")
        } else {
            format!("{dir}{sep}{name}")
        }
    }
}

/// Kind of drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveKind {
    /// Internal disk.
    Fixed,
    /// USB stick, card reader.
    Removable,
    /// Network share.
    Network,
    /// CD/DVD.
    Optical,
    /// RAM disk.
    Ram,
    /// Anything else.
    Unknown,
}

/// A mounted volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    /// Root path (`C:\`, `/`).
    pub root: String,
    /// Label.
    pub label: String,
    /// Filesystem name (NTFS, ReFS, exFAT, ext4…).
    pub fs: String,
    /// Kind.
    pub kind: DriveKind,
    /// Capacity in bytes.
    pub total: u64,
    /// Free bytes available to the user.
    pub free: u64,
    /// Rotational disk (seek penalty), when the OS reports it.
    pub seek_penalty: Option<bool>,
}

/// Volume enumeration and free-space measurement.
pub trait Volumes: Send + Sync {
    /// Mounted volumes worth scanning.
    fn list(&self) -> Vec<VolumeInfo>;

    /// The volume a path is on, with current free space.
    fn volume_of(&self, path: &str) -> Option<VolumeInfo>;
}

/// A file or folder to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Full path.
    pub path: String,
    /// Cleanup: the allowed root the target must stay strictly inside. `None`: a user
    /// selection, checked with [`Guard::check_user_delete`].
    pub root: Option<String>,
    /// Identity seen at discovery; a different identity means the path now names
    /// another object and the target is skipped.
    pub file_id: Option<u64>,
    /// Last write time (Unix seconds) seen at discovery; a different time means the
    /// object was rewritten since it was reviewed and the target is skipped.
    pub mtime: Option<i64>,
    /// Allocated bytes seen at discovery (reported, not trusted for checks).
    pub allocated: u64,
    /// A directory (user selections only; cleanup removes files).
    pub is_dir: bool,
}

impl Target {
    /// Runs the guard check that applies to this target on `final_path` (the path the
    /// opened object actually has).
    pub fn check(&self, guard: &Guard, final_path: &str) -> Result<(), crate::guard::Denied> {
        match &self.root {
            Some(root) => guard.check_cleanup(final_path, root),
            None => guard.check_user_delete(final_path),
        }
    }
}

/// Destructive operations. Every method revalidates the target immediately before
/// acting: it must not be a link or reparse point, must still have the identity seen
/// at discovery, and its final path must pass the guard.
pub trait Remover: Send + Sync {
    /// Deletes a file permanently (cleanup of caches and temporary files).
    fn delete(&self, t: &Target, guard: &Guard) -> ItemOutcome;

    /// Removes an empty directory strictly inside `root` (after cleanup). Returns
    /// whether it was removed; a non-empty or protected directory is left alone.
    fn remove_empty_dir(&self, path: &str, root: &str, guard: &Guard) -> bool;

    /// Moves a file or folder to the Recycle Bin (user selections, review categories).
    fn recycle(&self, t: &Target, guard: &Guard) -> ItemOutcome;

    /// Items and bytes in the Recycle Bin of all drives, if the platform has one.
    fn recycle_bin(&self) -> Option<(u64, u64)>;

    /// Empties the Recycle Bin of all drives.
    fn empty_recycle_bin(&self) -> Result<(), ErrorKind>;

    /// Opens the operating system's own cleanup tool, if there is one.
    fn open_system_cleanup(&self) -> Result<(), ErrorKind>;
}
