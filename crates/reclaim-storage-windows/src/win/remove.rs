//! `Remover` port: deletion and recycling of revalidated objects.
//!
//! Revalidation opens the target itself (never following a reparse point) with
//! `DELETE` access and checks, on that handle: not a reparse point, the expected type,
//! the identity seen at discovery, a final path equal to the requested one (otherwise
//! some component went through a link) and the guard on that final path. Deletion then
//! sets the disposition on the same handle, so the object that was checked is the one
//! that is deleted.

use reclaim_storage_core::cleanup::{ItemOutcome, SkipReason};
use reclaim_storage_core::entry::ErrorKind;
use reclaim_storage_core::guard::{Denied, Guard};
use reclaim_storage_core::path::{NormPath, PathStyle};
use reclaim_storage_core::ports::{Remover, Target};
use windows_sys::Win32::Foundation::{
    E_UNEXPECTED, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_LOCK_VIOLATION,
    ERROR_NOT_SUPPORTED, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION, MAX_PATH, S_OK,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_BASIC_INFO, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED,
    FILE_READ_ATTRIBUTES, FileDispositionInfo, FileDispositionInfoEx, GetFinalPathNameByHandleW,
    GetLongPathNameW, SYNCHRONIZE, SetFileInformationByHandle, VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows_sys::Win32::UI::Shell::{
    FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING,
    SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW, SHFILEOPSTRUCTW,
    SHFileOperationW, SHQUERYRBINFO, SHQueryRecycleBinW,
};

use super::volumes::{drive_type, volume_root};
use super::{
    Handle, SHARE_ALL, file_index, file_info, from_wide, info_ex, last_error, map_error, map_io,
    open, wide,
};
use crate::common::{filetime_to_unix, long_path, strip_long_prefix};

/// `ERROR_INVALID_FUNCTION`: some filesystems reject the extended disposition this way.
const ERROR_INVALID_FUNCTION: u32 = 1;
/// `ERROR_CANCELLED`: reported when the shell aborted a recycle operation.
const ERROR_CANCELLED: i32 = 1223;

/// Windows remover (handle-based revalidation, shell Recycle Bin).
#[derive(Debug, Default, Clone, Copy)]
pub struct WinRemover;

/// A target that passed revalidation, still open.
#[derive(Debug)]
struct Validated {
    handle: Handle,
    final_path: String,
}

fn open_outcome(code: u32) -> ItemOutcome {
    match code {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => ItemOutcome::Skipped(SkipReason::Missing),
        ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => ItemOutcome::Skipped(SkipReason::InUse),
        c => ItemOutcome::Failed(map_error(c)),
    }
}

fn dispose_outcome(code: u32) -> ItemOutcome {
    match code {
        ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => ItemOutcome::Skipped(SkipReason::InUse),
        c => ItemOutcome::Failed(map_error(c)),
    }
}

/// The normalized DOS path of an open object, without the `\\?\` prefix.
fn final_path(h: &Handle) -> Result<String, u32> {
    let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
    let mut buf = vec![0u16; 512];
    loop {
        // SAFETY: `h` is a valid handle; `buf` is writable for `buf.len()` characters.
        let n = unsafe {
            GetFinalPathNameByHandleW(h.raw(), buf.as_mut_ptr(), buf.len() as u32, flags)
        };
        if n == 0 {
            return Err(last_error());
        }
        if (n as usize) < buf.len() {
            return Ok(strip_long_prefix(&String::from_utf16_lossy(
                &buf[..n as usize],
            )));
        }
        // Too small: `n` is the required size including the NUL.
        buf = vec![0u16; n as usize + 1];
    }
}

/// The path with 8.3 short components expanded (`GetLongPathNameW` works per component
/// and does not resolve links), without the `\\?\` prefix.
pub(crate) fn long_name(path: &str) -> Option<String> {
    let w = wide(&long_path(path));
    let mut buf = vec![0u16; 512];
    loop {
        // SAFETY: `w` is NUL-terminated; `buf` is writable for `buf.len()` characters.
        let n = unsafe { GetLongPathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if n == 0 {
            return None;
        }
        if (n as usize) < buf.len() {
            return Some(strip_long_prefix(&from_wide(&buf)));
        }
        buf = vec![0u16; n as usize + 1];
    }
}

/// Whether the final path of the opened object is the requested path (case, separators
/// and short names aside).
fn same_path(requested: &NormPath, requested_raw: &str, fin: &str) -> bool {
    let Ok(fin) = NormPath::new(fin, PathStyle::Windows) else {
        return false;
    };
    if *requested == fin {
        return true;
    }
    long_name(requested_raw)
        .and_then(|l| NormPath::new(&l, PathStyle::Windows).ok())
        .is_some_and(|l| l == fin)
}

/// Opens and revalidates `path` (see the module documentation).
fn revalidate(
    path: &str,
    expect_dir: bool,
    file_id: Option<u64>,
    mtime: Option<i64>,
    check: impl FnOnce(&str) -> Result<(), Denied>,
) -> Result<Validated, ItemOutcome> {
    let requested = NormPath::new(path, PathStyle::Windows)
        .map_err(|_| ItemOutcome::Skipped(SkipReason::from_denied(&Denied::BadPath)))?;
    let handle = open(
        &long_path(path),
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARE_ALL,
        FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
    )
    .map_err(open_outcome)?;
    let tag: FILE_ATTRIBUTE_TAG_INFO =
        info_ex(&handle).map_err(|e| ItemOutcome::Failed(map_error(e)))?;
    if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(ItemOutcome::Skipped(SkipReason::Link));
    }
    if (tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != expect_dir {
        return Err(ItemOutcome::Skipped(SkipReason::Changed));
    }
    if let Some(id) = file_id {
        let i = file_info(&handle).map_err(|e| ItemOutcome::Failed(map_error(e)))?;
        if file_index(&i) != id {
            return Err(ItemOutcome::Skipped(SkipReason::Changed));
        }
    }
    if let Some(expected) = mtime {
        // Same handle as the deletion: a file rewritten in place since it was reviewed
        // keeps its id but not its last write time.
        let basic: FILE_BASIC_INFO =
            info_ex(&handle).map_err(|e| ItemOutcome::Failed(map_error(e)))?;
        if filetime_to_unix(basic.LastWriteTime) != expected {
            return Err(ItemOutcome::Skipped(SkipReason::Changed));
        }
    }
    let fin = final_path(&handle).map_err(|e| ItemOutcome::Failed(map_error(e)))?;
    if !same_path(&requested, path, &fin) {
        return Err(ItemOutcome::Skipped(SkipReason::Link));
    }
    check(&fin).map_err(|d| ItemOutcome::Skipped(SkipReason::from_denied(&d)))?;
    Ok(Validated {
        handle,
        final_path: fin,
    })
}

/// Marks the open object for deletion: POSIX semantics (the name goes away at once)
/// ignoring the read-only attribute, or the classic disposition where the extended
/// class is not supported.
fn dispose(h: &Handle) -> Result<(), u32> {
    let ex = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: `h` was opened with DELETE access; `ex` is the struct of this class and
    // outlives the call.
    let ok = unsafe {
        SetFileInformationByHandle(
            h.raw(),
            FileDispositionInfoEx,
            (&raw const ex).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if ok != 0 {
        return Ok(());
    }
    let e = last_error();
    if !matches!(
        e,
        ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION
    ) {
        return Err(e);
    }
    let classic = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: as above, with the struct of the classic disposition class.
    let ok = unsafe {
        SetFileInformationByHandle(
            h.raw(),
            FileDispositionInfo,
            (&raw const classic).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if ok != 0 { Ok(()) } else { Err(last_error()) }
}

fn exists(path: &str) -> bool {
    std::fs::symlink_metadata(long_path(path)).is_ok()
}

impl Remover for WinRemover {
    fn delete(&self, t: &Target, guard: &Guard) -> ItemOutcome {
        let v = match revalidate(&t.path, false, t.file_id, t.mtime, |p| t.check(guard, p)) {
            Ok(v) => v,
            Err(o) => return o,
        };
        match dispose(&v.handle) {
            Ok(()) => ItemOutcome::Deleted(t.allocated),
            Err(e) => dispose_outcome(e),
        }
    }

    fn remove_empty_dir(&self, path: &str, root: &str, guard: &Guard) -> bool {
        if guard.check_cleanup(path, root).is_err() {
            return false;
        }
        let Ok(v) = revalidate(path, true, None, None, |p| guard.check_cleanup(p, root)) else {
            return false;
        };
        // A non-empty directory fails with ERROR_DIR_NOT_EMPTY and stays.
        dispose(&v.handle).is_ok()
    }

    fn recycle(&self, t: &Target, guard: &Guard) -> ItemOutcome {
        // On other drive kinds the shell would delete permanently.
        let fixed = volume_root(&t.path)
            .and_then(|r| drive_type(&r))
            .is_some_and(|k| k == DRIVE_FIXED);
        if !fixed {
            return ItemOutcome::Skipped(SkipReason::Protected(
                "no Recycle Bin on this drive".into(),
            ));
        }
        let v = match revalidate(&t.path, t.is_dir, t.file_id, t.mtime, |p| t.check(guard, p)) {
            Ok(v) => v,
            Err(o) => return o,
        };
        let Validated {
            handle,
            final_path: path,
        } = v;
        // The shell opens the object itself; our DELETE handle would block it.
        drop(handle);
        if path.encode_utf16().count() >= MAX_PATH as usize {
            return ItemOutcome::Failed(ErrorKind::TooLong);
        }
        let mut from: Vec<u16> = path.encode_utf16().collect();
        from.extend([0, 0]);
        let mut op = SHFILEOPSTRUCTW {
            hwnd: std::ptr::null_mut(),
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            pTo: std::ptr::null(),
            // Nuke warning: a file too large for the bin is confirmed, never destroyed
            // silently.
            fFlags: (FOF_ALLOWUNDO
                | FOF_NOCONFIRMATION
                | FOF_NOERRORUI
                | FOF_SILENT
                | FOF_WANTNUKEWARNING) as u16,
            fAnyOperationsAborted: 0,
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: std::ptr::null(),
        };
        // SAFETY: `op` is fully initialized; `from` is a double-NUL-terminated list that
        // outlives the call.
        let r = unsafe { SHFileOperationW(&mut op) };
        if r == 0 && op.fAnyOperationsAborted == 0 && !exists(&path) {
            ItemOutcome::Recycled(t.allocated)
        } else {
            ItemOutcome::Failed(ErrorKind::Os(if r != 0 { r } else { ERROR_CANCELLED }))
        }
    }

    fn recycle_bin(&self) -> Option<(u64, u64)> {
        // SAFETY: SHQUERYRBINFO is plain integers; all-zero is a valid value.
        let mut info: SHQUERYRBINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHQUERYRBINFO>() as u32;
        // SAFETY: a null root queries all drives; `info` is live with cbSize set.
        let hr = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
        (hr == S_OK).then(|| {
            (
                u64::try_from(info.i64NumItems).unwrap_or(0),
                u64::try_from(info.i64Size).unwrap_or(0),
            )
        })
    }

    fn empty_recycle_bin(&self) -> Result<(), ErrorKind> {
        // SAFETY: no owner window and a null root (all drives) are documented values.
        let hr = unsafe {
            SHEmptyRecycleBinW(
                std::ptr::null_mut(),
                std::ptr::null(),
                SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
            )
        };
        if hr == S_OK {
            return Ok(());
        }
        // Some builds report an already empty bin as E_UNEXPECTED.
        if hr == E_UNEXPECTED && self.recycle_bin().is_some_and(|(n, _)| n == 0) {
            return Ok(());
        }
        Err(ErrorKind::Os(hr))
    }

    fn open_system_cleanup(&self) -> Result<(), ErrorKind> {
        let windir = std::env::var("WINDIR")
            .or_else(|_| std::env::var("SystemRoot"))
            .unwrap_or_else(|_| "C:\\Windows".into());
        let exe = format!("{}\\System32\\cleanmgr.exe", windir.trim_end_matches('\\'));
        std::process::Command::new(exe)
            .spawn()
            .map(drop)
            .map_err(|e| map_io(&e))
    }
}
