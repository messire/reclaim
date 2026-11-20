//! Small safe wrappers around the Win32 calls the adapter uses.

use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    PROCESS_MEMORY_COUNTERS_EX2,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows_sys::Win32::System::Threading::{
    GetProcessIoCounters, GetProcessTimes, IO_COUNTERS, IsProcessCritical, OpenProcess,
    OpenProcessToken, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32, QueryFullProcessImageNameW,
    WaitForSingleObject,
};

use reclaim_core::model::MemorySnapshot;
use reclaim_core::units::Bytes;

/// `ERROR_ACCESS_DENIED`.
pub const ERROR_ACCESS_DENIED: u32 = 5;
/// `ERROR_INVALID_PARAMETER`: returned by `OpenProcess` for a PID that no longer exists.
pub const ERROR_INVALID_PARAMETER: u32 = 87;

/// The calling thread's last error.
pub fn last_error() -> u32 {
    // SAFETY: GetLastError only reads thread-local state.
    unsafe { GetLastError() }
}

/// An owned kernel handle, closed on drop. No raw handle leaves the crate.
#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

// SAFETY: kernel handles are process-wide and may be used and closed from any thread.
unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    /// Wraps a handle returned by the OS; `None` for null or `INVALID_HANDLE_VALUE`.
    pub fn new(h: HANDLE) -> Option<Self> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(h))
        }
    }

    /// The raw handle, valid while `self` lives.
    pub fn raw(&self) -> HANDLE {
        self.0
    }

    /// Opens a process with the given rights.
    pub fn open_process(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Result<Self, u32> {
        // SAFETY: OpenProcess has no memory-safety preconditions; null signals an error.
        let h = unsafe { OpenProcess(access, 0, pid) };
        Self::new(h).ok_or_else(last_error)
    }

    /// True if the handle (opened with `SYNCHRONIZE`) is signaled, i.e. the process exited.
    pub fn is_signaled(&self) -> bool {
        // SAFETY: valid handle; zero timeout never blocks.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Converts a NUL-terminated UTF-16 buffer.
pub fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// All processes: (pid, image name).
pub fn snapshot_processes() -> Result<Vec<(u32, String)>, u32> {
    // SAFETY: no preconditions; INVALID_HANDLE_VALUE signals failure.
    let snap = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
        .ok_or_else(last_error)?;
    let mut out = Vec::with_capacity(512);
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `e` is live with dwSize set, as required.
    let mut more = unsafe { Process32FirstW(snap.raw(), &mut e) } != 0;
    while more {
        out.push((e.th32ProcessID, from_wide(&e.szExeFile)));
        // SAFETY: as above.
        more = unsafe { Process32NextW(snap.raw(), &mut e) } != 0;
    }
    Ok(out)
}

/// Memory counters; private working set when the build provides `EX2`.
pub fn memory(h: &OwnedHandle) -> Result<MemorySnapshot, u32> {
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut c2: PROCESS_MEMORY_COUNTERS_EX2 = unsafe { std::mem::zeroed() };
    c2.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
    // SAFETY: the pointer refers to a live struct of `cb` bytes; the API accepts EX2 via cast.
    let ok = unsafe {
        GetProcessMemoryInfo(
            h.raw(),
            (&raw mut c2).cast::<PROCESS_MEMORY_COUNTERS>(),
            c2.cb,
        )
    };
    let mut snap = if ok != 0 {
        MemorySnapshot {
            working_set: Bytes(c2.WorkingSetSize as u64),
            private_working_set: Some(Bytes(c2.PrivateWorkingSetSize as u64)),
            private_bytes: Bytes(c2.PrivateUsage as u64),
            page_faults: u64::from(c2.PageFaultCount),
            io_bytes: None,
        }
    } else {
        // Older builds reject the EX2 size: fall back to EX (risk O5).
        // SAFETY: zeroed is a valid bit pattern for this plain C struct.
        let mut c: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        // SAFETY: as above, for the EX struct.
        let ok = unsafe {
            GetProcessMemoryInfo(
                h.raw(),
                (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(),
                c.cb,
            )
        };
        if ok == 0 {
            return Err(last_error());
        }
        MemorySnapshot {
            working_set: Bytes(c.WorkingSetSize as u64),
            private_working_set: None,
            private_bytes: Bytes(c.PrivateUsage as u64),
            page_faults: u64::from(c.PageFaultCount),
            io_bytes: None,
        }
    };
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut io: IO_COUNTERS = unsafe { std::mem::zeroed() };
    // SAFETY: `io` is a live IO_COUNTERS.
    if unsafe { GetProcessIoCounters(h.raw(), &mut io) } != 0 {
        snap.io_bytes = Some(Bytes(
            io.ReadTransferCount.saturating_add(io.WriteTransferCount),
        ));
    }
    Ok(snap)
}

/// Process creation time (FILETIME as u64).
pub fn start_time(h: &OwnedHandle) -> Result<u64, u32> {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
    // SAFETY: all four pointers refer to distinct live FILETIME values.
    let ok = unsafe { GetProcessTimes(h.raw(), &mut c, &mut e, &mut k, &mut u) };
    if ok != 0 {
        Ok((u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime))
    } else {
        Err(last_error())
    }
}

/// `IsProcessCritical`.
pub fn is_critical(h: &OwnedHandle) -> Option<bool> {
    let mut v = 0;
    // SAFETY: `v` is a live BOOL.
    (unsafe { IsProcessCritical(h.raw(), &mut v) } != 0).then_some(v != 0)
}

/// Whether the process token is elevated.
pub fn token_elevated(h: &OwnedHandle) -> Option<bool> {
    let mut tok: HANDLE = std::ptr::null_mut();
    // SAFETY: `tok` is a live HANDLE slot.
    if unsafe { OpenProcessToken(h.raw(), TOKEN_QUERY, &mut tok) } == 0 {
        return None;
    }
    let tok = OwnedHandle::new(tok)?;
    let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    // SAFETY: buffer pointer and size describe the live `e`.
    let ok = unsafe {
        GetTokenInformation(
            tok.raw(),
            TokenElevation,
            (&raw mut e).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    (ok != 0).then_some(e.TokenIsElevated != 0)
}

/// Session id of a process.
pub fn session_of(pid: u32) -> Option<u32> {
    let mut s = 0u32;
    // SAFETY: `s` is a live u32.
    (unsafe { ProcessIdToSessionId(pid, &mut s) } != 0).then_some(s)
}

/// Full image path (used only to classify the location; never stored).
pub fn image_path(h: &OwnedHandle) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: buffer pointer and length (in characters) describe `buf`.
    let ok = unsafe {
        QueryFullProcessImageNameW(h.raw(), PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
    };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// The Windows directory with a trailing backslash, lower-case.
pub fn windows_dir() -> String {
    let mut buf = [0u16; 512];
    // SAFETY: buffer pointer and size (in characters) describe `buf`.
    let n = unsafe { GetSystemWindowsDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let mut s = if n > 0 && n < buf.len() {
        String::from_utf16_lossy(&buf[..n])
    } else {
        r"C:\Windows".to_owned()
    };
    if !s.ends_with('\\') {
        s.push('\\');
    }
    s.to_lowercase()
}
