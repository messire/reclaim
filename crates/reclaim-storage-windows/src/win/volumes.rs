//! `Volumes` port.

use reclaim_storage_core::ports::{DriveKind, VolumeInfo, Volumes};
use windows_sys::Win32::Foundation::MAX_PATH;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW,
    GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery,
    STORAGE_PROPERTY_QUERY, StorageDeviceSeekPenaltyProperty,
};
use windows_sys::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
};

use super::{from_wide, open, wide};
use crate::common::{long_path, strip_long_prefix};

/// Volume enumeration through the documented volume management functions.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinVolumes;

/// `GetDriveTypeW` of a root (`None` for an invalid root).
pub(crate) fn drive_type(root: &str) -> Option<u32> {
    let w = wide(root);
    // SAFETY: `w` is a NUL-terminated root path that outlives the call.
    let t = unsafe { GetDriveTypeW(w.as_ptr()) };
    (t != DRIVE_NO_ROOT_DIR).then_some(t)
}

fn drive_kind(t: u32) -> DriveKind {
    match t {
        DRIVE_FIXED => DriveKind::Fixed,
        DRIVE_REMOVABLE => DriveKind::Removable,
        DRIVE_REMOTE => DriveKind::Network,
        DRIVE_CDROM => DriveKind::Optical,
        DRIVE_RAMDISK => DriveKind::Ram,
        _ => DriveKind::Unknown,
    }
}

/// Label, filesystem name and serial of a mounted root; `None` when the volume is not
/// ready (optical drive without media, disconnected share).
pub(crate) fn volume_information(root: &str) -> Option<(String, String, u32)> {
    let w = wide(root);
    let mut label = [0u16; MAX_PATH as usize + 1];
    let mut fs = [0u16; MAX_PATH as usize + 1];
    let mut serial = 0u32;
    let mut max_component = 0u32;
    let mut flags = 0u32;
    // SAFETY: every buffer is writable with the length passed; `w` is NUL-terminated.
    let ok = unsafe {
        GetVolumeInformationW(
            w.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            &mut serial,
            &mut max_component,
            &mut flags,
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    };
    (ok != 0).then(|| (from_wide(&label), from_wide(&fs), serial))
}

/// The root of the volume that contains `path` (`C:\`, `\\server\share\`, or a mounted
/// folder), with a trailing backslash.
pub(crate) fn volume_root(path: &str) -> Option<String> {
    // The plain form keeps the result in the plain form; only long paths need the prefix.
    let query = if path.encode_utf16().count() < MAX_PATH as usize {
        path.replace('/', "\\")
    } else {
        long_path(path)
    };
    let w = wide(&query);
    let mut buf = vec![0u16; w.len().max(MAX_PATH as usize) + 2];
    // SAFETY: `w` is NUL-terminated; `buf` is writable for `buf.len()` characters.
    let ok = unsafe { GetVolumePathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if ok == 0 {
        return None;
    }
    let root = from_wide(&buf);
    if root.starts_with("\\\\?\\Volume{") {
        return Some(root);
    }
    let mut root = strip_long_prefix(&root);
    if !root.ends_with('\\') {
        root.push('\\');
    }
    Some(root)
}

/// The device path to open for volume-level queries: `\\.\C:` for a drive root,
/// otherwise the volume GUID path without its trailing backslash.
pub(crate) fn volume_device(root: &str) -> Option<String> {
    let b = root.as_bytes();
    if (b.len() == 2 || (b.len() == 3 && b[2] == b'\\'))
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
    {
        return Some(format!("\\\\.\\{}:", b[0] as char));
    }
    let w = wide(root);
    let mut buf = [0u16; 64];
    // SAFETY: `w` is a NUL-terminated mount point; `buf` holds 64 characters (a volume
    // GUID path is 49 plus NUL).
    let ok = unsafe {
        GetVolumeNameForVolumeMountPointW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32)
    };
    (ok != 0).then(|| from_wide(&buf).trim_end_matches('\\').to_owned())
}

/// Whether the disk under `root` incurs a seek penalty (rotational). Opening the device
/// with no access rights is enough for this query and works unelevated.
fn seek_penalty(root: &str) -> Option<bool> {
    let dev = volume_device(root)?;
    let h = open(&dev, 0, FILE_SHARE_READ | FILE_SHARE_WRITE, 0).ok()?;
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceSeekPenaltyProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut desc = DEVICE_SEEK_PENALTY_DESCRIPTOR {
        Version: 0,
        Size: 0,
        IncursSeekPenalty: false,
    };
    let mut returned = 0u32;
    // SAFETY: input and output point to correctly sized, live structs; the call is
    // synchronous (no OVERLAPPED).
    let ok = unsafe {
        DeviceIoControl(
            h.raw(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&raw const query).cast(),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            (&raw mut desc).cast(),
            std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    (ok != 0 && returned as usize >= std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>())
        .then_some(desc.IncursSeekPenalty)
}

fn info(root: &str) -> Option<VolumeInfo> {
    let kind = drive_kind(drive_type(root)?);
    let (label, fs, _) = volume_information(root)?;
    let w = wide(root);
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: `w` is NUL-terminated; the out pointers are live locals.
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, &mut total_free) };
    if ok == 0 {
        return None;
    }
    let seek_penalty = match kind {
        DriveKind::Network | DriveKind::Optical | DriveKind::Ram => None,
        _ => seek_penalty(root),
    };
    Some(VolumeInfo {
        root: root.to_owned(),
        label,
        fs,
        kind,
        total,
        free,
        seek_penalty,
    })
}

impl Volumes for WinVolumes {
    fn list(&self) -> Vec<VolumeInfo> {
        // SAFETY: a zero-length query only returns the required size.
        let need = unsafe { GetLogicalDriveStringsW(0, std::ptr::null_mut()) };
        if need == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; need as usize + 1];
        // SAFETY: `buf` is writable for `buf.len()` characters.
        let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) };
        if n == 0 || n as usize > buf.len() {
            return Vec::new();
        }
        buf[..n as usize]
            .split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .filter_map(|root| info(&root))
            .collect()
    }

    fn volume_of(&self, path: &str) -> Option<VolumeInfo> {
        info(&volume_root(path)?)
    }
}
