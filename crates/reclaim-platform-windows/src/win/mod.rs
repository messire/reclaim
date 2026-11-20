//! The Windows adapter (ARCHITECTURE §6.1, OS-CAPABILITIES §2).

mod ffi;
mod pdh;

use windows_sys::Win32::System::Memory::{
    CreateMemoryResourceNotification, HighMemoryResourceNotification,
    LowMemoryResourceNotification, QueryMemoryResourceNotification,
};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE,
    SetProcessWorkingSetSize,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use reclaim_core::model::{
    AccessHint, Availability, Capabilities, ImageLocation, ImageName, MemorySnapshot, Pressure,
    ProcessInfo, ProcessKey, SystemSnapshot,
};
use reclaim_core::ports::{
    OpenError, PlatformError, ProcessHandle, ProcessSource, ReclaimAction, ReclaimError, Reclaimer,
    SessionInfo, SystemMetrics,
};
use reclaim_core::units::Bytes;

use ffi::{ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, OwnedHandle, last_error};

/// Rights for acting on a process: query, set quota (trim) and synchronize (exit check).
const TRIM_RIGHTS: u32 =
    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA | PROCESS_SYNCHRONIZE;

/// The Windows platform: implements every port of `reclaim-core`.
#[derive(Debug)]
pub struct WindowsPlatform {
    self_pid: u32,
    self_session: Option<u32>,
    self_elevated: bool,
    windows_dir: String,
    low: Option<OwnedHandle>,
    high: Option<OwnedHandle>,
    pdh: Option<pdh::PdhReader>,
    private_ws: bool,
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsPlatform {
    /// Creates the adapter. Cheap: no PDH, no enumeration.
    pub fn new() -> Self {
        // SAFETY: no preconditions.
        let self_pid = unsafe { GetCurrentProcessId() };
        let me = OwnedHandle::open_process(self_pid, PROCESS_QUERY_LIMITED_INFORMATION).ok();
        let self_elevated = me.as_ref().and_then(ffi::token_elevated).unwrap_or(false);
        let private_ws = me
            .as_ref()
            .and_then(|h| ffi::memory(h).ok())
            .is_some_and(|m| m.private_working_set.is_some());
        let notif = |kind| {
            // SAFETY: no preconditions; null signals failure.
            OwnedHandle::new(unsafe { CreateMemoryResourceNotification(kind) })
        };
        Self {
            self_pid,
            self_session: ffi::session_of(self_pid),
            self_elevated,
            windows_dir: ffi::windows_dir(),
            low: notif(LowMemoryResourceNotification),
            high: notif(HighMemoryResourceNotification),
            pdh: None,
            private_ws,
        }
    }

    /// True if Reclaim itself runs elevated.
    pub fn is_elevated(&self) -> bool {
        self.self_elevated
    }

    fn pressure(&self) -> Pressure {
        let state = |h: &Option<OwnedHandle>| {
            h.as_ref().is_some_and(|h| {
                let mut s = 0;
                // SAFETY: valid notification handle; `s` is a live BOOL.
                unsafe { QueryMemoryResourceNotification(h.raw(), &mut s) != 0 && s != 0 }
            })
        };
        if state(&self.low) {
            Pressure::High
        } else if state(&self.high) {
            Pressure::Low
        } else {
            Pressure::Normal
        }
    }

    fn location(&self, h: &OwnedHandle) -> ImageLocation {
        match ffi::image_path(h) {
            Some(p) if p.to_lowercase().starts_with(&self.windows_dir) => ImageLocation::System,
            Some(_) => ImageLocation::Other,
            None => ImageLocation::Unknown,
        }
    }

    fn access(&self, pid: u32, h: Option<&OwnedHandle>) -> AccessHint {
        match (ffi::session_of(pid), self.self_session) {
            (Some(s), Some(me)) if s != me => return AccessHint::OtherUser,
            _ => {}
        }
        match h.and_then(ffi::token_elevated) {
            Some(true) if !self.self_elevated => AccessHint::RequiresElevation,
            Some(_) => AccessHint::Accessible,
            None => AccessHint::Unknown,
        }
    }

    fn describe(&self, pid: u32, image: &str) -> ProcessInfo {
        let h = OwnedHandle::open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok();
        let start = h
            .as_ref()
            .and_then(|h| ffi::start_time(h).ok())
            .unwrap_or(0);
        ProcessInfo {
            key: ProcessKey::new(pid, start),
            image: ImageName::new(image),
            location: h
                .as_ref()
                .map_or(ImageLocation::Unknown, |h| self.location(h)),
            session_id: ffi::session_of(pid).unwrap_or(u32::MAX),
            access: self.access(pid, h.as_ref()),
            critical: h.as_ref().and_then(ffi::is_critical),
            memory: h.as_ref().and_then(|h| ffi::memory(h).ok()),
        }
    }

    fn open_validated(pid: u32, start: u64, access: u32) -> Result<OwnedHandle, OpenError> {
        let h = OwnedHandle::open_process(pid, access).map_err(|code| match code {
            ERROR_ACCESS_DENIED => OpenError::AccessDenied,
            ERROR_INVALID_PARAMETER => OpenError::Exited,
            c => OpenError::Os(i64::from(c)),
        })?;
        let actual = ffi::start_time(&h).map_err(|c| OpenError::Os(i64::from(c)))?;
        if actual != start {
            return Err(OpenError::IdentityChanged);
        }
        Ok(h)
    }
}

/// An open, identity-validated process handle with trim and synchronize rights.
#[derive(Debug)]
pub struct WinHandle {
    key: ProcessKey,
    handle: OwnedHandle,
}

impl ProcessHandle for WinHandle {
    fn key(&self) -> ProcessKey {
        self.key
    }

