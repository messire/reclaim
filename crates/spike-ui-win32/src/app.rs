use std::cell::RefCell;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, HWND, LPARAM, LRESULT, POINT, RECT, WAIT_OBJECT_0, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{CreateFontIndirectW, HFONT, UpdateWindow};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    EVENT_MODIFY_STATE, INFINITE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Controls::{
    ICC_LISTVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, LVCF_TEXT, LVCF_WIDTH,
    LVCOLUMNW, LVIF_TEXT, LVM_INSERTCOLUMNW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT,
    LVN_COLUMNCLICK, LVN_GETDISPINFOW, LVS_EX_FULLROWSELECT, LVS_OWNERDATA, LVS_REPORT,
    LVS_SHOWSELALWAYS, NMHDR, NMLISTVIEW, NMLVDISPINFOW, WC_LISTVIEWW,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NIN_SELECT,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const WM_HIDE_REQUEST: u32 = WM_APP + 2;
const ID_TRIM: usize = 100;
const ID_OPEN: usize = 200;
const ID_QUIT: usize = 201;
const TIMER_SYSTEM: usize = 1;

struct Row {
    name: Vec<u16>,
    pid: Vec<u16>,
    ws: u64,
    ws_text: Vec<u16>,
    private: u64,
    private_text: Vec<u16>,
    pid_value: u32,
}

#[derive(Default)]
struct State {
    tray_hwnd: HWND,
    main: HWND,
    labels: [HWND; 3],
    button: HWND,
    list: HWND,
    font: HFONT,
    rows: Vec<Row>,
    sort: Option<(i32, bool)>,
    taskbar_created: u32,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn mib(b: u64) -> String {
    format!("{:.1} MiB", b as f64 / 1_048_576.0)
}

fn rows() -> Vec<Row> {
    // Same deterministic synthetic data as the Slint variant.
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    (0..300u32)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let ws = (x % 2048) << 20;
            let private = ((x >> 11) % 2048) << 20;
            let pid = 1000 + i * 4;
            Row {
                name: w(&format!("process-{i:03}.exe")),
                pid: w(&pid.to_string()),
                pid_value: pid,
                ws,
                ws_text: w(&mib(ws)),
                private,
                private_text: w(&mib(private)),
            }
        })
        .collect()
}

fn hinstance() -> windows_sys::Win32::Foundation::HINSTANCE {
    // SAFETY: null asks for the module of the current executable.
    unsafe { GetModuleHandleW(null()) }
}

fn register(class: &[u16], proc_: WNDPROC) {
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: proc_,
        hInstance: hinstance(),
        // SAFETY: loading a predefined system cursor.
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        hbrBackground: (windows_sys::Win32::Graphics::Gdi::COLOR_WINDOW + 1) as usize as _,
        lpszClassName: class.as_ptr(),
        // SAFETY: zeroed is valid for the remaining fields of this C struct.
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: `wc` is fully initialised and the class name outlives the call.
    unsafe { RegisterClassExW(&wc) };
}

fn ui_font() -> HFONT {
    // SAFETY: zeroed is valid for this C struct.
    let mut m: NONCLIENTMETRICSW = unsafe { std::mem::zeroed() };
    m.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
    // SAFETY: pointer and size describe `m`.
    unsafe { SystemParametersInfoW(SPI_GETNONCLIENTMETRICS, m.cbSize, (&raw mut m).cast(), 0) };
    // SAFETY: lfMessageFont is a valid LOGFONTW.
    unsafe { CreateFontIndirectW(&m.lfMessageFont) }
}

fn child(class: *const u16, text: &str, style: u32, parent: HWND, id: usize, font: HFONT) -> HWND {
    let t = w(text);
    // SAFETY: valid class and text pointers; parent is a live window.
    let h = unsafe {
        CreateWindowExW(
            0,
            class,
            t.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            0,
            0,
            parent,
            id as _,
            hinstance(),
            null(),
        )
    };
    // SAFETY: h is a live child window.
    unsafe { SendMessageW(h, WM_SETFONT, font as usize, 1) };
    h
}

