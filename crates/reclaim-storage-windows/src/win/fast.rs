//! Accelerated whole-volume NTFS scan from the documented `FSCTL_QUERY_FILE_LAYOUT`
//! (winioctl.h): the file system returns every file record with its names, streams and
//! times in large batches, instead of one directory listing per folder.
//!
//! Requirements: an elevated process (the volume is opened for reading), an NTFS
//! volume, a volume root and an empty index. Differences to the listing scanner:
//! the reparse tag is not part of the output, so a reparse *directory* without children
//! counts as a link (junctions, mount points and directory symlinks are empty) and a
//! reparse *file* as a file marked `REPARSE`; NTFS metadata files (`$MFT`, `$LogFile`, …)
//! are included as system files because they occupy the disk, except `$BadClus:$Bad`,
//! which maps the whole volume.

use std::fmt;
use std::mem::offset_of;
use std::sync::RwLock;
use std::time::Instant;

use reclaim_storage_core::tree::{TOP, Tree};
use reclaim_storage_engine::ScanControl;
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_HANDLE_EOF, ERROR_INVALID_PARAMETER, GENERIC_READ,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FILE_LAYOUT_ENTRY, FILE_LAYOUT_INFO_ENTRY, FILE_LAYOUT_INFO_ENTRY_0, FILE_LAYOUT_NAME_ENTRY,
    FSCTL_QUERY_FILE_LAYOUT, QUERY_FILE_LAYOUT_FILTER_TYPE_NONE,
    QUERY_FILE_LAYOUT_INCLUDE_EXTRA_INFO, QUERY_FILE_LAYOUT_INCLUDE_NAMES,
    QUERY_FILE_LAYOUT_INCLUDE_STREAMS,
    QUERY_FILE_LAYOUT_INCLUDE_STREAMS_WITH_NO_CLUSTERS_ALLOCATED, QUERY_FILE_LAYOUT_INPUT,
    QUERY_FILE_LAYOUT_OUTPUT, QUERY_FILE_LAYOUT_RESTART, STREAM_LAYOUT_ENTRY,
};

use super::volumes::{volume_device, volume_information, volume_root};
use super::{SHARE_ALL, as_bytes, file_index, file_info, last_error, open};
use crate::common::layout::{Layout, off};
use crate::common::long_path;

// The parser reads the layout structures by byte offsets; they must match the SDK.
const _: () = {
    assert!(offset_of!(QUERY_FILE_LAYOUT_OUTPUT, FileEntryCount) == off::OUT_FILE_ENTRY_COUNT);
    assert!(offset_of!(QUERY_FILE_LAYOUT_OUTPUT, FirstFileOffset) == off::OUT_FIRST_FILE_OFFSET);
    assert!(offset_of!(FILE_LAYOUT_ENTRY, NextFileOffset) == off::FILE_NEXT);
    assert!(offset_of!(FILE_LAYOUT_ENTRY, FileAttributes) == off::FILE_ATTRIBUTES);
    assert!(offset_of!(FILE_LAYOUT_ENTRY, FileReferenceNumber) == off::FILE_REFERENCE);
    assert!(offset_of!(FILE_LAYOUT_ENTRY, FirstNameOffset) == off::FILE_FIRST_NAME);
    assert!(offset_of!(FILE_LAYOUT_ENTRY, FirstStreamOffset) == off::FILE_FIRST_STREAM);
    assert!(offset_of!(FILE_LAYOUT_ENTRY, ExtraInfoOffset) == off::FILE_EXTRA_INFO);
    assert!(offset_of!(FILE_LAYOUT_ENTRY, ExtraInfoLength) == off::FILE_EXTRA_INFO_LEN);
    assert!(offset_of!(FILE_LAYOUT_NAME_ENTRY, NextNameOffset) == off::NAME_NEXT);
    assert!(offset_of!(FILE_LAYOUT_NAME_ENTRY, Flags) == off::NAME_FLAGS);
    assert!(offset_of!(FILE_LAYOUT_NAME_ENTRY, ParentFileReferenceNumber) == off::NAME_PARENT);
    assert!(offset_of!(FILE_LAYOUT_NAME_ENTRY, FileNameLength) == off::NAME_LENGTH);
    assert!(offset_of!(FILE_LAYOUT_NAME_ENTRY, FileName) == off::NAME_TEXT);
    assert!(offset_of!(STREAM_LAYOUT_ENTRY, NextStreamOffset) == off::STREAM_NEXT);
    assert!(offset_of!(STREAM_LAYOUT_ENTRY, AllocationSize) == off::STREAM_ALLOCATION);
    assert!(offset_of!(STREAM_LAYOUT_ENTRY, EndOfFile) == off::STREAM_EOF);
    assert!(offset_of!(STREAM_LAYOUT_ENTRY, AttributeTypeCode) == off::STREAM_TYPE);
    assert!(offset_of!(STREAM_LAYOUT_ENTRY, StreamIdentifierLength) == off::STREAM_ID_LENGTH);
    assert!(offset_of!(STREAM_LAYOUT_ENTRY, StreamIdentifier) == off::STREAM_ID);
    assert!(offset_of!(FILE_LAYOUT_INFO_ENTRY, BasicInformation) == 0);
    assert!(offset_of!(FILE_LAYOUT_INFO_ENTRY_0, LastWriteTime) == off::INFO_LAST_WRITE);
};

