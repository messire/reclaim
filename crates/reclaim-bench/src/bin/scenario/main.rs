//! Effectiveness scenarios S1–S8 with control and trim arms (BENCHMARKS §6).
//!
//! ```text
//! reclaim-scenario --scenario S2 [--arms control,trim] [--settle-secs 120] [--quick]
//!                  [--probes-only] [--size 2048] [--allow-pressure] [--out bench-results]
//!                  [--label name]
//! ```
//! - `--quick` runs the follow-up 60× faster (+60 s after one second): for CI smoke runs,
//!   not for published numbers.
//! - `--probes-only` trims only the harness's own probes (safe on a shared machine).
//! - S5 allocates until less than 10 % of memory is available and needs `--allow-pressure`.
//!
//! Writes `<out>/<scenario>-<label>-<unix time>/` with one JSON record per arm and
//! `summary.md`. Exit code 3 on unsafe behaviour (latency regressions are reported in the
//! summary, not as a failure: they are a result).

#[cfg(windows)]
fn main() {
    std::process::exit(win::main());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("reclaim-scenario only runs on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
mod win {
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use reclaim_bench::run::{Arm, FastClock, arm, run_arm_timed};
    use reclaim_bench::spawn::Probe;
    use reclaim_bench::summary::{faults, scenario_md};
    use reclaim_core::model::ProcessKey;
    use reclaim_core::outcome::RunMode;
    use reclaim_core::plan::Selection;
    use reclaim_core::ports::{Clock, ProcessSource, SystemMetrics};
    use reclaim_engine::{Coordinator, StdClock, Store};
    use reclaim_platform_windows::WindowsPlatform;

    const QUICK_FACTOR: u64 = 60;

    fn arg(args: &[String], name: &str) -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        args.get(i + 1).cloned()
    }

    fn flag(args: &[String], name: &str) -> bool {
        args.iter().any(|a| a == name)
    }

    /// xorshift64 seeded from the clock: arm order and race delays.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    /// A scenario's workload probes, created afresh for every arm so that no arm inherits
    /// the working sets an earlier trim arm reduced (or the pressure it relieved).
    fn workload(scenario: &str, size: u64, allow_pressure: bool) -> Result<Vec<Probe>, String> {
        let mut probes = Vec::new();
        match scenario {
            "S3" => probes.push(Probe::start(&[
                "alloc-touch",
                "--size",
                &size.to_string(),
                "--dirty",
            ])?),
            "S5" => {
                if !allow_pressure {
                    return Err("S5 drives the machine into high memory pressure; pass --allow-pressure to confirm".into());
                }
                let mut p = WindowsPlatform::new();
                loop {
                    let s = p.sample().map_err(|e| e.to_string())?;
                    if s.phys_available.get() * 10 < s.phys_total.get() || probes.len() >= 64 {
                        break;
                    }
                    probes.push(Probe::start(&["alloc-touch", "--size", "1024", "--dirty"])?);
                }
            }
            _ => {}
        }
        Ok(probes)
    }

    /// S7: probes that exit 10–500 ms from now, while the arm plans and trims.
    fn racers(rng: &mut Rng) -> Result<Vec<Probe>, String> {
        (0..24)
            .map(|_| {
                let ms = 10 + rng.next() % 491;
                Probe::start(&["exit-after", "--ms", &ms.to_string(), "--size", "48"])
            })
            .collect()
    }

    fn keys(p: &mut WindowsPlatform, pids: &[u32]) -> Vec<ProcessKey> {
        p.enumerate()
            .map(|l| {
                l.into_iter()
                    .filter(|i| pids.contains(&i.key.pid))
                    .map(|i| i.key)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn main() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let Some(scenario) = arg(&args, "--scenario").map(|s| s.to_uppercase()) else {
            eprintln!("usage: reclaim-scenario --scenario S1..S8 [options]; see the source header");
            return 2;
        };
        let quick = flag(&args, "--quick");
        let probes_only = flag(&args, "--probes-only");
        let settle = Duration::from_secs(
            arg(&args, "--settle-secs")
                .and_then(|v| v.parse().ok())
                .unwrap_or(if quick { 0 } else { 120 }),
        );
        let size: u64 = arg(&args, "--size")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2048);
        let label = arg(&args, "--label")
            .or_else(|| std::env::var("COMPUTERNAME").ok())
            .unwrap_or_else(|| "local".into());
        let mut modes: Vec<RunMode> = arg(&args, "--arms")
            .unwrap_or_else(|| "control,trim".into())
            .split(',')
            .filter_map(|a| match a.trim() {
                "control" | "dry-run" => Some(RunMode::DryRun),
                "trim" => Some(RunMode::Trim),
                _ => None,
            })
            .collect();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut rng = Rng(now | 1);
        // Randomized arm order (BENCHMARKS §6): Fisher–Yates.
        for i in (1..modes.len()).rev() {
            modes.swap(i, (rng.next() % (i as u64 + 1)) as usize);
        }

        let allow_pressure = flag(&args, "--allow-pressure");
        if scenario == "S5" && !allow_pressure {
            eprintln!(
                "reclaim-scenario: S5 drives the machine into high memory pressure; pass --allow-pressure to confirm"
            );
            return 1;
        }
        // The latency probe re-touches its memory every 100 ms, so it recovers by itself
        // between arms and can keep one baseline across them.
        let latency = match Probe::start(&["latency", "--size", "256", "--interval-ms", "100"]) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("reclaim-scenario: {e}");
                return 1;
            }
        };
        let policy = Store::new(Store::default_dir()).load_settings().0.policy();
        let mut arms: Vec<Arm> = Vec::new();
        let out_dir = PathBuf::from(arg(&args, "--out").unwrap_or_else(|| "bench-results".into()))
            .join(format!("{scenario}-{label}-{now}"));
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            eprintln!("reclaim-scenario: cannot create {}: {e}", out_dir.display());
            return 1;
        }

        for (i, &mode) in modes.iter().enumerate() {
            let workload = match workload(&scenario, size, allow_pressure) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("reclaim-scenario: {e}");
                    return 1;
                }
            };
            if i > 0 || !quick {
                eprintln!("settling for {} s…", settle.as_secs());
                std::thread::sleep(settle);
            }
            let race = if scenario == "S7" {
                match racers(&mut rng) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("reclaim-scenario: {e}");
                        return 1;
                    }
                }
            } else {
                Vec::new()
            };
            let mut platform = WindowsPlatform::new();
            platform.set_details(true);
            let selection = if probes_only {
                let mut pids: Vec<u32> = workload.iter().chain(&race).map(Probe::pid).collect();
                pids.push(latency.pid());
                Selection::Only(keys(&mut platform, &pids))
            } else {
                Selection::AllEligible
            };
            let clock: Box<dyn Clock + Send> = if quick {
                Box::new(FastClock {
                    inner: StdClock::default(),
                    factor: QUICK_FACTOR,
                })
            } else {
                Box::new(StdClock::default())
            };
            let factor = if quick { QUICK_FACTOR } else { 1 };
            let mut c = Coordinator::new(platform, clock, policy.clone());
            let start = Instant::now();
            eprintln!("arm {} ({mode:?})…", i + 1);
            let (record, completed) = match run_arm_timed(&mut c, mode, &selection, true, |d| {
                std::thread::sleep(d / factor as u32)
            }) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("reclaim-scenario: arm {}: {e}", i + 1);
                    return 1;
                }
            };
            if quick {
                // Leave the post-trim latency window time to fill.
                std::thread::sleep(
                    (completed + Duration::from_secs(10)).saturating_duration_since(Instant::now()),
                );
            }
            let before = latency.passes_between(
                start.checked_sub(Duration::from_secs(10)).unwrap_or(start),
                start,
            );
            let after = latency.passes_between(completed, completed + Duration::from_secs(10));
            let json = record.to_json();
            let file = out_dir.join(format!(
                "arm-{}-{}.json",
                i + 1,
                if mode == RunMode::Trim {
                    "trim"
                } else {
                    "control"
                }
            ));
            if let Err(e) = std::fs::write(&file, json) {
                eprintln!("reclaim-scenario: cannot write {}: {e}", file.display());
            }
            drop(race);
            drop(workload);
            arms.push(arm(mode, record, before, after));
        }
        drop(latency);

        let commit = std::env::var("GITHUB_SHA").unwrap_or_else(|_| "local build".into());
        let md = scenario_md(
            &format!("Scenario {scenario}"),
            &[
                ("machine", label.clone()),
                ("commit", commit),
                ("reclaim", reclaim_core::VERSION.to_owned()),
                (
                    "follow-up",
                    if quick {
                        format!("shortened {QUICK_FACTOR}× (smoke run, not for publication)")
                    } else {
                        "real time (+10 s, +60 s)".into()
                    },
                ),
                (
                    "targets",
                    if probes_only {
                        "harness probes only".into()
                    } else {
                        "all eligible processes (default policy)".into()
                    },
                ),
                ("arm order", format!("{modes:?}")),
                ("settle between arms", format!("{} s", settle.as_secs())),
            ],
            &arms,
        );
        let path = out_dir.join("summary.md");
        if let Err(e) = std::fs::write(&path, &md) {
            eprintln!("reclaim-scenario: cannot write {}: {e}", path.display());
        }
        println!("{md}");
        eprintln!("results in {}", out_dir.display());
        if faults(&arms).is_empty() { 0 } else { 3 }
    }
}
