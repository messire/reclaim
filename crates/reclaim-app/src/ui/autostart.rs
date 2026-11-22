//! Start with Windows: the per-user `Run` key, shared with the installer.
//!
//! The value `Reclaim` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` holds
//! `"<path>\reclaim.exe" --tray`. The installer's *Start Reclaim with Windows* checkbox
//! writes the same value (installer/reclaim.iss), so the two always agree. No
//! administrator rights are needed.
//!
//! Windows also keeps a per-entry on/off flag under `...\Explorer\StartupApproved\Run`,
//! set when the user turns an entry off in Task Manager or Settings → Apps → Startup. An
//! entry turned off there counts as off here, and turning it on here clears that flag.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// Value name, also used by the installer.
const NAME: &str = "Reclaim";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Whether Reclaim starts with Windows for the current user.
pub fn enabled() -> bool {
    let (key, name) = (wide(RUN), wide(NAME));
    let mut size = 0u32;
    // SAFETY: NUL-terminated strings; a null buffer with a size pointer only queries the size.
    let run = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    run == ERROR_SUCCESS && !turned_off_by_user()
}

/// The StartupApproved flag: the first byte is even when the entry is on, odd when off.
fn turned_off_by_user() -> bool {
    let (key, name) = (wide(APPROVED), wide(NAME));
    let mut data = [0u8; 12];
    let mut size = data.len() as u32;
    // SAFETY: NUL-terminated strings; `data` holds `size` bytes.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            data.as_mut_ptr().cast::<c_void>(),
            &mut size,
        )
    };
    r == ERROR_SUCCESS && size > 0 && data[0] & 1 == 1
}

fn delete(key: &str) -> Result<(), u32> {
    let (key, name) = (wide(key), wide(NAME));
    // SAFETY: NUL-terminated strings.
    let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
    match r {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        e => Err(e),
    }
}

/// Turns start with Windows on (for the running `reclaim.exe`, in the tray) or off.
/// Returns the Win32 error code on failure.
pub fn set(on: bool) -> Result<(), u32> {
    if !on {
        delete(RUN)?;
        return delete(APPROVED);
    }
    let exe = std::env::current_exe().map_err(|e| e.raw_os_error().unwrap_or(0) as u32)?;
    let command = wide(&format!("\"{}\" --tray", exe.display()));
    let (key, name) = (wide(RUN), wide(NAME));
    let bytes = u32::try_from(command.len() * 2).map_err(|_| 0u32)?;
    // SAFETY: NUL-terminated strings; `command` is `bytes` long, NUL included (REG_SZ).
    let r = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            command.as_ptr().cast::<c_void>(),
            bytes,
        )
    };
    if r != ERROR_SUCCESS {
        return Err(r);
    }
    // An entry the user turned off in Task Manager stays off until this flag is cleared.
    delete(APPROVED)
}