/// Output buffer per call: 1 MiB, 8-byte aligned.
const OUT_BUF_WORDS: usize = 1024 * 1024 / 8;

/// Why the fast scan cannot run (the caller falls back to the listing scanner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastError {
    /// The process is not elevated (the volume cannot be opened for reading).
    NotAdmin,
    /// The volume is not NTFS.
    NotNtfs,
    /// The path is not the root of a volume.
    NotWholeVolume,
    /// Any other Win32 error (`ERROR_INVALID_PARAMETER` for a non-empty index).
    Os(u32),
}

impl fmt::Display for FastError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FastError::NotAdmin => f.write_str("the fast scan needs administrator rights"),
            FastError::NotNtfs => f.write_str("the fast scan needs an NTFS volume"),
            FastError::NotWholeVolume => f.write_str("the fast scan only scans whole volumes"),
            FastError::Os(c) => write!(f, "the fast scan failed (Windows error {c})"),
        }
    }
}

impl std::error::Error for FastError {}

/// Result of a fast scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FastSummary {
    /// Non-directory entries inserted (one per hard-link name).
    pub files: u64,
    /// Directories inserted.
    pub dirs: u64,
    /// Logical bytes of the volume root (hard links counted once).
    pub logical: u64,
    /// Allocated bytes of the volume root (hard links counted once).
    pub allocated: u64,
    /// Wall time.
    pub elapsed_ms: u64,
    /// Stopped through the control; the index is partial (or empty when cancelled
    /// before the layout was complete).
    pub cancelled: bool,
    /// Names whose parent directory was never reached (dropped).
    pub orphans: u64,
}

fn same_root(a: &str, b: &str) -> bool {
    a.trim_end_matches('\\')
        .eq_ignore_ascii_case(b.trim_end_matches('\\'))
}