fn refresh_system() {
    // SAFETY: zeroed is valid for this C struct.
    let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `ms` is live with dwLength set.
    if unsafe { GlobalMemoryStatusEx(&mut ms) } == 0 {
        return;
    }
    let texts = [
        format!("Available: {}", mib(ms.ullAvailPhys)),
        format!(
            "Committed: {} / {}",
            mib(ms.ullTotalPageFile - ms.ullAvailPageFile),
            mib(ms.ullTotalPageFile)
        ),
        format!("Load: {} %", ms.dwMemoryLoad),
    ];
    STATE.with_borrow(|s| {
        for (h, t) in s.labels.iter().zip(texts) {
            let t = w(&t);
            // SAFETY: live label window, NUL-terminated text.
            unsafe { SetWindowTextW(*h, t.as_ptr()) };
        }
    });
}

fn layout(hwnd: HWND) {
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: live window, `rc` is a live RECT.
    unsafe { GetClientRect(hwnd, &mut rc) };
    // SAFETY: GetDpiForWindow has no preconditions beyond a valid window.
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as i32;
    let px = |v: i32| v * dpi / 96;
    STATE.with_borrow(|s| {
        let width = (rc.right - px(24)) / 3;
        for (i, h) in s.labels.iter().enumerate() {
            // SAFETY: live child windows.
            unsafe { MoveWindow(*h, px(12) + i as i32 * width, px(12), width, px(22), 1) };
        }
        // SAFETY: live child windows.
        unsafe {
            MoveWindow(s.button, px(12), px(44), px(180), px(30), 1);
            MoveWindow(
                s.list,
                px(12),
                px(84),
                rc.right - px(24),
                rc.bottom - px(96),
                1,
            );
        }
    });
}

fn create_main() -> HWND {
    let class = w("ReclaimSpikeMain");
    let title = w("Reclaim (Win32 spike)");
    // SAFETY: class registered in main(); title outlives the call.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            720,
            560,
            null_mut(),
            null_mut(),
            hinstance(),
            null(),
        )
    };
    let font = STATE.with_borrow(|s| s.font);
    let stat = w("STATIC");
    let btn = w("BUTTON");
    let labels = [
        child(stat.as_ptr(), "", 0, hwnd, 0, font),
        child(stat.as_ptr(), "", 0, hwnd, 0, font),
        child(stat.as_ptr(), "", 0, hwnd, 0, font),
    ];
    let button = child(
        btn.as_ptr(),
        "Trim working sets…",
        BS_PUSHBUTTON as u32 | WS_TABSTOP,
        hwnd,
        ID_TRIM,
        font,
    );
    let list = child(
        WC_LISTVIEWW,
        "",
        LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | WS_TABSTOP | WS_BORDER,
        hwnd,
        0,
        font,
    );
    // SAFETY: live list view.
    unsafe {
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            LVS_EX_FULLROWSELECT as usize,
            LVS_EX_FULLROWSELECT as isize,
        )
    };
    for (i, (name, width)) in [
        ("Process", 220),
        ("PID", 80),
        ("Working set", 140),
        ("Private bytes", 140),
    ]
    .iter()
    .enumerate()
    {
        let mut text = w(name);
        let col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            cx: *width,
            pszText: text.as_mut_ptr(),
            // SAFETY: zeroed is valid for the remaining fields.
            ..unsafe { std::mem::zeroed() }
        };
        // SAFETY: live list view; `col` and its text outlive the call.
        unsafe { SendMessageW(list, LVM_INSERTCOLUMNW, i, (&raw const col) as isize) };
    }
    // SAFETY: live list view.
    unsafe { SendMessageW(list, LVM_SETITEMCOUNT, 300, 0) };
    STATE.with_borrow_mut(|s| {
        s.main = hwnd;
        s.labels = labels;
        s.button = button;
        s.list = list;
    });
    layout(hwnd);
    refresh_system();
    // SAFETY: live window; 1 s timer, killed implicitly with the window.
    unsafe { SetTimer(hwnd, TIMER_SYSTEM, 1000, None) };
    hwnd
}

