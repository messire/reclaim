//! Drives the engine's run coordinator for benchmark arms and the soak (B5, B8, §6).

use std::time::{Duration, Instant};

use reclaim_core::analysis::{RunAnalysis, analyse};
use reclaim_core::outcome::{RunMode, RunRecord};
use reclaim_core::plan::Selection;
use reclaim_core::ports::Clock;
use reclaim_core::units::Millis;
use reclaim_engine::{Coordinator, Platform, Step};

/// A clock whose monotonic time runs `factor` times faster than `inner`, so the +10 s and
/// +60 s follow-ups arrive after 1/factor of the time (B8's shortened follow-up).
#[derive(Debug)]
pub struct FastClock<C> {
    /// Real clock.
    pub inner: C,
    /// Speed-up.
    pub factor: u64,
}

impl<C: Clock> Clock for FastClock<C> {
    fn monotonic(&self) -> Millis {
        Millis(self.inner.monotonic().0.saturating_mul(self.factor))
    }

    fn wall(&self) -> Millis {
        self.inner.wall()
    }
}

/// One measured arm.
#[derive(Debug, Clone)]
pub struct Arm {
    /// Control (dry run) or trim.
    pub mode: RunMode,
    /// The record.
    pub record: RunRecord,
    /// Its analysis.
    pub analysis: RunAnalysis,
    /// Probe pass times (µs) in the 10 s before the run.
    pub latency_before: Vec<f64>,
    /// Probe pass times (µs) in the 10 s after trimming completed.
    pub latency_after: Vec<f64>,
}

/// Plans and runs one arm to its final record. `wait` sleeps until the coordinator's next
/// follow-up (real sleep in the harness, a fake clock advance in tests).
pub fn run_arm<P: Platform>(
    c: &mut Coordinator<P>,
    mode: RunMode,
    selection: &Selection,
    follow_up: bool,
    wait: impl FnMut(Duration),
) -> Result<RunRecord, String> {
    run_arm_timed(c, mode, selection, follow_up, wait).map(|(r, _)| r)
}

/// As [`run_arm`], also returning the real instant trimming completed (the coordinator's
/// clock may be accelerated, so its elapsed time cannot anchor real-time windows).
pub fn run_arm_timed<P: Platform>(
    c: &mut Coordinator<P>,
    mode: RunMode,
    selection: &Selection,
    follow_up: bool,
    mut wait: impl FnMut(Duration),
) -> Result<(RunRecord, Instant), String> {
    let plan = c.plan(selection).map_err(|e| e.to_string())?;
    if plan.targets.is_empty() {
        c.discard();
        return Err("no eligible target".into());
    }
    c.start(plan.id, mode).map_err(|e| e.to_string())?;
    loop {
        match c.step() {
            Step::Progress { .. } => {}
            Step::Completed(_) => break,
            Step::Idle => return Err("run did not start".into()),
        }
    }
    let completed = Instant::now();
    if !follow_up {
        return Ok((c.finalize(true), completed));
    }
    loop {
        let (_, done) = c.tick();
        if let Some(r) = done {
            return Ok((r, completed));
        }
        let ms = c.next_follow_up().map_or(0, |d| d.since(c.now()).0);
        wait(Duration::from_millis(ms.max(1)));
    }
}

/// Resource counters of the harness process at a soak checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Resources {
    /// Kernel handles.
    pub handles: u64,
    /// GDI objects.
    pub gdi: u64,
    /// USER objects.
    pub user: u64,
    /// Private bytes.
    pub private_bytes: u64,
}

/// Soak checkpoints: 10, 100, 1 000, … up to `cycles` (always including the last).
pub fn checkpoints(cycles: u64) -> Vec<u64> {
    let mut v: Vec<u64> = std::iter::successors(Some(10u64), |c| c.checked_mul(10))
        .take_while(|c| *c < cycles)
        .collect();
    v.push(cycles);
    v
}

/// Runs `cycles` plan → trim → finalize cycles (B8) and measures at each checkpoint.
/// Returns the checkpoints with their counters, and the number of cycles that failed.
pub fn soak<P: Platform>(
    c: &mut Coordinator<P>,
    cycles: u64,
    selection: &Selection,
    mut wait: impl FnMut(Duration),
    mut measure: impl FnMut() -> Resources,
) -> (Vec<(u64, Resources)>, u64) {
    let marks = checkpoints(cycles);
    let mut out = vec![(0, measure())];
    let mut failed = 0;
    for i in 1..=cycles {
        if run_arm(c, RunMode::Trim, selection, true, &mut wait).is_err() {
            failed += 1;
        }
        if marks.contains(&i) {
            out.push((i, measure()));
        }
    }
    (out, failed)
}

/// Builds an [`Arm`] from a record and the probe pass times observed around it.
pub fn arm(mode: RunMode, record: RunRecord, before: Vec<f64>, after: Vec<f64>) -> Arm {
    Arm {
        mode,
        analysis: analyse(&record),
        record,
        latency_before: before,
        latency_after: after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reclaim_core::policy::Policy;
    use reclaim_platform_fake::{FakePlatform, process};

    fn coordinator(f: &FakePlatform) -> Coordinator<FakePlatform> {
        Coordinator::new(f.clone(), Box::new(f.clone()), Policy::default())
    }

    #[test]
    fn arm_with_follow_up() {
        let f = FakePlatform::new();
        f.add(process(10, "a.exe", 300).ws_after_trim_mib(100));
        let mut c = coordinator(&f);
        let g = f.clone();
        let r = run_arm(&mut c, RunMode::Trim, &Selection::AllEligible, true, |d| {
            g.advance(d.as_millis() as u64)
        })
        .unwrap();
        assert_eq!(r.follow_ups.len(), 2);
        assert_eq!(r.totals().trimmed, 1);
        let a = arm(RunMode::Trim, r, vec![1.0], vec![2.0]);
        assert_eq!(a.analysis.totals.trimmed, 1);

        let r = run_arm(
            &mut c,
            RunMode::DryRun,
            &Selection::AllEligible,
            false,
            |_| unreachable!(),
        )
        .unwrap();
        assert!(r.follow_ups.is_empty());
    }

    #[test]
    fn no_target_is_an_error() {
        let f = FakePlatform::new();
        f.add(process(10, "tiny.exe", 1));
        let mut c = coordinator(&f);
        assert!(run_arm(&mut c, RunMode::Trim, &Selection::AllEligible, true, |_| {}).is_err());
    }

    #[test]
    fn soak_measures_checkpoints() {
        assert_eq!(checkpoints(1000), [10, 100, 1000]);
        assert_eq!(checkpoints(250), [10, 100, 250]);
        assert_eq!(checkpoints(5), [5]);
        let f = FakePlatform::new();
        f.add(process(10, "a.exe", 300).ws_after_trim_mib(100));
        let mut c = coordinator(&f);
        let g = f.clone();
        let mut n = 0;
        let (marks, failed) = soak(
            &mut c,
            100,
            &Selection::AllEligible,
            |d| g.advance(d.as_millis() as u64),
            || {
                n += 1;
                Resources {
                    handles: n,
                    ..Resources::default()
                }
            },
        );
        assert_eq!(failed, 0);
        assert_eq!(marks.iter().map(|m| m.0).collect::<Vec<_>>(), [0, 10, 100]);
    }

    #[test]
    fn fast_clock() {
        let f = FakePlatform::new();
        let c = FastClock {
            inner: f.clone(),
            factor: 60,
        };
        f.advance(1000);
        assert_eq!(c.monotonic(), Millis(60_000));
        assert_eq!(c.wall(), f.wall());
    }
}
