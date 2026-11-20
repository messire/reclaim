use std::rc::Rc;
use std::time::Duration;

use slint::{ModelRc, SharedString, StandardListViewItem, VecModel};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    EVENT_MODIFY_STATE, INFINITE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
    WaitForSingleObject,
};

slint::slint! {
    #[edition = "1.19"]
    import { Button, StandardTableView, VerticalBox, HorizontalBox, GroupBox } from "std-widgets.slint";

    export component MainWindow inherits Window {
        title: "Reclaim (Slint spike)";
        preferred-width: 720px;
        preferred-height: 560px;

        in property <string> available;
        in property <string> committed;
        in property <string> load;
        in property <[[StandardListViewItem]]> rows;
        in-out property <bool> confirm-open: false;
        callback sort(int, bool);
        callback trim-confirmed();

        VerticalBox {
            GroupBox {
                title: "System memory";
                HorizontalBox {
                    Text { text: "Available: " + root.available; accessible-role: text; }
                    Text { text: "Committed: " + root.committed; }
                    Text { text: "Load: " + root.load; }
                }
            }
            HorizontalBox {
                alignment: start;
                Button {
                    text: "Trim working sets…";
                    clicked => { root.confirm-open = true; }
                }
            }
            StandardTableView {
                columns: [{ title: "Process" }, { title: "PID" }, { title: "Working set" }, { title: "Private bytes" }];
                rows: root.rows;
                sort-ascending(c) => { root.sort(c, true); }
                sort-descending(c) => { root.sort(c, false); }
            }
        }

        if root.confirm-open : Rectangle {
            background: #00000080;
            TouchArea {}
            Rectangle {
                width: 380px;
                height: 160px;
                background: #ffffff;
                border-radius: 6px;
                VerticalBox {
                    Text {
                        text: "Trim working sets of 42 eligible processes?\nThis can cause page faults and does not free committed memory.";
                        wrap: word-wrap;
                    }
                    HorizontalBox {
                        alignment: end;
                        Button { text: "Trim"; primary: true; clicked => { root.confirm-open = false; root.trim-confirmed(); } }
                        Button { text: "Cancel"; clicked => { root.confirm-open = false; } }
                    }
                }
            }
        }
    }

    export component Tray inherits SystemTrayIcon {
        in property <image> tray-icon;
        icon: root.tray-icon;
        tooltip: "Reclaim";
        callback open();
        callback quit();
        clicked => { root.open(); }
        Menu {
            MenuItem { title: "Open"; activated => { root.open(); } }
            MenuItem { title: "Quit"; activated => { root.quit(); } }
        }
    }
}

struct Row {
    name: String,
    pid: u32,
    ws: u64,
    private: u64,
}

fn rows() -> Vec<Row> {
    // Deterministic synthetic data: the spike measures the toolkit, not enumeration.
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    (0..300)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            Row {
                name: format!("process-{i:03}.exe"),
                pid: 1000 + i * 4,
                ws: (x % 2048) << 20,
                private: ((x >> 11) % 2048) << 20,
            }
        })
        .collect()
}

fn mib(b: u64) -> String {
    format!("{:.1} MiB", b as f64 / 1_048_576.0)
}

