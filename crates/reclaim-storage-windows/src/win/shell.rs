//! Shell integration: Explorer, the properties dialog, elevation and file dialogs.

use std::os::windows::process::CommandExt;

use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, RECT};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows_sys::Win32::UI::Shell::{
    BFFM_INITIALIZED, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, GPFIDL_DEFAULT,
    SEE_MASK_INVOKEIDLIST, SHBrowseForFolderW, SHELLEXECUTEINFOW, SHGetPathFromIDListEx,
    ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, HWND_TOP, IsWindow, SW_SHOW, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow,
    SetWindowPos,
};

use super::{Handle, from_wide, last_error, wide};

/// Characters a path buffer of the dialogs can hold (long paths included).
const PATH_BUF: usize = 32_768;

/// COM for the calling thread for the duration of a shell call. A thread already in
/// another apartment (`RPC_E_CHANGED_MODE`) keeps it; only a successful initialization
/// is balanced with `CoUninitialize`.
#[derive(Debug)]
struct Com(bool);

impl Com {
    fn init() -> Self {
        // SAFETY: reserved must be null; the flags are documented values.
        let hr = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        Com(hr >= 0)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx of `init`; the guard is a
            // local of the function that created it, so this runs on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// The owner for a dialog: the given window if it is a live window, otherwise the
/// active window of the calling thread, so a dialog is never left without an owner
/// (an unowned dialog can open behind the app window and look like a hang).
fn hwnd(owner: isize) -> HWND {
    let h = owner as HWND;
    // SAFETY: IsWindow accepts any value; GetActiveWindow has no preconditions.
    unsafe {
        if !h.is_null() && IsWindow(h) != 0 {
            h
        } else {
            GetActiveWindow()
        }
    }
}

/// `SHBrowseForFolderW` callback: once the dialog exists, centre it over its owner
/// (passed in `data`) and bring it to the front, so it never opens behind the app.
unsafe extern "system" fn browse_callback(
    dlg: HWND,
    msg: u32,
    _param: LPARAM,
    data: LPARAM,
) -> i32 {
    if msg == BFFM_INITIALIZED {
        let owner = data as HWND;
        let mut o = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut d = o;
        // SAFETY: `dlg` is the live dialog the shell passed in; `owner` is either null
        // (GetWindowRect then fails and the dialog keeps its position) or a window
        // checked by `hwnd`. The RECTs are writable locals.
        unsafe {
            if !owner.is_null()
                && GetWindowRect(owner, &mut o) != 0
                && GetWindowRect(dlg, &mut d) != 0
            {
                let x = o.left + ((o.right - o.left) - (d.right - d.left)) / 2;
                let y = o.top + ((o.bottom - o.top) - (d.bottom - d.top)) / 2;
                SetWindowPos(dlg, HWND_TOP, x.max(o.left), y.max(o.top), 0, 0, SWP_NOSIZE);
            } else {
                SetWindowPos(dlg, HWND_TOP, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE);
            }
            SetForegroundWindow(dlg);
        }
    }
    0
}

/// Opens Explorer with `path` selected.
pub fn open_in_explorer(path: &str) -> Result<(), String> {
    if path.contains('"') {
        return Err("invalid path".into());
    }
    // The quotes must reach Explorer verbatim; std's argument quoting would escape them.
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(drop)
        .map_err(|e| format!("could not start Explorer: {e}"))
}

/// Shows the shell's properties dialog of a file or folder.
pub fn show_properties(path: &str) -> Result<(), String> {
    let _com = Com::init();
    let verb = wide("properties");
    let file = wide(path);
    // SAFETY: SHELLEXECUTEINFOW is integers and pointers; all-zero is valid (null
    // pointers, no flags).
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_INVOKEIDLIST;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.nShow = SW_SHOW;
    // SAFETY: `info` is initialized with cbSize; the strings outlive the call.
    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok != 0 {
        Ok(())
    } else {
        Err(format!(
            "could not show properties (error {})",
            last_error()
        ))
    }
}

/// Whether the process runs elevated (administrator token).
pub fn is_elevated() -> bool {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo handle of the current process needs no closing; `token`
    // receives a new handle on success.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        return false;
    }
    let token = Handle::from_raw(token);
    let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    // SAFETY: `token` was opened with TOKEN_QUERY; `e` is the struct of this class.
    let ok = unsafe {
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            (&raw mut e).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    ok != 0 && e.TokenIsElevated != 0
}

/// Lets the user pick a folder (`owner`: an HWND or 0). `None` when cancelled.
pub fn pick_folder(owner: isize, title: &str) -> Option<String> {
    let _com = Com::init();
    let title = wide(title);
    let mut display = vec![0u16; 260];
    let owner = hwnd(owner);
    let bi = BROWSEINFOW {
        hwndOwner: owner,
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: display.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        lpfn: Some(browse_callback),
        lParam: owner as LPARAM,
        iImage: 0,
    };
    // SAFETY: `bi` is fully initialized; the display buffer holds MAX_PATH characters
    // as documented; the title outlives the modal call.
    let pidl = unsafe { SHBrowseForFolderW(&bi) };
    if pidl.is_null() {
        return None;
    }
    let mut buf = vec![0u16; PATH_BUF];
    // SAFETY: `pidl` is the item list the dialog returned; `buf` is writable for its
    // length.
    let ok =
        unsafe { SHGetPathFromIDListEx(pidl, buf.as_mut_ptr(), buf.len() as u32, GPFIDL_DEFAULT) };
    // SAFETY: the dialog's PIDL is owned by the caller and freed with CoTaskMemFree.
    unsafe { CoTaskMemFree(pidl.cast_const().cast()) };
    let s = from_wide(&buf);
    (ok != 0 && !s.is_empty()).then_some(s)
}

/// The standard "Save as" dialog. `filter_desc`/`ext` describe one file type
/// (`"CSV files"`, `"csv"`). `None` when cancelled.
pub fn save_file_dialog(
    owner: isize,
    default_name: &str,
    filter_desc: &str,
    ext: &str,
) -> Option<String> {
    let ext = ext.trim_start_matches('.');
    let mut filter: Vec<u16> = filter_desc.encode_utf16().collect();
    filter.push(0);
    filter.extend(format!("*.{ext}").encode_utf16());
    filter.extend([0, 0]);
    let def_ext = wide(ext);
    let mut file = vec![0u16; PATH_BUF];
    for (d, s) in file
        .iter_mut()
        .zip(default_name.encode_utf16().take(PATH_BUF - 1))
    {
        *d = s;
    }
    // SAFETY: OPENFILENAMEW is integers and pointers; all-zero is valid.
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = hwnd(owner);
    ofn.lpstrFilter = filter.as_ptr();
    ofn.nFilterIndex = 1;
    ofn.lpstrFile = file.as_mut_ptr();
    ofn.nMaxFile = file.len() as u32;
    ofn.lpstrDefExt = def_ext.as_ptr();
    ofn.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    // SAFETY: `ofn` is initialized with its size; every buffer it points to outlives
    // the modal call and `nMaxFile` is the file buffer's length.
    let ok = unsafe { GetSaveFileNameW(&mut ofn) };
    let s = from_wide(&file);
    (ok != 0 && !s.is_empty()).then_some(s)
}
