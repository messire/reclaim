use std::fmt::Write as _;
use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetProcessTimes, SetEvent, WaitForSingleObject,
};

const MIB: f64 = 1024.0 * 1024.0;

struct Event(HANDLE, String);

impl Event {
    fn new(name: String) -> Self {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: NUL-terminated name; manual-reset, initially non-signaled.
        let h = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) };
        assert!(!h.is_null(), "CreateEventW failed");
        Self(h, name)
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: owned, valid handle.
        unsafe { CloseHandle(self.0) };
    }
}

fn private_bytes(child: &Child) -> f64 {
    // SAFETY: zeroed is valid for this C struct.
    let mut c: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let h = child.as_raw_handle() as HANDLE;
    // SAFETY: the std child handle has full access; pointer and size describe `c`.
    let ok =
        unsafe { GetProcessMemoryInfo(h, (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(), c.cb) };
    if ok != 0 {
        c.PrivateUsage as f64 / MIB
    } else {
        f64::NAN
    }
}

fn cpu_time(child: &Child) -> Duration {
    let z = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut a, mut b, mut k, mut u) = (z, z, z, z);
    let h = child.as_raw_handle() as HANDLE;
    // SAFETY: valid process handle; four distinct live FILETIMEs.
    unsafe { GetProcessTimes(h, &mut a, &mut b, &mut k, &mut u) };
    let t = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Duration::from_nanos((t(k) + t(u)) * 100)
}

struct Idle {
    cpu_pct: f64,
    mean: f64,
    max: f64,
}

fn idle(child: &Child, secs: u64) -> Idle {
    let c0 = cpu_time(child);
    let t0 = Instant::now();
    let mut samples = Vec::new();
    for _ in 0..secs {
        std::thread::sleep(Duration::from_secs(1));
        samples.push(private_bytes(child));
    }
    let cpu = cpu_time(child) - c0;
    Idle {
        cpu_pct: cpu.as_secs_f64() / t0.elapsed().as_secs_f64() * 100.0,
        mean: samples.iter().sum::<f64>() / samples.len().max(1) as f64,
        max: samples.iter().cloned().fold(f64::NAN, f64::max),
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exe = arg(&args, "--exe").expect("--exe <path> is required");
    let label = arg(&args, "--label").unwrap_or_else(|| exe.clone());
    let runs: usize = arg(&args, "--runs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let idle_secs: u64 = arg(&args, "--idle-secs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let me = std::process::id();

    let mut starts = Vec::new();
    let mut ready_private = Vec::new();
    let mut last: Option<(Child, Event)> = None;
    for run in 0..runs {
        let ready = Event::new(format!("Local\\reclaim-ui-ready-{me}-{run}"));
        let hide = Event::new(format!("Local\\reclaim-ui-hide-{me}-{run}"));
        let t0 = Instant::now();
        let mut child = Command::new(&exe)
            .args(["--ready-event", &ready.1, "--hide-event", &hide.1])
            .spawn()
            .expect("spawn spike");
        // SAFETY: valid event handle; 15 s timeout.
        let w = unsafe { WaitForSingleObject(ready.0, 15_000) };
        if w != WAIT_OBJECT_0 {
            let _ = child.kill();
            eprintln!("run {run}: no ready signal within 15 s");
            continue;
        }
        starts.push(t0.elapsed());
        ready_private.push(private_bytes(&child));
        if run + 1 == runs {
            last = Some((child, hide));
        } else {
            let _ = child.kill();
            let _ = child.wait();
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    let mut r = String::new();
    let _ = writeln!(r, "## UI spike: {label}\n");
    let size = std::fs::metadata(&exe)
        .map(|m| m.len() as f64 / MIB)
        .unwrap_or(f64::NAN);
    let _ = writeln!(r, "| Metric | Value |\n|---|---|");
    let _ = writeln!(r, "| Executable size | {size:.2} MiB |");
    if !starts.is_empty() {
        let mut s = starts.clone();
        s.sort();
        let p95 = s[((s.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)];
        let _ = writeln!(
            r,
            "| Warm start to usable UI (N={}) | median {:?}, p95 {p95:?}, min {:?} |",
            s.len(),
            s[s.len() / 2],
            s[0]
        );
        let mean = ready_private.iter().sum::<f64>() / ready_private.len() as f64;
        let _ = writeln!(r, "| Private bytes at ready | {mean:.1} MiB |");
        let each: Vec<String> = starts
            .iter()
            .map(|d| format!("{:.0}", d.as_secs_f64() * 1000.0))
            .collect();
        let _ = writeln!(r, "| Each start, in order (ms) | {} |", each.join(", "));
    }
    if let Some((mut child, _)) = last.take_if(|_| idle_secs == 0) {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Some((mut child, hide)) = last {
        std::thread::sleep(Duration::from_secs(10));
        let v = idle(&child, idle_secs);
        let _ = writeln!(
            r,
            "| Idle, window visible ({idle_secs} s) | CPU {:.3} % of one core; private bytes mean {:.1} MiB, max {:.1} MiB |",
            v.cpu_pct, v.mean, v.max
        );
        // SAFETY: valid event handle.
        unsafe { SetEvent(hide.0) };
        std::thread::sleep(Duration::from_secs(10));
        let t = idle(&child, idle_secs);
        let _ = writeln!(
            r,
            "| Idle, closed to tray ({idle_secs} s) | CPU {:.3} % of one core; private bytes mean {:.1} MiB, max {:.1} MiB |",
            t.cpu_pct, t.mean, t.max
        );
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = writeln!(r);
    print!("{r}");
    if let Some(path) = arg(&args, "--out")
        && let Err(e) = std::fs::write(&path, &r)
    {
        eprintln!("cannot write {path}: {e}");
        std::process::exit(1);
    }
}
