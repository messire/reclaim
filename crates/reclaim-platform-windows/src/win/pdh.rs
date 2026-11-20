//! On-demand PDH reader for the standby / modified breakdown and the paging-input rate.
//!
//! `pdh.dll` is loaded at run time (restricted to System32) and unloaded on drop, so it is
//! never on the startup or idle path (spike A7: ≈ 250 ms and ≈ 5 MiB to open a query).

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
};

use reclaim_core::units::Bytes;

type RawProc = unsafe extern "system" fn() -> isize;
type OpenQuery = unsafe extern "system" fn(*const u16, usize, *mut PDH_HQUERY) -> u32;
type AddCounter =
    unsafe extern "system" fn(PDH_HQUERY, *const u16, usize, *mut PDH_HCOUNTER) -> u32;
type QueryFn = unsafe extern "system" fn(PDH_HQUERY) -> u32;
type GetValue =
    unsafe extern "system" fn(PDH_HCOUNTER, u32, *mut u32, *mut PDH_FMT_COUNTERVALUE) -> u32;

const STANDBY: [&str; 3] = [
    r"\Memory\Standby Cache Core Bytes",
    r"\Memory\Standby Cache Normal Priority Bytes",
    r"\Memory\Standby Cache Reserve Bytes",
];
const MODIFIED: &str = r"\Memory\Modified Page List Bytes";
const PAGES_INPUT: &str = r"\Memory\Pages Input/sec";

/// Values read in one collection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PdhValues {
    /// Sum of the standby lists.
    pub standby: Option<Bytes>,
    /// Modified page list.
    pub modified: Option<Bytes>,
    /// Pages read from disk per second.
    pub pages_input_per_sec: Option<u64>,
}

/// An open PDH query. Dropping it closes the query and unloads `pdh.dll`.
#[derive(Debug)]
pub struct PdhReader {
    module: HMODULE,
    query: PDH_HQUERY,
    collect: QueryFn,
    close: QueryFn,
    get: GetValue,
    standby: Vec<PDH_HCOUNTER>,
    modified: Option<PDH_HCOUNTER>,
    pages_input: Option<PDH_HCOUNTER>,
}

// SAFETY: PDH query and counter handles may be used from any thread as long as calls are
// not concurrent; the reader is owned by one thread at a time (`&mut self` for reads).
unsafe impl Send for PdhReader {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl PdhReader {
    /// Loads PDH and opens the query. Returns `None` if PDH is unavailable.
    pub fn open() -> Option<Self> {
        let dll = wide("pdh.dll");
        // SAFETY: NUL-terminated module name; search restricted to System32.
        let module = unsafe {
            LoadLibraryExW(
                dll.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if module.is_null() {
            return None;
        }
        let sym = |name: &[u8]| {
            // SAFETY: module is loaded; name is NUL-terminated ASCII.
            unsafe { GetProcAddress(module, name.as_ptr()) }
        };
        let (Some(o), Some(a), Some(c), Some(g), Some(cl)) = (
            sym(b"PdhOpenQueryW\0"),
            sym(b"PdhAddEnglishCounterW\0"),
            sym(b"PdhCollectQueryData\0"),
            sym(b"PdhGetFormattedCounterValue\0"),
            sym(b"PdhCloseQuery\0"),
        ) else {
            // SAFETY: module was loaded above and no export from it is used.
            unsafe { FreeLibrary(module) };
            return None;
        };
        // SAFETY: each target type matches the documented signature of that export (pdh.h).
        let (open, add, collect, get, close) = unsafe {
            (
                std::mem::transmute::<RawProc, OpenQuery>(o),
                std::mem::transmute::<RawProc, AddCounter>(a),
                std::mem::transmute::<RawProc, QueryFn>(c),
                std::mem::transmute::<RawProc, GetValue>(g),
                std::mem::transmute::<RawProc, QueryFn>(cl),
            )
        };
        let mut query: PDH_HQUERY = std::ptr::null_mut();
        // SAFETY: `query` is a live slot; a null data source means real-time data.
        if unsafe { open(std::ptr::null(), 0, &mut query) } != 0 {
            // SAFETY: as above.
            unsafe { FreeLibrary(module) };
            return None;
        }
        let add_one = |path: &str| {
            let p = wide(path);
            let mut h: PDH_HCOUNTER = std::ptr::null_mut();
            // SAFETY: the query is open; path is NUL-terminated; `h` is a live slot.
            (unsafe { add(query, p.as_ptr(), 0, &mut h) } == 0).then_some(h)
        };
        let reader = Self {
            module,
            query,
            collect,
            close,
            get,
            standby: STANDBY.iter().filter_map(|p| add_one(p)).collect(),
            modified: add_one(MODIFIED),
            pages_input: add_one(PAGES_INPUT),
        };
        // First collection primes rate counters.
        // SAFETY: the query is open.
        unsafe { (reader.collect)(reader.query) };
        Some(reader)
    }

    fn value(&self, h: PDH_HCOUNTER) -> Option<f64> {
        let mut ty = 0u32;
        // SAFETY: zeroed is a valid bit pattern for this C struct/union.
        let mut v: PDH_FMT_COUNTERVALUE = unsafe { std::mem::zeroed() };
        // SAFETY: the counter belongs to the open query; out pointers are live.
        let rc = unsafe { (self.get)(h, PDH_FMT_DOUBLE, &mut ty, &mut v) };
        if rc != 0 {
            return None;
        }
        // SAFETY: PDH_FMT_DOUBLE was requested, so the double member is the active one.
        Some(unsafe { v.Anonymous.doubleValue })
    }

    /// Collects and reads all counters. Rates need two collections at least one second apart;
    /// the first read after `open` may report 0 pages/s.
    pub fn read(&mut self) -> PdhValues {
        // SAFETY: the query is open.
        unsafe { (self.collect)(self.query) };
        let standby = if self.standby.is_empty() {
            None
        } else {
            let mut sum = 0u64;
            for h in &self.standby {
                sum = sum.saturating_add(self.value(*h).unwrap_or(0.0) as u64);
            }
            Some(Bytes(sum))
        };
        PdhValues {
            standby,
            modified: self
                .modified
                .and_then(|h| self.value(h))
                .map(|v| Bytes(v as u64)),
            pages_input_per_sec: self
                .pages_input
                .and_then(|h| self.value(h))
                .map(|v| v as u64),
        }
    }
}

impl Drop for PdhReader {
    fn drop(&mut self) {
        // SAFETY: the query is open and not used afterwards; then no PDH function pointer
        // is used after FreeLibrary.
        unsafe {
            (self.close)(self.query);
            FreeLibrary(self.module);
        }
    }
}
