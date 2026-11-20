use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Memory::{
    CreateMemoryResourceNotification, HighMemoryResourceNotification,
    LowMemoryResourceNotification, QueryMemoryResourceNotification,
};
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::ProcessStatus::{
    GetPerformanceInfo, GetProcessMemoryInfo, PERFORMANCE_INFORMATION, PROCESS_MEMORY_COUNTERS,
    PROCESS_MEMORY_COUNTERS_EX, PROCESS_MEMORY_COUNTERS_EX2,
};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS,
    IsProcessCritical, OpenProcess, OpenProcessToken, PROCESS_ACCESS_RIGHTS,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE,
    SetProcessWorkingSetSize, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const MIB: f64 = 1024.0 * 1024.0;
const PAGE: usize = 4096;

/// Processes whose access results are listed individually in the report.
const INTERESTING: &[&str] = &[
    "System",
    "Registry",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "Memory Compression",
    "MsMpEng.exe",
    "fontdrvhost.exe",
    "dwm.exe",
    "audiodg.exe",
    "explorer.exe",
    "sihost.exe",
    "ctfmon.exe",
    "svchost.exe",
];

pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--probe") {
        let mib: usize = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(256);
        probe(mib);
        return;
    }
    let out = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1).cloned());
    let sizes: Vec<usize> = args
        .iter()
        .position(|a| a == "--sizes")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.split(',').filter_map(|v| v.parse().ok()).collect())
        .unwrap_or_else(|| vec![256, 1024]);

    let mut r = String::new();
    let _ = writeln!(r, "# Windows API spike report\n");
    section_environment(&mut r);
    section_system_metrics(&mut r);
    section_pressure(&mut r);
    section_foreground(&mut r);
    section_access_matrix(&mut r);
    section_enumeration_cost(&mut r);
    section_pdh(&mut r);
    for mib in sizes {
        section_trim(&mut r, mib);
    }

    print!("{r}");
    if let Some(path) = out
        && let Err(e) = std::fs::write(&path, &r)
    {
        eprintln!("cannot write {path}: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Helpers

struct Handle(HANDLE);

impl Handle {
    fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Result<Self, u32> {
        // SAFETY: OpenProcess has no memory-safety preconditions; a null result is an error.
        let h = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            Err(last_error())
        } else {
            Ok(Self(h))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: self.0 is a valid handle owned exclusively by this value.
        unsafe { CloseHandle(self.0) };
    }
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Handle({:p})", self.0)
    }
}

fn last_error() -> u32 {
    // SAFETY: GetLastError only reads thread-local state.
    unsafe { GetLastError() }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn ok_or_err(res: Result<(), u32>) -> String {
    match res {
        Ok(()) => "ok".into(),
        Err(code) => format!("err {code}"),
    }
}

fn mem_ex2(h: HANDLE) -> Result<PROCESS_MEMORY_COUNTERS_EX2, u32> {
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut c: PROCESS_MEMORY_COUNTERS_EX2 = unsafe { std::mem::zeroed() };
    c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
    // SAFETY: the pointer refers to a live struct of `cb` bytes; the API accepts EX2 via cast.
    let ok =
        unsafe { GetProcessMemoryInfo(h, (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(), c.cb) };
    if ok != 0 { Ok(c) } else { Err(last_error()) }
}

fn mem_ex(h: HANDLE) -> Result<PROCESS_MEMORY_COUNTERS_EX, u32> {
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut c: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    // SAFETY: the pointer refers to a live struct of `cb` bytes; the API accepts EX via cast.
    let ok =
        unsafe { GetProcessMemoryInfo(h, (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(), c.cb) };
    if ok != 0 { Ok(c) } else { Err(last_error()) }
}

fn own_private_bytes() -> f64 {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing.
    let me = unsafe { GetCurrentProcess() };
    mem_ex(me)
        .map(|c| c.PrivateUsage as f64 / MIB)
        .unwrap_or(f64::NAN)
}

fn start_time(h: HANDLE) -> Result<u64, u32> {
    let mut t = [FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    }; 4];
    let [a, b, c, d] = &mut t;
    // SAFETY: all four pointers refer to distinct live FILETIME values.
    let ok = unsafe { GetProcessTimes(h, a, b, c, d) };
    if ok != 0 {
        Ok((u64::from(t[0].dwHighDateTime) << 32) | u64::from(t[0].dwLowDateTime))
    } else {
        Err(last_error())
    }
}

fn is_critical(h: HANDLE) -> Result<bool, u32> {
    let mut v = 0;
    // SAFETY: `v` is a live BOOL.
    let ok = unsafe { IsProcessCritical(h, &mut v) };
    if ok != 0 {
        Ok(v != 0)
    } else {
        Err(last_error())
    }
}

fn io_counters(h: HANDLE) -> Result<(), u32> {
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut c: IO_COUNTERS = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a live IO_COUNTERS.
    let ok = unsafe { GetProcessIoCounters(h, &mut c) };
    if ok != 0 { Ok(()) } else { Err(last_error()) }
}

fn token_elevated(h: HANDLE) -> Result<bool, u32> {
    let mut tok: HANDLE = std::ptr::null_mut();
    // SAFETY: `tok` is a live HANDLE slot.
    if unsafe { OpenProcessToken(h, TOKEN_QUERY, &mut tok) } == 0 {
        return Err(last_error());
    }
    let tok = Handle(tok);
    let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    // SAFETY: buffer pointer and size describe the live `e`.
    let ok = unsafe {
        GetTokenInformation(
            tok.0,
            TokenElevation,
            (&raw mut e).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    if ok != 0 {
        Ok(e.TokenIsElevated != 0)
    } else {
        Err(last_error())
    }
}

fn registry_string(value: &str) -> String {
    let key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let name = wide(value);
    let mut buf = [0u16; 256];
    let mut len = size_of_val(&buf) as u32;
    // SAFETY: strings are NUL-terminated; buffer pointer and byte length match.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc == 0 {
        from_wide(&buf)
    } else {
        format!("(err {rc})")
    }
}

fn processes() -> Vec<(u32, String)> {
    let mut out = Vec::new();
    // SAFETY: no preconditions; INVALID_HANDLE_VALUE signals failure.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return out;
    }
    let snap = Handle(snap);
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `e` is live with dwSize set, as required.
    let mut more = unsafe { Process32FirstW(snap.0, &mut e) } != 0;
    while more {
        out.push((e.th32ProcessID, from_wide(&e.szExeFile)));
        // SAFETY: as above.
        more = unsafe { Process32NextW(snap.0, &mut e) } != 0;
    }
    out
}

fn time_avg<F: FnMut()>(n: u32, mut f: F) -> Duration {
    let t = Instant::now();
    for _ in 0..n {
        f();
    }
    t.elapsed() / n
}

// ---------------------------------------------------------------------------
// Sections

fn section_environment(r: &mut String) {
    // SAFETY: pseudo-handle, no closing needed.
    let me = unsafe { GetCurrentProcess() };
    let _ = writeln!(r, "## Environment\n");
    let _ = writeln!(r, "| Item | Value |\n|---|---|");
    let _ = writeln!(r, "| Product | {} |", registry_string("ProductName"));
    let _ = writeln!(
        r,
        "| Display version | {} |",
        registry_string("DisplayVersion")
    );
    let _ = writeln!(
        r,
        "| Build | {}.{} |",
        registry_string("CurrentBuild"),
        ubr()
    );
    let _ = writeln!(r, "| Spike elevated | {:?} |", token_elevated(me));
    let _ = writeln!(
        r,
        "| Spike private bytes at start | {:.1} MiB |",
        own_private_bytes()
    );
    let _ = writeln!(
        r,
        "| Logical CPUs | {} |\n",
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );
    let _ = writeln!(
        r,
        "> Note: Windows 11 reports ProductName \"Windows 10\" in the registry; use the build number (≥ 22000 = Windows 11).\n"
    );
}

fn ubr() -> String {
    // UBR is a DWORD; read it through the same API with a DWORD buffer.
    use windows_sys::Win32::System::Registry::RRF_RT_REG_DWORD;
    let key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let name = wide("UBR");
    let mut v = 0u32;
    let mut len = 4u32;
    // SAFETY: strings are NUL-terminated; `v` is a 4-byte buffer.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut v).cast(),
            &mut len,
        )
    };
    if rc == 0 { v.to_string() } else { "?".into() }
}

fn section_system_metrics(r: &mut String) {
    // SAFETY: zeroed is a valid bit pattern for these plain C structs.
    let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: zeroed is a valid bit pattern for this plain C struct.
    let mut pi: PERFORMANCE_INFORMATION = unsafe { std::mem::zeroed() };
    pi.cb = size_of::<PERFORMANCE_INFORMATION>() as u32;
    let t_ms = time_avg(1000, || {
        // SAFETY: `ms` is live with dwLength set.
        unsafe { GlobalMemoryStatusEx(&mut ms) };
    });
    let t_pi = time_avg(1000, || {
        // SAFETY: `pi` is live with cb set.
        unsafe { GetPerformanceInfo(&mut pi, pi.cb) };
    });
    let page = pi.PageSize as f64;
    let _ = writeln!(r, "## System metrics\n");
    let _ = writeln!(
        r,
        "| Metric | Value | Source | Avg call cost |\n|---|---|---|---|"
    );
    let _ = writeln!(
        r,
        "| Total physical | {:.0} MiB | GlobalMemoryStatusEx | {:?} |",
        ms.ullTotalPhys as f64 / MIB,
        t_ms
    );
    let _ = writeln!(
        r,
        "| Available physical | {:.0} MiB | GlobalMemoryStatusEx | |",
        ms.ullAvailPhys as f64 / MIB
    );
    let _ = writeln!(
        r,
        "| Memory load | {} % | GlobalMemoryStatusEx | |",
        ms.dwMemoryLoad
    );
    let _ = writeln!(
        r,
        "| Commit charge | {:.0} MiB | GetPerformanceInfo | {:?} |",
        pi.CommitTotal as f64 * page / MIB,
        t_pi
    );
    let _ = writeln!(
        r,
        "| Commit limit | {:.0} MiB | GetPerformanceInfo | |",
        pi.CommitLimit as f64 * page / MIB
    );
    let _ = writeln!(
        r,
        "| System cache | {:.0} MiB | GetPerformanceInfo | |",
        pi.SystemCache as f64 * page / MIB
    );
    let _ = writeln!(
        r,
        "| Processes | {} | GetPerformanceInfo | |\n",
        pi.ProcessCount
    );
}

fn section_pressure(r: &mut String) {
    let _ = writeln!(r, "## Memory resource notifications\n");
    let _ = writeln!(
        r,
        "| Handle | Query state | Wait(0) #1 | Wait(0) #2 |\n|---|---|---|---|"
    );
    for (name, kind) in [
        ("Low", LowMemoryResourceNotification),
        ("High", HighMemoryResourceNotification),
    ] {
        // SAFETY: no preconditions; null signals failure.
        let h = unsafe { CreateMemoryResourceNotification(kind) };
        if h.is_null() {
            let _ = writeln!(r, "| {name} | create err {} | | |", last_error());
            continue;
        }
        let h = Handle(h);
        let mut state = 0;
        // SAFETY: valid handle, `state` is a live BOOL.
        let q = unsafe { QueryMemoryResourceNotification(h.0, &mut state) };
        // SAFETY: valid waitable handle; zero timeout.
        let w1 = unsafe { WaitForSingleObject(h.0, 0) };
        // SAFETY: as above.
        let w2 = unsafe { WaitForSingleObject(h.0, 0) };
        let wait = |w| {
            if w == WAIT_OBJECT_0 {
                "signaled"
            } else if w == WAIT_TIMEOUT {
                "timeout"
            } else {
                "other"
            }
        };
        let qs = if q != 0 {
            if state != 0 {
                "signaled"
            } else {
                "not signaled"
            }
        } else {
            "err"
        };
        let _ = writeln!(r, "| {name} | {qs} | {} | {} |", wait(w1), wait(w2));
    }
    let _ = writeln!(
        r,
        "\nIf a handle is signaled on both waits, the notification is level-triggered (REVIEW F27).\n"
    );
}

fn section_foreground(r: &mut String) {
    // SAFETY: no preconditions.
    let hwnd = unsafe { GetForegroundWindow() };
    let mut pid = 0u32;
    if !hwnd.is_null() {
        // SAFETY: hwnd came from the OS; `pid` is a live u32.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    }
    let _ = writeln!(r, "## Foreground window\n");
    if hwnd.is_null() {
        let _ = writeln!(
            r,
            "No foreground window (expected on CI / secure desktop).\n"
        );
    } else {
        let _ = writeln!(r, "Foreground PID: {pid}\n");
    }
}

#[derive(Default)]
struct Tally(BTreeMap<String, u32>);

impl Tally {
    fn add(&mut self, k: String) {
        *self.0.entry(k).or_default() += 1;
    }
    fn render(&self) -> String {
        self.0
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn section_access_matrix(r: &mut String) {
    let procs = processes();
    let mut t_query = Tally::default();
    let mut t_ex2 = Tally::default();
    let mut t_times = Tally::default();
    let mut t_crit = Tally::default();
    let mut t_io = Tally::default();
    let mut t_trim_open = Tally::default();
    let mut t_token = Tally::default();
    let mut rows = Vec::new();

    for (pid, name) in &procs {
        let q = Handle::open(*pid, PROCESS_QUERY_LIMITED_INFORMATION);
        let (ex2, times, crit, io, token) = match &q {
            Ok(h) => (
                ok_or_err(mem_ex2(h.0).map(|_| ())),
                ok_or_err(start_time(h.0).map(|_| ())),
                match is_critical(h.0) {
                    Ok(true) => "critical".to_string(),
                    Ok(false) => "ok".to_string(),
                    Err(e) => format!("err {e}"),
                },
                ok_or_err(io_counters(h.0)),
                match token_elevated(h.0) {
                    Ok(true) => "elevated".to_string(),
                    Ok(false) => "not elevated".to_string(),
                    Err(e) => format!("err {e}"),
                },
            ),
            Err(_) => Default::default(),
        };
        let query = ok_or_err(q.as_ref().map(|_| ()).map_err(|e| *e));
        let trim_open = ok_or_err(
            Handle::open(
                *pid,
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA | PROCESS_SYNCHRONIZE,
            )
            .map(|_| ()),
        );
        t_query.add(query.clone());
        if q.is_ok() {
            t_ex2.add(ex2.clone());
            t_times.add(times.clone());
            t_crit.add(crit.clone());
            t_io.add(io.clone());
            t_token.add(token.clone());
        }
        t_trim_open.add(trim_open.clone());
        let interesting =
            INTERESTING.iter().any(|n| n.eq_ignore_ascii_case(name)) || *pid == 0 || *pid == 4;
        let already = rows.iter().any(
            |(_, n, ..): &(u32, String, String, String, String, String, String, String)| n == name,
        );
        if interesting && !already {
            rows.push((*pid, name.clone(), query, ex2, crit, io, token, trim_open));
        }
    }

    let _ = writeln!(r, "## Process access matrix ({} processes)\n", procs.len());
    let _ = writeln!(r, "Error 5 = access denied, 87 = invalid parameter.\n");
    let _ = writeln!(r, "| Test | Results |\n|---|---|");
    let _ = writeln!(r, "| OpenProcess(QUERY_LIMITED) | {} |", t_query.render());
    let _ = writeln!(r, "| GetProcessMemoryInfo(EX2) | {} |", t_ex2.render());
    let _ = writeln!(r, "| GetProcessTimes | {} |", t_times.render());
    let _ = writeln!(r, "| IsProcessCritical | {} |", t_crit.render());
    let _ = writeln!(
        r,
        "| GetProcessIoCounters (limited handle) | {} |",
        t_io.render()
    );
    let _ = writeln!(
        r,
        "| OpenProcessToken + TokenElevation | {} |",
        t_token.render()
    );
    let _ = writeln!(
        r,
        "| OpenProcess(QUERY_LIMITED + SET_QUOTA + SYNCHRONIZE) | {} |\n",
        t_trim_open.render()
    );
    let _ = writeln!(r, "### Named processes (first instance)\n");
    let _ = writeln!(
        r,
        "| PID | Image | Query | EX2 | Critical | IO | Token | Open for trim |\n|---|---|---|---|---|---|---|---|"
    );
    for (pid, name, q, ex2, crit, io, tok, trim) in rows {
        let _ = writeln!(
            r,
            "| {pid} | {name} | {q} | {ex2} | {crit} | {io} | {tok} | {trim} |"
        );
    }
    let _ = writeln!(r);
}

fn section_enumeration_cost(r: &mut String) {
    let mut times = Vec::new();
    let mut count = 0;
    for _ in 0..7 {
        let t = Instant::now();
        let procs = processes();
        count = procs.len();
        for (pid, _) in &procs {
            if let Ok(h) = Handle::open(*pid, PROCESS_QUERY_LIMITED_INFORMATION) {
                let _ = mem_ex2(h.0);
                let _ = start_time(h.0);
            }
        }
        times.push(t.elapsed());
    }
    times.sort();
    let _ = writeln!(r, "## Enumeration cost\n");
    let _ = writeln!(
        r,
        "Toolhelp snapshot + OpenProcess + GetProcessMemoryInfo + GetProcessTimes for {count} processes: median {:?}, min {:?}, max {:?} (7 runs).\n",
        times[times.len() / 2],
        times[0],
        times[times.len() - 1]
    );
}

fn section_pdh(r: &mut String) {
    const COUNTERS: &[&str] = &[
        r"\Memory\Standby Cache Core Bytes",
        r"\Memory\Standby Cache Normal Priority Bytes",
        r"\Memory\Standby Cache Reserve Bytes",
        r"\Memory\Modified Page List Bytes",
        r"\Memory\Free & Zero Page List Bytes",
        r"\Memory\Cache Bytes",
        r"\Memory\Pages Input/sec",
        r"\Memory\Page Reads/sec",
    ];
    let _ = writeln!(r, "## PDH counters\n");
    let before = own_private_bytes();
    let t0 = Instant::now();
    let mut q: PDH_HQUERY = std::ptr::null_mut();
    // SAFETY: `q` is a live slot; null data source means real-time.
    let rc = unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut q) };
    if rc != 0 {
        let _ = writeln!(r, "PdhOpenQueryW failed: 0x{rc:08x}\n");
        return;
    }
    let mut handles: Vec<(&str, Result<PDH_HCOUNTER, u32>)> = Vec::new();
    for c in COUNTERS {
        let path = wide(c);
        let mut h: PDH_HCOUNTER = std::ptr::null_mut();
        // SAFETY: q is open; path is NUL-terminated; `h` is a live slot.
        let rc = unsafe { PdhAddEnglishCounterW(q, path.as_ptr(), 0, &mut h) };
        handles.push((c, if rc == 0 { Ok(h) } else { Err(rc) }));
    }
    // SAFETY: q is open.
    unsafe { PdhCollectQueryData(q) };
    let init = t0.elapsed();
    let loaded = own_private_bytes();
    std::thread::sleep(Duration::from_secs(1));
    // SAFETY: q is open.
    unsafe { PdhCollectQueryData(q) };

    let _ = writeln!(r, "| Counter | Value |\n|---|---|");
    for (name, h) in &handles {
        let v = match h {
            Err(rc) => format!("add failed 0x{rc:08x}"),
            Ok(h) => {
                let mut ty = 0u32;
                // SAFETY: zeroed is a valid bit pattern for this C struct/union.
                let mut v: PDH_FMT_COUNTERVALUE = unsafe { std::mem::zeroed() };
                // SAFETY: counter handle belongs to the open query; out pointers are live.
                let rc =
                    unsafe { PdhGetFormattedCounterValue(*h, PDH_FMT_DOUBLE, &mut ty, &mut v) };
                if rc == 0 {
                    // SAFETY: PDH_FMT_DOUBLE was requested, so the double member is the active one.
                    let d = unsafe { v.Anonymous.doubleValue };
                    if name.ends_with("/sec") {
                        format!("{d:.1} /s")
                    } else {
                        format!("{:.0} MiB", d / MIB)
                    }
                } else {
                    format!("read failed 0x{rc:08x}")
                }
            }
        };
        let _ = writeln!(r, "| `{name}` | {v} |");
    }
    // SAFETY: q is open and not used afterwards.
    unsafe { PdhCloseQuery(q) };
    let after_close = own_private_bytes();
    let _ = writeln!(
        r,
        "\nInit (open + add + first collect): {init:?}. Spike private bytes: {before:.1} MiB before, {loaded:.1} MiB with PDH loaded, {after_close:.1} MiB after close.\n"
    );
}

fn section_trim(r: &mut String, mib: usize) {
    let _ = writeln!(
        r,
        "## Trim experiment: probe with {mib} MiB dirty private memory\n"
    );
    let exe = std::env::current_exe().expect("current exe");
    let mut child = match Command::new(exe)
        .args(["--probe", &mib.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(r, "spawn failed: {e}\n");
            return;
        }
    };
    let mut stdin = child.stdin.take().expect("stdin");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let mut next = || lines.next().and_then(Result::ok).unwrap_or_default();
    let ready = next();
    if ready != "ready" {
        let _ = writeln!(r, "probe did not start: {ready:?}\n");
        return;
    }

    let h = match Handle::open(
        child.id(),
        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA | PROCESS_SYNCHRONIZE,
    ) {
        Ok(h) => h,
        Err(e) => {
            let _ = writeln!(r, "open failed: {e}\n");
            return;
        }
    };
    let snap = |label: &str, r: &mut String| match mem_ex2(h.0) {
        Ok(c) => {
            let _ = writeln!(
                r,
                "| {label} | {:.1} | {:.1} | {:.1} | {} |",
                c.WorkingSetSize as f64 / MIB,
                c.PrivateWorkingSetSize as f64 / MIB,
                c.PrivateUsage as f64 / MIB,
                c.PageFaultCount
            );
        }
        Err(e) => {
            let _ = writeln!(r, "| {label} | err {e} | | | |");
        }
    };

    let _ = writeln!(
        r,
        "| Point | WS MiB | Private WS MiB | Private bytes MiB | Page faults |\n|---|---|---|---|---|"
    );
    snap("before trim", r);
    let _ = writeln!(stdin, "touch");
    let warm = next();
    snap("after warm re-touch", r);
    let t = Instant::now();
    // SAFETY: valid handle with PROCESS_SET_QUOTA; (usize::MAX, usize::MAX) requests an empty WS.
    let ok = unsafe { SetProcessWorkingSetSize(h.0, usize::MAX, usize::MAX) };
    let trim_time = t.elapsed();
    let trim_err = if ok == 0 { Some(last_error()) } else { None };
    snap("immediately after trim", r);
    std::thread::sleep(Duration::from_secs(1));
    snap("+1 s, untouched", r);
    let _ = writeln!(stdin, "touch");
    let cold = next();
    snap("after re-touch", r);

    // SAFETY: handle has SYNCHRONIZE; zero timeout.
    let alive = unsafe { WaitForSingleObject(h.0, 0) };
    drop(stdin);
    let _ = child.wait();
    // SAFETY: as above.
    let exited = unsafe { WaitForSingleObject(h.0, 0) };
    let mut code = 0u32;
    // SAFETY: valid handle; `code` is live.
    unsafe { GetExitCodeProcess(h.0, &mut code) };
    // SAFETY: valid handle to an exited process; the call must fail or no-op safely.
    let trim_exited = unsafe { SetProcessWorkingSetSize(h.0, usize::MAX, usize::MAX) };
    let trim_exited_err = last_error();
    let mem_exited = mem_ex2(h.0);

    let wait = |w| {
        if w == WAIT_OBJECT_0 {
            "signaled"
        } else if w == WAIT_TIMEOUT {
            "timeout"
        } else {
            "other"
        }
    };
    let _ = writeln!(r);
    let _ = writeln!(
        r,
        "- `SetProcessWorkingSetSize(-1, -1)` took {trim_time:?}, result: {}",
        trim_err.map_or("ok".into(), |e| format!("err {e}"))
    );
    let _ = writeln!(
        r,
        "- Re-touch of all pages while resident: {warm}; after trim: {cold}"
    );
    let _ = writeln!(
        r,
        "- Exit check while alive: {} (expect timeout); after exit: {} (expect signaled), exit code {code}",
        wait(alive),
        wait(exited)
    );
    let _ = writeln!(
        r,
        "- Trim on exited process: {}; memory query on exited process: {}\n",
        if trim_exited != 0 {
            "ok (no-op)".to_string()
        } else {
            format!("err {trim_exited_err}")
        },
        match mem_exited {
            Ok(c) => format!("ok, WS {:.1} MiB", c.WorkingSetSize as f64 / MIB),
            Err(e) => format!("err {e}"),
        }
    );
}

/// Probe child: allocates and dirties `mib` MiB, then serves "touch" commands on stdin.
fn probe(mib: usize) {
    let mut buf = vec![0u8; mib * 1024 * 1024];
    touch(&mut buf, 1);
    println!("ready");
    let _ = std::io::stdout().flush();
    let mut round = 2u8;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim() == "touch" {
            let t = Instant::now();
            touch(&mut buf, round);
            round = round.wrapping_add(1);
            println!("{:?}", t.elapsed());
            let _ = std::io::stdout().flush();
        }
    }
}

fn touch(buf: &mut [u8], v: u8) {
    for i in (0..buf.len()).step_by(PAGE) {
        buf[i] = v;
    }
    std::hint::black_box(&buf);
}
