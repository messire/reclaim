//! Soak test B8: repeated plan → trim → finalize cycles over the harness's own probes,
//! with the follow-up shortened 60×, checking that handle, GDI, USER and private-byte
//! counts of the engine's process do not trend upward.
//!
//! ```text
//! reclaim-soak [--cycles 1000] [--probes 3] [--size 64] [--out soak.md]
//! ```
//! Exit code 0 on pass, 3 on an upward trend.

#[cfg(windows)]
fn main() {
    std::process::exit(win::main());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("reclaim-soak only runs on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
mod win {
    use reclaim_bench::run::{FastClock, Resources, soak};
    use reclaim_bench::spawn::Probe;
    use reclaim_bench::summary::soak_md;
    use reclaim_core::plan::Selection;
    use reclaim_core::policy::Policy;
    use reclaim_core::ports::ProcessSource;
    use reclaim_core::units::Bytes;
    use reclaim_engine::{Coordinator, StdClock};
    use reclaim_platform_windows::{WindowsPlatform, own_private_bytes};
    use windows_sys::Win32::System::Threading::{
        GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources, GetProcessHandleCount,
    };

    const FACTOR: u64 = 60;

    fn arg(args: &[String], name: &str, default: u64) -> u64 {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    fn measure() -> Resources {
        let mut handles = 0u32;
        // SAFETY: the pseudo-handle needs no closing; `handles` is a live u32.
        unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut handles) };
        // SAFETY: pseudo-handle of this process; no other preconditions.
        let gdi = unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
        // SAFETY: as above.
        let user = unsafe { GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS) };
        Resources {
            handles: u64::from(handles),
            gdi: u64::from(gdi),
            user: u64::from(user),
            private_bytes: own_private_bytes().map_or(0, Bytes::get),
        }
    }

    pub fn main() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let cycles = arg(&args, "--cycles", 1000);
        let size = arg(&args, "--size", 64).to_string();
        let probes: Result<Vec<Probe>, String> = (0..arg(&args, "--probes", 3))
            .map(|_| Probe::start(&["alloc-touch", "--size", &size]))
            .collect();
        let probes = match probes {
            Ok(p) => p,
            Err(e) => {
                eprintln!("reclaim-soak: {e}");
                return 1;
            }
        };
        let mut platform = WindowsPlatform::new();
        let pids: Vec<u32> = probes.iter().map(Probe::pid).collect();
        let keys = platform
            .enumerate()
            .map(|l| {
                l.into_iter()
                    .filter(|i| pids.contains(&i.key.pid))
                    .map(|i| i.key)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Trimmed probes stay eligible across cycles: no size threshold.
        let policy = Policy {
            min_working_set: Bytes::ZERO,
            ..Policy::default()
        };
        let clock = FastClock {
            inner: StdClock::default(),
            factor: FACTOR,
        };
        let mut c = Coordinator::new(platform, Box::new(clock), policy);
        eprintln!("soak: {cycles} cycles over {} probes…", keys.len());
        let (marks, failed) = soak(
            &mut c,
            cycles,
            &Selection::Only(keys),
            |d| std::thread::sleep(d / FACTOR as u32),
            measure,
        );
        drop(probes);
        let (md, pass) = soak_md(&marks, failed);
        println!("{md}");
        if let Some(i) = args.iter().position(|a| a == "--out")
            && let Some(p) = args.get(i + 1)
            && let Err(e) = std::fs::write(p, &md)
        {
            eprintln!("reclaim-soak: cannot write {p}: {e}");
        }
        if pass && failed == 0 { 0 } else { 3 }
    }
}
