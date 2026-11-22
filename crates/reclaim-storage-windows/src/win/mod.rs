//! Win32 implementation. Every `unsafe` block states why the call is sound.

pub mod fast;
mod fs;
mod known;
mod remove;
pub mod shell;
mod volumes;
pub mod watch;

pub use fs::WinFs;
pub use known::known_folders;
pub use remove::WinRemover;
pub use volumes::WinVolumes;

use std::mem::MaybeUninit;

use reclaim_storage_core::entry::ErrorKind;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_FILENAME_EXCED_RANGE,
    ERROR_LOCK_VIOLATION, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO,
    FILE_INFO_BY_HANDLE_CLASS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_STANDARD_INFO, FileAttributeTagInfo, FileBasicInfo, FileStandardInfo,
    GetFileInformationByHandle, GetFileInformationByHandleEx, OPEN_EXISTING,
};

/// Share mode that never blocks other programs (and never makes our open fail because
/// of theirs, except when they deny sharing themselves).
pub(crate) const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

/// NUL-terminated UTF-16.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 up to the first NUL (or the whole buffer).
pub(crate) fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// The calling thread's last Win32 error.
pub(crate) fn last_error() -> u32 {
    // SAFETY: GetLastError has no preconditions; it reads thread-local state.
    unsafe { GetLastError() }
}

/// Maps a Win32 error code.
pub(crate) fn map_error(code: u32) -> ErrorKind {
    match code {
        ERROR_ACCESS_DENIED => ErrorKind::AccessDenied,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => ErrorKind::NotFound,
        ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => ErrorKind::InUse,
        ERROR_FILENAME_EXCED_RANGE => ErrorKind::TooLong,
        c => ErrorKind::Os(c as i32),
    }
}

/// Maps a `std::io::Error`, preferring its raw Win32 code.
pub(crate) fn map_io(e: &std::io::Error) -> ErrorKind {
    match e.raw_os_error() {
        Some(c) => map_error(c as u32),
        None => ErrorKind::from_io(e),
    }
}

/// An owned kernel handle, closed on drop.
pub(crate) struct Handle(HANDLE);

// SAFETY: a kernel handle is a process-wide value; it may be used and closed from any
// thread. `Handle` owns it exclusively.
unsafe impl Send for Handle {}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Handle({:#x})", self.0 as usize)
    }
}

impl Handle {
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }

    /// Gives up ownership (the caller closes it with [`Handle::from_raw`]).
    pub(crate) fn into_raw(self) -> HANDLE {
        let h = self.0;
        std::mem::forget(self);
        h
    }

    /// Takes ownership of a handle previously released with [`Handle::into_raw`].
    pub(crate) fn from_raw(h: HANDLE) -> Self {
        Self(h)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is valid and owned by `self`; it is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// `CreateFileW(path, access, share, OPEN_EXISTING, flags)`; `Err` carries the Win32
/// error. `os_path` is passed verbatim (callers add the long-path prefix).
pub(crate) fn open(os_path: &str, access: u32, share: u32, flags: u32) -> Result<Handle, u32> {
    let w = wide(os_path);
    // SAFETY: `w` is NUL-terminated and outlives the call; no security attributes or
    // template handle are passed.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            access,
            share,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        Err(last_error())
    } else {
        Ok(Handle(h))
    }
}

/// `GetFileInformationByHandle`.
pub(crate) fn file_info(h: &Handle) -> Result<BY_HANDLE_FILE_INFORMATION, u32> {
    let mut i = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: `h` is a valid handle and `i` is writable storage of the right type.
    let ok = unsafe { GetFileInformationByHandle(h.raw(), i.as_mut_ptr()) };
    if ok == 0 {
        Err(last_error())
    } else {
        // SAFETY: the struct consists of integers only; it was zero-initialized and the
        // call succeeded and filled it.
        Ok(unsafe { i.assume_init() })
    }
}

/// The 64-bit file index (`nFileIndexHigh << 32 | nFileIndexLow`): on NTFS the file
/// reference number, the same value `FILE_ID_BOTH_DIR_INFO.FileId` reports.
pub(crate) fn file_index(i: &BY_HANDLE_FILE_INFORMATION) -> u64 {
    (u64::from(i.nFileIndexHigh) << 32) | u64::from(i.nFileIndexLow)
}

/// A fixed-size `GetFileInformationByHandleEx` class and its struct.
pub(crate) trait InfoClass: Copy {
    const CLASS: FILE_INFO_BY_HANDLE_CLASS;
}

impl InfoClass for FILE_BASIC_INFO {
    const CLASS: FILE_INFO_BY_HANDLE_CLASS = FileBasicInfo;
}

impl InfoClass for FILE_STANDARD_INFO {
    const CLASS: FILE_INFO_BY_HANDLE_CLASS = FileStandardInfo;
}

impl InfoClass for FILE_ATTRIBUTE_TAG_INFO {
    const CLASS: FILE_INFO_BY_HANDLE_CLASS = FileAttributeTagInfo;
}

/// `GetFileInformationByHandleEx` for a fixed-size class.
pub(crate) fn info_ex<T: InfoClass>(h: &Handle) -> Result<T, u32> {
    let mut v = MaybeUninit::<T>::zeroed();
    // SAFETY: `T` is the struct documented for `T::CLASS` (see the impls above) and the
    // buffer is exactly `size_of::<T>()` writable bytes.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            h.raw(),
            T::CLASS,
            v.as_mut_ptr().cast(),
            std::mem::size_of::<T>() as u32,
        )
    };
    if ok == 0 {
        Err(last_error())
    } else {
        // SAFETY: the call succeeded and filled the struct; its fields are integers and
        // BOOLEANs, which the OS writes as 0 or 1.
        Ok(unsafe { v.assume_init() })
    }
}

/// Views a `u64` buffer (8-byte aligned, as the variable-length Windows records
/// require) as bytes.
pub(crate) fn as_bytes(buf: &[u64]) -> &[u8] {
    // SAFETY: any initialized `u64` memory is valid as bytes; the length covers exactly
    // the same allocation and the lifetime is tied to `buf`.
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), std::mem::size_of_val(buf)) }
}
