//! The few Win32 calls the desktop shell needs besides the toolkit: single instance and
//! activation, the local UTC offset, and the benchmark events of `ui-bench`.

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW,
    SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

/// Per-session names: one Reclaim per logged-on user session.
const MUTEX: &str = r"Local\Reclaim.SingleInstance";
const ACTIVATE: &str = r"Local\Reclaim.Activate";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A handle closed on drop.
#[derive(Debug)]
struct Handle(HANDLE);

// SAFETY: kernel handles may be used and closed from any thread.
unsafe impl Send for Handle {}

impl Handle {
    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: owned handle, closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Proof that this is the first instance; held for the life of the process.
#[derive(Debug)]
pub struct Instance {
    _mutex: Option<Handle>,
    activate: Option<Handle>,
}

/// Claims the single-instance mutex. If another instance holds it, asks that instance to
/// show its window and returns `None`.
pub fn claim() -> Option<Instance> {
    let name = wide(MUTEX);
    // SAFETY: NUL-terminated name; null security attributes use the default DACL.
    let m = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    // SAFETY: reads thread-local state set by the call above.
    let err = unsafe { GetLastError() };
    let mutex = (!m.is_null()).then_some(Handle(m));
    // An elevated first instance makes the mutex inaccessible to a non-elevated second
    // one (ERROR_ACCESS_DENIED): that also means "already running".
    if err == ERROR_ALREADY_EXISTS || (mutex.is_none() && err == ERROR_ACCESS_DENIED) {
        drop(mutex);
        activate_other();
        return None;
    }
    // Any other failure: run without the guarantee rather than refuse to start (a second
    // instance is redundant, not harmful).
    let ev = wide(ACTIVATE);
    // SAFETY: NUL-terminated name; auto-reset, initially non-signaled.
    let e = unsafe { CreateEventW(std::ptr::null(), 0, 0, ev.as_ptr()) };
    Some(Instance {
        _mutex: mutex,
        activate: (!e.is_null()).then_some(Handle(e)),
    })
}

fn activate_other() {
    // Let the running instance take the foreground (only the foreground process may grant it).
    // SAFETY: no preconditions.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    signal(ACTIVATE);
}

impl Instance {
    /// Calls `on_activate` (on a helper thread) each time a second instance starts.
    pub fn watch_activation(&mut self, on_activate: impl Fn() + Send + 'static) {
        let Some(h) = self.activate.take() else {
            return;
        };
        let _ = std::thread::Builder::new()
            .name("reclaim-activate".into())
            .spawn(move || {
                // SAFETY: valid event handle (full access from CreateEventW), owned here.
                while unsafe { WaitForSingleObject(h.raw(), INFINITE) } == WAIT_OBJECT_0 {
                    on_activate();
                }
            });
    }
}

/// Minutes to add to UTC for local time, daylight saving included.
pub fn utc_offset_minutes() -> i32 {
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut tz: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `tz` is a live struct.
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = match id {
        2 => tz.Bias + tz.DaylightBias, // TIME_ZONE_ID_DAYLIGHT
        1 => tz.Bias + tz.StandardBias, // TIME_ZONE_ID_STANDARD
        0 => tz.Bias,                   // TIME_ZONE_ID_UNKNOWN: no DST rules
        _ => 0,                         // TIME_ZONE_ID_INVALID
    };
    -bias
}

/// Signals a named event created by `ui-bench` (startup marker).
pub fn signal(name: &str) {
    let w = wide(name);
    // SAFETY: NUL-terminated name.
    let h = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, w.as_ptr()) };
    if !h.is_null() {
        let h = Handle(h);
        // SAFETY: valid event handle with EVENT_MODIFY_STATE.
        unsafe { SetEvent(h.0) };
    }
}

/// Waits (on a helper thread) for a named event created by `ui-bench`, then calls `f`.
pub fn on_event(name: &str, f: impl FnOnce() + Send + 'static) {
    let w = wide(name);
    // SAFETY: NUL-terminated name.
    let h = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, w.as_ptr()) };
    if h.is_null() {
        return;
    }
    let h = Handle(h);
    let _ = std::thread::Builder::new().spawn(move || {
        // SAFETY: valid event handle with SYNCHRONIZE access.
        if unsafe { WaitForSingleObject(h.raw(), INFINITE) } == WAIT_OBJECT_0 {
            f();
        }
    });
}

/// Dark caption for the main window: immersive dark mode (Windows 10 20H1+ and 11), plus
/// caption, border and title colours that match the window background (Windows 11 only;
/// Windows 10 rejects those attributes and keeps its dark default). One-off calls per
/// window, no per-frame cost.
pub fn dark_caption(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DwmSetWindowAttribute,
    };
    // COLORREF is 0x00BBGGRR.
    const fn rgb(r: u32, g: u32, b: u32) -> u32 {
        r | (g << 8) | (b << 16)
    }
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    let dark: i32 = 1;
    let attrs: [(i32, u32); 3] = [
        (DWMWA_CAPTION_COLOR, rgb(0x0b, 0x10, 0x13)),
        (DWMWA_BORDER_COLOR, rgb(0x1b, 0x28, 0x30)),
        (DWMWA_TEXT_COLOR, rgb(0x8a, 0x9d, 0xa5)),
    ];
    // SAFETY: `hwnd` is the live top-level window of this thread; each pointer refers to
    // a local of the size passed, read synchronously by DWM.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark).cast(),
            size_of::<i32>() as u32,
        );
        for (attr, color) in attrs {
            DwmSetWindowAttribute(
                hwnd,
                attr as u32,
                (&raw const color).cast(),
                size_of::<u32>() as u32,
            );
        }
    }
}