fn show_main() {
    let existing = STATE.with_borrow(|s| s.main);
    let hwnd = if existing.is_null() {
        create_main()
    } else {
        existing
    };
    // SAFETY: live window.
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        SetForegroundWindow(hwnd);
    }
}

fn close_main() {
    let hwnd = STATE.with_borrow(|s| s.main);
    if !hwnd.is_null() {
        // SAFETY: live window owned by this thread.
        unsafe { DestroyWindow(hwnd) };
    }
}

fn sort(col: i32) {
    STATE.with_borrow_mut(|s| {
        let asc = !matches!(s.sort, Some((c, true)) if c == col);
        match col {
            0 => s.rows.sort_by(|a, b| a.name.cmp(&b.name)),
            1 => s.rows.sort_by_key(|r| r.pid_value),
            2 => s.rows.sort_by_key(|r| r.ws),
            _ => s.rows.sort_by_key(|r| r.private),
        }
        if !asc {
            s.rows.reverse();
        }
        s.sort = Some((col, asc));
        // SAFETY: live list view; null rect invalidates everything.
        unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(s.list, null(), 1) };
    });
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            if STATE.with_borrow(|s| s.main == hwnd) {
                layout(hwnd);
            }
            0
        }
        WM_TIMER => {
            // SAFETY: live window.
            if unsafe { IsWindowVisible(hwnd) } != 0 {
                refresh_system();
            }
            0
        }
        WM_COMMAND if (wp & 0xffff) == ID_TRIM => {
            let text = w(
                "Trim working sets of 42 eligible processes?\n\nThis can cause page faults and does not free committed memory.",
            );
            let caption = w("Reclaim");
            // SAFETY: live owner window; NUL-terminated strings.
            unsafe {
                MessageBoxW(
                    hwnd,
                    text.as_ptr(),
                    caption.as_ptr(),
                    MB_OKCANCEL | MB_ICONWARNING,
                )
            };
            0
        }
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY always carries an NMHDR pointer.
            let hdr = unsafe { &*(lp as *const NMHDR) };
            if hdr.code == LVN_GETDISPINFOW {
                // SAFETY: for LVN_GETDISPINFOW the header is part of NMLVDISPINFOW.
                let di = unsafe { &mut *(lp as *mut NMLVDISPINFOW) };
                if di.item.mask & LVIF_TEXT != 0 {
                    STATE.with_borrow(|s| {
                        if let Some(r) = s.rows.get(di.item.iItem as usize) {
                            let t = match di.item.iSubItem {
                                0 => &r.name,
                                1 => &r.pid,
                                2 => &r.ws_text,
                                _ => &r.private_text,
                            };
                            // The row strings live in thread-local state until the next sort, which the
                            // list view never overlaps with a pending display-info request.
                            di.item.pszText = t.as_ptr() as *mut u16;
                        }
                    });
                }
            } else if hdr.code == LVN_COLUMNCLICK {
                // SAFETY: for LVN_COLUMNCLICK the header is part of NMLISTVIEW.
                let nm = unsafe { &*(lp as *const NMLISTVIEW) };
                sort(nm.iSubItem);
            }
            0
        }
        WM_CLOSE => {
            // Close (X) goes to the tray; the window and its resources are released.
            close_main();
            0
        }
        WM_DESTROY => {
            STATE.with_borrow_mut(|s| {
                if s.main == hwnd {
                    s.main = null_mut();
                }
            });
            0
        }
        // SAFETY: default handling with the original arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: zeroed is valid for this C struct.
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