    fn memory(&self) -> Result<MemorySnapshot, PlatformError> {
        ffi::memory(&self.handle)
            .map_err(|c| PlatformError::new("GetProcessMemoryInfo", Some(i64::from(c))))
    }

    fn has_exited(&self) -> bool {
        self.handle.is_signaled()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl SystemMetrics for WindowsPlatform {
    fn sample(&mut self) -> Result<SystemSnapshot, PlatformError> {
        // SAFETY: zeroed is a valid bit pattern for this plain C struct.
        let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        // SAFETY: `ms` is live with dwLength set.
        if unsafe { GlobalMemoryStatusEx(&mut ms) } == 0 {
            return Err(PlatformError::new(
                "GlobalMemoryStatusEx",
                Some(i64::from(last_error())),
            ));
        }
        Ok(SystemSnapshot {
            phys_total: Bytes(ms.ullTotalPhys),
            phys_available: Bytes(ms.ullAvailPhys),
            commit_charge: Bytes(ms.ullTotalPageFile.saturating_sub(ms.ullAvailPageFile)),
            commit_limit: Bytes(ms.ullTotalPageFile),
            standby: None,
            modified: None,
            pages_input_per_sec: None,
            pressure: self.pressure(),
        })
    }

    /// Loads (`true`) or unloads (`false`) the optional PDH breakdown. Loading costs
    /// ≈ 250 ms and a few MiB, so callers do it off the UI thread and only on demand.
    fn set_details(&mut self, on: bool) -> bool {
        if on && self.pdh.is_none() {
            self.pdh = pdh::PdhReader::open();
        } else if !on {
            self.pdh = None;
        }
        self.pdh.is_some()
    }

    fn sample_detailed(&mut self) -> Result<SystemSnapshot, PlatformError> {
        let mut s = self.sample()?;
        if let Some(p) = self.pdh.as_mut() {
            let v = p.read();
            s.standby = v.standby;
            s.modified = v.modified;
            s.pages_input_per_sec = v.pages_input_per_sec;
        }
        Ok(s)
    }
}

impl ProcessSource for WindowsPlatform {
    fn enumerate(&mut self) -> Result<Vec<ProcessInfo>, PlatformError> {
        let list = ffi::snapshot_processes()
            .map_err(|c| PlatformError::new("CreateToolhelp32Snapshot", Some(i64::from(c))))?;
        Ok(list
            .iter()
            .map(|(pid, image)| self.describe(*pid, image))
            .collect())
    }

    fn open(&mut self, key: ProcessKey) -> Result<Box<dyn ProcessHandle>, OpenError> {
        let handle = Self::open_validated(key.pid, key.start_time, TRIM_RIGHTS)?;
        if handle.is_signaled() {
            return Err(OpenError::Exited);
        }
        Ok(Box::new(WinHandle { key, handle }))
    }
}

impl Reclaimer for WindowsPlatform {
    fn capabilities(&self) -> Capabilities {
        let pdh_reason = || Availability::Unsupported {
            reason: "Performance counters are loaded on demand".to_owned(),
        };
        Capabilities {
            trim_working_set: Availability::Available,
            private_working_set: if self.private_ws {
                Availability::Available
            } else {
                Availability::Unsupported {
                    reason: "Not provided by this Windows build".to_owned(),
                }
            },
            standby_breakdown: if self.pdh.is_some() {
                Availability::Available
            } else {
                pdh_reason()
            },
            paging_io_rate: if self.pdh.is_some() {
                Availability::Available
            } else {
                pdh_reason()
            },
            pressure_signal: if self.low.is_some() && self.high.is_some() {
                Availability::Available
            } else {
                Availability::Unsupported {
                    reason: "Memory resource notifications unavailable".to_owned(),
                }
            },
            per_process_io: Availability::Available,
        }
    }

    fn reclaim(
        &mut self,
        h: &dyn ProcessHandle,
        action: ReclaimAction,
    ) -> Result<(), ReclaimError> {
        let ReclaimAction::TrimWorkingSet = action;
        // Only handles created by this adapter carry the rights the call needs.
        let Some(w) = h.as_any().downcast_ref::<WinHandle>() else {
            return Err(ReclaimError::Unsupported);
        };
        let raw = w.handle.raw();
        // SAFETY: `raw` is a valid handle with PROCESS_SET_QUOTA for the duration of the call.
        // (size_t)-1 for both bounds removes as many pages as possible (documented).
        let ok = unsafe { SetProcessWorkingSetSize(raw, usize::MAX, usize::MAX) };
        if ok != 0 {
            return Ok(());
        }
        let code = last_error();
        // An exited process also fails with ERROR_ACCESS_DENIED: check exit first (spike A13).
        if h.has_exited() {
            return Err(ReclaimError::Exited);
        }
        Err(match code {
            ERROR_ACCESS_DENIED => ReclaimError::AccessDenied,
            c => ReclaimError::Os(i64::from(c)),
        })
    }
}

impl SessionInfo for WindowsPlatform {
    fn self_pid(&self) -> u32 {
        self.self_pid
    }

    fn foreground_process(&self) -> Option<ProcessKey> {
        // SAFETY: no preconditions; null during secure desktop / lock screen.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        // SAFETY: hwnd came from the OS; `pid` is a live u32.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 {
            return None;
        }
        let h = OwnedHandle::open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
        Some(ProcessKey::new(pid, ffi::start_time(&h).ok()?))
    }
}

/// Private bytes of the current process (used by self-measurement and benchmarks).
pub fn own_private_bytes() -> Option<Bytes> {
    // SAFETY: no preconditions.
    let pid = unsafe { GetCurrentProcessId() };
    let h = OwnedHandle::open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    ffi::memory(&h).ok().map(|m| m.private_bytes)
}