fn to_model(rows: &[Row]) -> ModelRc<ModelRc<StandardListViewItem>> {
    let v: Vec<ModelRc<StandardListViewItem>> = rows
        .iter()
        .map(|r| {
            ModelRc::new(VecModel::from(vec![
                StandardListViewItem::from(SharedString::from(r.name.as_str())),
                StandardListViewItem::from(SharedString::from(r.pid.to_string())),
                StandardListViewItem::from(SharedString::from(mib(r.ws))),
                StandardListViewItem::from(SharedString::from(mib(r.private))),
            ]))
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

fn system(w: &MainWindow) {
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `ms` is live with dwLength set.
    if unsafe { GlobalMemoryStatusEx(&mut ms) } != 0 {
        w.set_available(mib(ms.ullAvailPhys).into());
        w.set_committed(
            format!(
                "{} / {}",
                mib(ms.ullTotalPageFile - ms.ullAvailPageFile),
                mib(ms.ullTotalPageFile)
            )
            .into(),
        );
        w.set_load(format!("{} %", ms.dwMemoryLoad).into());
    }
}

fn arg(name: &str) -> Option<Vec<u16>> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1)
        .map(|s| s.encode_utf16().chain(std::iter::once(0)).collect())
}

fn signal(name: &[u16]) {
    // SAFETY: NUL-terminated name.
    let h = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if !h.is_null() {
        // SAFETY: valid event handle, closed right after.
        unsafe {
            SetEvent(h);
            CloseHandle(h);
        }
    }
}

type Holder = Rc<std::cell::RefCell<Option<MainWindow>>>;
type Data = Rc<std::cell::RefCell<Vec<Row>>>;

/// Creates the main window. It is dropped again when closed to the tray, so its
/// renderer resources can be released (ARCHITECTURE §8.2), and recreated on Open.
fn build(holder: &Holder, data: &Data) -> MainWindow {
    let window = MainWindow::new().expect("window");
    window.set_rows(to_model(&data.borrow()));
    system(&window);
    {
        let w = window.as_weak();
        let data = data.clone();
        window.on_sort(move |col, asc| {
            let mut d = data.borrow_mut();
            match col {
                0 => d.sort_by(|a, b| a.name.cmp(&b.name)),
                1 => d.sort_by_key(|r| r.pid),
                2 => d.sort_by_key(|r| r.ws),
                _ => d.sort_by_key(|r| r.private),
            }
            if !asc {
                d.reverse();
            }
            if let Some(w) = w.upgrade() {
                w.set_rows(to_model(&d));
            }
        });
    }
    window.on_trim_confirmed(|| {});
    // Close (X) goes to the tray (owner decision Q5).
    let h = holder.clone();
    window.window().on_close_requested(move || {
        drop_later(&h);
        slint::CloseRequestResponse::HideWindow
    });
    window
}

fn drop_later(holder: &Holder) {
    let h = holder.clone();
    slint::Timer::single_shot(Duration::ZERO, move || {
        h.borrow_mut().take();
    });
}

fn open(holder: &Holder, data: &Data) {
    let mut slot = holder.borrow_mut();
    if slot.is_none() {
        *slot = Some(build(holder, data));
    }
    if let Some(w) = slot.as_ref() {
        let _ = w.show();
    }
}

pub fn main() {
    let holder: Holder = Rc::new(std::cell::RefCell::new(None));
    let data: Data = Rc::new(std::cell::RefCell::new(rows()));
    let tray = Tray::new().expect("tray");
    tray.set_tray_icon(tray_icon());
    {
        let (h, d) = (holder.clone(), data.clone());
        tray.on_open(move || open(&h, &d));
    }
    tray.on_quit(|| {
        let _ = slint::quit_event_loop();
    });

    // 1 s system refresh only while a window exists and is visible (ARCHITECTURE §4.1).
    let timer = slint::Timer::default();
    {
        let h = holder.clone();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(1),
            move || {
                if let Some(w) = h.borrow().as_ref()
                    && w.window().is_visible()
                {
                    system(w);
                }
            },
        );
    }

    if let Some(name) = arg("--hide-event") {
        let h = holder.clone();
        std::thread::spawn(move || {
            // SAFETY: NUL-terminated name.
            let ev = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
            if ev.is_null() {
                return;
            }
            // SAFETY: valid event handle with SYNCHRONIZE access.
            if unsafe { WaitForSingleObject(ev, INFINITE) } == WAIT_OBJECT_0 {
                let _ = slint::invoke_from_event_loop(hide_current);
            }
            // SAFETY: valid handle, not used afterwards.
            unsafe { CloseHandle(ev) };
        });
        HOLDER.with(|cell| *cell.borrow_mut() = Some(h));
    }

    open(&holder, &data);
    tray.show().expect("tray show");
    if let Some(name) = arg("--ready-event") {
        // Prefer "after the first rendered frame"; fall back to the first event-loop
        // iteration if the renderer does not support rendering notifiers.
        let once = Rc::new(std::cell::Cell::new(Some(name)));
        let fired = once.clone();
        let notifier = holder.borrow().as_ref().map(|w| {
            w.window().set_rendering_notifier(move |state, _| {
                if matches!(state, slint::RenderingState::AfterRendering)
                    && let Some(n) = fired.take()
                {
                    signal(&n);
                }
            })
        });
        if !matches!(notifier, Some(Ok(()))) {
            eprintln!("ready marker: rendering notifier unavailable, using first event-loop turn");
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(n) = once.take() {
                    signal(&n);
                }
            });
        }
    }
    slint::run_event_loop_until_quit().expect("event loop");
}

/// 32x32 RGBA tray icon generated in code (no image decoder, no asset file).
fn tray_icon() -> slint::Image {
    const N: u32 = 32;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(N, N);
    for (i, px) in buf.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = ((i as u32 % N) as i32 - 16, (i as u32 / N) as i32 - 16);
        let inside = x * x + y * y <= 15 * 15;
        *px = if inside {
            slint::Rgba8Pixel {
                r: 0x1f,
                g: 0x7a,
                b: 0xd6,
                a: 0xff,
            }
        } else {
            slint::Rgba8Pixel {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            }
        };
    }
    slint::Image::from_rgba8(buf)
}

thread_local! {
    static HOLDER: std::cell::RefCell<Option<Holder>> = const { std::cell::RefCell::new(None) };
}

/// Benchmark hook: behaves like the user pressing Close (X).
fn hide_current() {
    HOLDER.with(|cell| {
        if let Some(h) = cell.borrow().as_ref() {
            if let Some(w) = h.borrow().as_ref() {
                let _ = w.hide();
            }
            drop_later(h);
        }
    });
}