fn add_tray(hwnd: HWND) {
    let mut nid = tray_data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_TRAY;
    // SAFETY: loading a predefined system icon.
    nid.hIcon = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) };
    for (d, s) in nid.szTip.iter_mut().zip("Reclaim".encode_utf16()) {
        *d = s;
    }
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    // SAFETY: `nid` is fully initialised.
    unsafe {
        Shell_NotifyIconW(NIM_ADD, &nid);
        Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

unsafe extern "system" fn tray_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let taskbar_created = STATE.with_borrow(|s| s.taskbar_created);
    if msg == taskbar_created && msg != 0 {
        add_tray(hwnd);
        return 0;
    }
    match msg {
        WM_TRAY => {
            let event = (lp & 0xffff) as u32;
            if event == NIN_SELECT || event == WM_LBUTTONUP {
                show_main();
            } else if event == WM_CONTEXTMENU {
                let mut pt = POINT { x: 0, y: 0 };
                // SAFETY: plain menu creation and display; the menu is destroyed afterwards.
                unsafe {
                    GetCursorPos(&mut pt);
                    let menu = CreatePopupMenu();
                    let open = w("Open");
                    let quit = w("Quit");
                    AppendMenuW(menu, MF_STRING, ID_OPEN, open.as_ptr());
                    AppendMenuW(menu, MF_STRING, ID_QUIT, quit.as_ptr());
                    SetForegroundWindow(hwnd);
                    TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, null());
                    DestroyMenu(menu);
                }
            }
            0
        }
        WM_COMMAND if (wp & 0xffff) == ID_OPEN => {
            show_main();
            0
        }
        WM_COMMAND if (wp & 0xffff) == ID_QUIT => {
            close_main();
            let nid = tray_data(hwnd);
            // SAFETY: removes the icon added in add_tray.
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &nid);
                PostQuitMessage(0);
            }
            0
        }
        WM_HIDE_REQUEST => {
            close_main();
            0
        }
        // SAFETY: default handling with the original arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn arg(name: &str) -> Option<Vec<u16>> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(|s| w(s))
}

pub fn main() {
    // SAFETY: called once before any window is created.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let icc = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_LISTVIEW_CLASSES,
    };
    // SAFETY: `icc` is initialised.
    unsafe { InitCommonControlsEx(&icc) };

    let main_class = w("ReclaimSpikeMain");
    let tray_class = w("ReclaimSpikeTray");
    register(&main_class, Some(main_proc));
    register(&tray_class, Some(tray_proc));
    let tc = w("TaskbarCreated");
    STATE.with_borrow_mut(|s| {
        s.rows = rows();
        s.font = ui_font();
        // SAFETY: NUL-terminated message name.
        s.taskbar_created = unsafe { RegisterWindowMessageW(tc.as_ptr()) };
    });

    // Hidden top-level window that owns the tray icon (it must receive TaskbarCreated broadcasts).
    // SAFETY: class registered above.
    let tray_hwnd = unsafe {
        CreateWindowExW(
            0,
            tray_class.as_ptr(),
            tray_class.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance(),
            null(),
        )
    };
    STATE.with_borrow_mut(|s| s.tray_hwnd = tray_hwnd);
    add_tray(tray_hwnd);

    show_main();
    if let Some(name) = arg("--ready-event") {
        // UpdateWindow painted synchronously, so the window is usable now.
        // SAFETY: NUL-terminated name; the handle is closed right after use.
        unsafe {
            let h = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            if !h.is_null() {
                SetEvent(h);
                CloseHandle(h);
            }
        }
    }
    if let Some(name) = arg("--hide-event") {
        let tray = tray_hwnd as usize;
        std::thread::spawn(move || {
            // SAFETY: NUL-terminated name.
            let h = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
            if h.is_null() {
                return;
            }
            // SAFETY: valid handle; PostMessageW is safe to call from another thread.
            unsafe {
                if WaitForSingleObject(h, INFINITE) == WAIT_OBJECT_0 {
                    PostMessageW(tray as HWND, WM_HIDE_REQUEST, 0, 0);
                }
                CloseHandle(h);
            }
        });
    }

    // SAFETY: zeroed is a valid MSG.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: standard message loop on this thread's windows.
    unsafe {
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            let main = STATE.with_borrow(|s| s.main);
            if main.is_null() || IsDialogMessageW(main, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