/// Scans a whole NTFS volume (`"C:\\"`) into an empty `tree`. `on_progress` receives
/// the number of file records read so far after every batch.
pub fn fast_scan(
    volume_root_path: &str,
    tree: &RwLock<Tree>,
    control: &ScanControl,
    on_progress: &dyn Fn(u64),
) -> Result<FastSummary, FastError> {
    let started = Instant::now();
    if !tree.read().unwrap_or_else(|p| p.into_inner()).is_empty() {
        return Err(FastError::Os(ERROR_INVALID_PARAMETER));
    }
    let mut root = volume_root_path.replace('/', "\\");
    if !root.ends_with('\\') {
        root.push('\\');
    }
    match volume_root(&root) {
        Some(r) if same_root(&r, &root) => {}
        _ => return Err(FastError::NotWholeVolume),
    }
    let (_, fs_name, _) = volume_information(&root).ok_or_else(|| FastError::Os(last_error()))?;
    if !fs_name.eq_ignore_ascii_case("NTFS") {
        return Err(FastError::NotNtfs);
    }
    let device = volume_device(&root).ok_or(FastError::NotWholeVolume)?;
    let volume = open(&device, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, 0).map_err(
        |e| match e {
            ERROR_ACCESS_DENIED => FastError::NotAdmin,
            e => FastError::Os(e),
        },
    )?;
    let root_dir = open(
        &long_path(&root),
        FILE_READ_ATTRIBUTES,
        SHARE_ALL,
        FILE_FLAG_BACKUP_SEMANTICS,
    )
    .map_err(FastError::Os)?;
    let info = file_info(&root_dir).map_err(FastError::Os)?;
    drop(root_dir);
    let root_frn = file_index(&info);
    let serial = u64::from(info.dwVolumeSerialNumber);

    let mut layout = Layout::default();
    let mut buf = vec![0u64; OUT_BUF_WORDS];
    let mut first = true;
    loop {
        if control.cancelled() {
            return Ok(FastSummary {
                elapsed_ms: started.elapsed().as_millis() as u64,
                cancelled: true,
                ..FastSummary::default()
            });
        }
        let mut input = QUERY_FILE_LAYOUT_INPUT::default();
        input.Anonymous.FilterEntryCount = 0;
        input.Flags = QUERY_FILE_LAYOUT_INCLUDE_NAMES
            | QUERY_FILE_LAYOUT_INCLUDE_STREAMS
            | QUERY_FILE_LAYOUT_INCLUDE_STREAMS_WITH_NO_CLUSTERS_ALLOCATED
            | QUERY_FILE_LAYOUT_INCLUDE_EXTRA_INFO
            | if first { QUERY_FILE_LAYOUT_RESTART } else { 0 };
        input.FilterType = QUERY_FILE_LAYOUT_FILTER_TYPE_NONE;
        let mut returned = 0u32;
        // SAFETY: `volume` is a volume handle opened for reading; the input struct and
        // the output buffer (8-byte aligned) are live and sized as passed; the call is
        // synchronous.
        let ok = unsafe {
            DeviceIoControl(
                volume.raw(),
                FSCTL_QUERY_FILE_LAYOUT,
                (&raw const input).cast(),
                std::mem::size_of::<QUERY_FILE_LAYOUT_INPUT>() as u32,
                buf.as_mut_ptr().cast(),
                std::mem::size_of_val(buf.as_slice()) as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            match last_error() {
                ERROR_HANDLE_EOF => break,
                ERROR_ACCESS_DENIED => return Err(FastError::NotAdmin),
                e => return Err(FastError::Os(e)),
            }
        }
        first = false;
        let bytes = as_bytes(&buf);
        let n = layout.parse_batch(&bytes[..(returned as usize).min(bytes.len())]);
        on_progress(layout.file_count());
        if n == 0 {
            // Nothing parsable: stop rather than ask for the same batch again.
            break;
        }
    }
    drop(buf);
    drop(volume);

    let stats = layout.build(tree, volume_root_path, root_frn, serial, &|| {
        control.cancelled()
    });
    let t = tree.read().unwrap_or_else(|p| p.into_inner());
    let top = t.node(TOP);
    Ok(FastSummary {
        files: stats.files,
        dirs: stats.dirs,
        logical: top.logical,
        allocated: top.allocated,
        elapsed_ms: started.elapsed().as_millis() as u64,
        cancelled: stats.cancelled,
        orphans: stats.orphans,
    })
}
