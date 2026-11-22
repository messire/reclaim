//! `Fs` port: handle-based listing and metadata.

use std::io::Read;
use std::mem::offset_of;

use reclaim_storage_core::entry::{EntryMeta, ErrorKind};
use reclaim_storage_core::path::PathStyle;
use reclaim_storage_core::ports::Fs;
use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_STANDARD_INFO, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
    GetFileInformationByHandleEx, SYNCHRONIZE,
};

use super::{
    SHARE_ALL, as_bytes, file_index, file_info, info_ex, last_error, map_error, map_io, open,
};
use crate::common::dirinfo::{self, off};
use crate::common::{RawMeta, entry_from, long_path};

// The parser reads FILE_ID_BOTH_DIR_INFO by byte offsets; they must match the SDK.
const _: () = {
    assert!(offset_of!(FILE_ID_BOTH_DIR_INFO, NextEntryOffset) == off::NEXT_ENTRY_OFFSET);
    assert!(offset_of!(FILE_ID_BOTH_DIR_INFO, LastWriteTime) == off::LAST_WRITE_TIME);
    assert!(offset_of!(FILE_ID_BOTH_DIR_INFO, EndOfFile) == off::END_OF_FILE);
    assert!(offset_of!(FILE_ID_BOTH_DIR_INFO, AllocationSize) == off::ALLOCATION_SIZE);
    assert!(offset_of!(FILE_ID_BOTH_DIR_INFO, FileAttributes) == off::FILE_ATTRIBUTES);
    assert!(offset_of!(FILE_ID_BOTH_DIR_INFO, FileNameLength) == off::FILE_NAME_LENGTH);
    assert!(offset_of!(FILE_ID_BOTH_DIR_INFO, EaSize) == off::EA_SIZE);
    assert!(offset_of!(FILE_ID_BOTH_DIR_INFO, FileId) == off::FILE_ID);
    assert!(offset_of!(FILE_ID_BOTH_DIR_INFO, FileName) == off::FILE_NAME);
};

/// Listing buffer: 64 KiB, 8-byte aligned.
const LIST_BUF_WORDS: usize = 64 * 1024 / 8;
/// `ERROR_INVALID_DATA`: a listing record did not fit its buffer.
const ERROR_INVALID_DATA: i32 = 13;

/// Windows filesystem adapter. Links and reparse points are never followed; paths are
/// passed to the OS with the long-path prefix and reported without it.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinFs;

impl Fs for WinFs {
    fn style(&self) -> PathStyle {
        PathStyle::Windows
    }

    fn stat(&self, path: &str) -> Result<EntryMeta, ErrorKind> {
        let h = open(
            &long_path(path),
            FILE_READ_ATTRIBUTES,
            SHARE_ALL,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .map_err(map_error)?;
        let basic: FILE_BASIC_INFO = info_ex(&h).map_err(map_error)?;
        let std_info: FILE_STANDARD_INFO = info_ex(&h).map_err(map_error)?;
        let tag: FILE_ATTRIBUTE_TAG_INFO = info_ex(&h).map_err(map_error)?;
        let bh = file_info(&h).map_err(map_error)?;
        let raw = RawMeta {
            attr: tag.FileAttributes,
            reparse_tag: tag.ReparseTag,
            end_of_file: std_info.EndOfFile,
            allocation: std_info.AllocationSize,
            last_write: basic.LastWriteTime,
            file_id: Some(file_index(&bh)),
            nlink: Some(std_info.NumberOfLinks),
            volume: Some(u64::from(bh.dwVolumeSerialNumber)),
        };
        Ok(entry_from(path.to_owned(), &raw))
    }

    fn read_dir(&self, path: &str, out: &mut Vec<EntryMeta>) -> Result<(), ErrorKind> {
        let h = open(
            &long_path(path),
            FILE_LIST_DIRECTORY | SYNCHRONIZE,
            SHARE_ALL,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .map_err(map_error)?;
        let volume = file_info(&h)
            .ok()
            .map(|i| u64::from(i.dwVolumeSerialNumber));
        let mut buf = vec![0u64; LIST_BUF_WORDS];
        let mut class = FileIdBothDirectoryRestartInfo;
        loop {
            // SAFETY: `h` is a directory handle opened for listing; the buffer is
            // `size_of_val(buf)` writable bytes, 8-byte aligned as the class requires.
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    h.raw(),
                    class,
                    buf.as_mut_ptr().cast(),
                    std::mem::size_of_val(buf.as_slice()) as u32,
                )
            };
            if ok == 0 {
                let e = last_error();
                if e == ERROR_NO_MORE_FILES {
                    return Ok(());
                }
                return Err(map_error(e));
            }
            if dirinfo::parse(as_bytes(&buf), volume, out).is_none() {
                return Err(ErrorKind::Os(ERROR_INVALID_DATA));
            }
            class = FileIdBothDirectoryInfo;
        }
    }

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, ErrorKind> {
        let f = std::fs::File::open(long_path(path)).map_err(|e| map_io(&e))?;
        Ok(Box::new(f))
    }
}
