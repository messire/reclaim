//! Known folders (`SHGetKnownFolderPath`, temp path, environment).

use reclaim_storage_core::guard::{Folder, KnownFolders};
use windows_sys::Win32::Foundation::{MAX_PATH, S_OK};
use windows_sys::Win32::Storage::FileSystem::GetTempPathW;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData,
    FOLDERID_LocalAppDataLow, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile,
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, FOLDERID_RoamingAppData,
    FOLDERID_Videos, FOLDERID_Windows, KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath,
};
use windows_sys::core::{GUID, PWSTR};

fn known(id: &GUID) -> Option<String> {
    let mut p: PWSTR = std::ptr::null_mut();
    // SAFETY: `id` is a valid KNOWNFOLDERID; the current user's token (null); `p`
    // receives a CoTaskMemAlloc'ed string. DONT_VERIFY: a folder that does not exist
    // (yet) is still protected by its path.
    let hr = unsafe {
        SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY as u32, std::ptr::null_mut(), &mut p)
    };
    let out = if hr == S_OK && !p.is_null() {
        let mut len = 0usize;
        // SAFETY: on success `p` is a NUL-terminated string; we read up to the NUL.
        while unsafe { *p.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `len` characters before the NUL are initialized.
        let s = unsafe { std::slice::from_raw_parts(p, len) };
        Some(String::from_utf16_lossy(s))
    } else {
        None
    };
    // SAFETY: the string must be freed with CoTaskMemFree whether or not the call
    // succeeded; freeing null is allowed.
    unsafe { CoTaskMemFree(p.cast_const().cast()) };
    out.filter(|s| !s.is_empty())
}

/// `GetTempPath2W(BufferLength, Buffer)`.
type GetTempPath2Fn = unsafe extern "system" fn(u32, *mut u16) -> u32;

/// `GetTempPath2W`, looked up at run time: it only exists on recent Windows builds and
/// a static import would keep the program from loading on older ones.
fn get_temp_path2() -> Option<GetTempPath2Fn> {
    let module = super::wide("kernel32.dll");
    // SAFETY: `module` is NUL-terminated; kernel32 is always loaded in a Win32 process.
    let h = unsafe { GetModuleHandleW(module.as_ptr()) };
    if h.is_null() {
        return None;
    }
    // SAFETY: `h` is a loaded module and the name is a NUL-terminated ANSI string.
    let f = unsafe { GetProcAddress(h, c"GetTempPath2W".as_ptr().cast()) }?;
    // SAFETY: the export named GetTempPath2W has exactly this documented signature.
    Some(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, GetTempPath2Fn>(f) })
}

fn temp_dir() -> Option<String> {
    let mut buf = [0u16; MAX_PATH as usize + 2];
    let mut n = match get_temp_path2() {
        // SAFETY: `buf` is writable for `buf.len()` characters.
        Some(f) => unsafe { f(buf.len() as u32, buf.as_mut_ptr()) },
        None => 0,
    };
    if n == 0 || n as usize >= buf.len() {
        // SAFETY: as above.
        n = unsafe { GetTempPathW(buf.len() as u32, buf.as_mut_ptr()) };
    }
    if n == 0 || n as usize >= buf.len() {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..n as usize]);
    let t = s.trim_end_matches('\\');
    // Keep a drive root's separator ("C:\").
    Some(if t.len() == 2 && t.ends_with(':') {
        format!("{t}\\")
    } else {
        t.to_owned()
    })
}

/// The long form of a path from the environment or `GetTempPath*W`, which may contain
/// 8.3 short components (`C:\Users\RUNNER~1`): the guard compares long final paths.
fn expand(p: String) -> String {
    super::remove::long_name(&p).unwrap_or(p)
}

/// Resolves the known folders the guard and the cleanup rules use.
pub fn known_folders() -> KnownFolders {
    let ids: [(Folder, &GUID); 14] = [
        (Folder::LocalAppData, &FOLDERID_LocalAppData),
        (Folder::RoamingAppData, &FOLDERID_RoamingAppData),
        (Folder::LocalLow, &FOLDERID_LocalAppDataLow),
        (Folder::Profile, &FOLDERID_Profile),
        (Folder::Downloads, &FOLDERID_Downloads),
        (Folder::Documents, &FOLDERID_Documents),
        (Folder::Desktop, &FOLDERID_Desktop),
        (Folder::Pictures, &FOLDERID_Pictures),
        (Folder::Music, &FOLDERID_Music),
        (Folder::Videos, &FOLDERID_Videos),
        (Folder::Windows, &FOLDERID_Windows),
        (Folder::ProgramFiles, &FOLDERID_ProgramFiles),
        (Folder::ProgramFilesX86, &FOLDERID_ProgramFilesX86),
        (Folder::ProgramData, &FOLDERID_ProgramData),
    ];
    let mut entries: Vec<(Folder, String)> = ids
        .iter()
        .filter_map(|(f, id)| known(id).map(|p| (*f, p)))
        .collect();
    if let Some(t) = temp_dir() {
        entries.push((Folder::Temp, expand(t)));
    }
    if let Ok(od) = std::env::var("OneDrive")
        && !od.is_empty()
    {
        entries.push((
            Folder::OneDrive,
            expand(od.trim_end_matches('\\').to_owned()),
        ));
    }
    if let Ok(sd) = std::env::var("SystemDrive")
        && !sd.is_empty()
    {
        entries.push((
            Folder::SystemDrive,
            format!("{}\\", sd.trim_end_matches('\\')),
        ));
    }
    KnownFolders::new(entries)
}
