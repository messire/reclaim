//! Analysis of a run record: deltas, rebound and cost indicators (ARCHITECTURE §5.6).

use serde::{Deserialize, Serialize};

use crate::model::{ImageName, MemorySnapshot, ProcessKey};
use crate::outcome::{Completion, FollowUpPoint, ProcessOutcome, RunMode, RunRecord, Totals};
use crate::units::{ByteDelta, Bytes, Count};

/// Working set regained by a process (or all of them) at one follow-up point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rebound {
    /// Follow-up point.
    pub point: FollowUpPoint,
    /// Working set regained since right after the trim.
    pub regained: Bytes,
    /// `regained` as a percentage of the trimmed amount; `None` if nothing was trimmed.
    pub percent: Option<u32>,
    /// True if the sample was taken late.
    pub late: bool,
}

/// Per-process analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessAnalysis {
    /// Identity.
    pub key: ProcessKey,
    /// Image name.
    pub image: ImageName,
    /// Working-set reduction right after the call.
    pub reduced: Bytes,
    /// Private-bytes change across the call (expected ≈ 0).
    pub private_bytes_delta: ByteDelta,
    /// Rebound at each follow-up point where the process was still running.
    pub rebound: Vec<Rebound>,
    /// Page faults between the end of the call and the last follow-up sample.
    pub faults_after: Option<Count>,
    /// True if the process exited during follow-up.
    pub exited_during_follow_up: bool,
}

/// System-level change at one point, relative to T0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SystemChange {
    /// `None` = T1 (immediately after); otherwise a follow-up point.
    pub point: Option<FollowUpPoint>,
    /// Available memory change since T0.
    pub available: ByteDelta,
    /// Committed memory change since T0.
    pub committed: ByteDelta,
    /// True if the sample was late.
    pub late: bool,
}

/// Analysis of a whole run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunAnalysis {
    /// Trim or dry run.
    pub mode: RunMode,
    /// How the run ended.
    pub completion: Completion,
    /// Outcome totals.
    pub totals: Totals,
    /// Trimmed processes.
    pub processes: Vec<ProcessAnalysis>,
    /// Rebound summed over processes still running at each point.
    pub rebound: Vec<Rebound>,
    /// System changes at T1 and each follow-up point.
    pub system: Vec<SystemChange>,
    /// Page faults of trimmed processes after the trim, summed.
    pub faults_after: Count,
    /// Paging input before the run (pages/s), if measured.
    pub paging_input_before: Option<u64>,
    /// Highest paging input during follow-up (pages/s), if measured.
    pub paging_input_peak: Option<u64>,
    /// True if any follow-up sample was late.
    pub any_late: bool,
}

fn percent(regained: Bytes, trimmed: Bytes) -> Option<u32> {
    if trimmed.get() == 0 {
        return None;
    }
    let p = u128::from(regained.get()) * 100 / u128::from(trimmed.get());
    Some(u32::try_from(p).unwrap_or(u32::MAX))
}

fn rebound_of(after: &MemorySnapshot, at: &MemorySnapshot, reduced: Bytes) -> (Bytes, Option<u32>) {
    let regained = at.working_set.saturating_sub(after.working_set);
    (regained, percent(regained, reduced))
}

/// Analyses a run record.
pub fn analyse(record: &RunRecord) -> RunAnalysis {
    let mut processes = Vec::new();
    for r in &record.results {
        let ProcessOutcome::Trimmed { before, after } = &r.outcome else {
            continue;
        };
        let reduced = before.working_set.saturating_sub(after.working_set);
        let mut rebound = Vec::new();
        let mut exited = false;
        let mut last_faults = None;
        for f in &record.follow_ups {
            let sample = f
                .processes
                .iter()
                .find(|(k, _)| *k == r.key)
                .and_then(|(_, m)| *m);
            match sample {
                Some(m) => {
                    let (regained, pct) = rebound_of(after, &m, reduced);
                    rebound.push(Rebound {
                        point: f.point,
                        regained,
                        percent: pct,
                        late: f.is_late(),
                    });
                    last_faults = Some(m.page_faults.saturating_sub(after.page_faults));
                }
                None => exited = true,
            }
        }
        processes.push(ProcessAnalysis {
            key: r.key,
            image: r.image.clone(),
            reduced,
            private_bytes_delta: after.private_bytes.delta_from(before.private_bytes),
            rebound,
            faults_after: last_faults,
            exited_during_follow_up: exited,
        });
    }

    let mut rebound = Vec::new();
    for f in &record.follow_ups {
        let mut regained = Bytes::ZERO;
        let mut trimmed = Bytes::ZERO;
        for p in &processes {
            if let Some(rb) = p.rebound.iter().find(|rb| rb.point == f.point) {
                regained += rb.regained;
                trimmed += p.reduced;
            }
        }
        rebound.push(Rebound {
            point: f.point,
            regained,
            percent: percent(regained, trimmed),
            late: f.is_late(),
        });
    }

    let t0 = &record.system_before;
    let mut system = vec![SystemChange {
        point: None,
        available: record
            .system_after
            .phys_available
            .delta_from(t0.phys_available),
        committed: record
            .system_after
            .commit_charge
            .delta_from(t0.commit_charge),
        late: false,
    }];
    system.extend(record.follow_ups.iter().map(|f| SystemChange {
        point: Some(f.point),
        available: f.system.phys_available.delta_from(t0.phys_available),
        committed: f.system.commit_charge.delta_from(t0.commit_charge),
        late: f.is_late(),
    }));

    let paging_input_peak = record
        .follow_ups
        .iter()
        .filter_map(|f| f.system.pages_input_per_sec)
        .chain(record.system_after.pages_input_per_sec)
        .max();

    RunAnalysis {
        mode: record.mode,
        completion: record.completion,
        totals: record.totals(),
        faults_after: processes.iter().filter_map(|p| p.faults_after).sum(),
        processes,
        rebound,
        system,
        paging_input_before: t0.pages_input_per_sec,
        paging_input_peak,
        any_late: record.follow_ups.iter().any(|f| f.is_late()),
    }
}

impl RunAnalysis {
    /// Rebound at the last follow-up point (normally +60 s).
    pub fn final_rebound(&self) -> Option<&Rebound> {
        self.rebound.last()
    }

    /// True if paging input during follow-up rose materially: at least double the
    /// pre-run rate and at least 100 pages/s more.
    pub fn paging_cost_material(&self) -> bool {
        match (self.paging_input_before, self.paging_input_peak) {
            (Some(b), Some(p)) => p >= b.saturating_mul(2) && p >= b.saturating_add(100),
            (None, Some(p)) => p >= 100,
            _ => false,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::SystemSnapshot;
    use crate::outcome::{FailureKind, FollowUp, ProcessResult};
    use crate::plan::{Plan, Selection};
    use crate::policy::tests::proc_;
    use crate::policy::{Context, Policy};
    use crate::units::Millis;

    pub(crate) fn mem(ws: u64, pb: u64, faults: u64) -> MemorySnapshot {
        MemorySnapshot {
            working_set: Bytes::from_mib(ws),
            private_bytes: Bytes::from_mib(pb),
            page_faults: faults,
            ..Default::default()
        }
    }

    pub(crate) fn sys(avail: u64, commit: u64, pin: Option<u64>) -> SystemSnapshot {
        SystemSnapshot {
            phys_total: Bytes::from_mib(16_000),
            phys_available: Bytes::from_mib(avail),
            commit_charge: Bytes::from_mib(commit),
            commit_limit: Bytes::from_mib(30_000),
            pages_input_per_sec: pin,
            ..Default::default()
        }
    }

    /// Two trimmed processes (a: 400→10, b: 200→0), one failure; follow-ups at +10 s and +60 s.
    pub(crate) fn sample_record(mode: RunMode) -> RunRecord {
        let procs = vec![proc_(10, "a.exe", 400), proc_(11, "b.exe", 200)];
        let ctx = Context {
            self_pid: 1,
            foreground: None,
        };
        let plan = Plan::build(1, &procs, &Policy::default(), &ctx, &Selection::AllEligible);
        let mut r = RunRecord::new(plan, mode, Millis(0), sys(4000, 9000, Some(20)));
        let (ka, kb) = (procs[0].key, procs[1].key);
        r.results.push(ProcessResult {
            key: ka,
            image: ImageName::new("a.exe"),
            outcome: ProcessOutcome::Trimmed {
                before: mem(400, 300, 1000),
                after: mem(10, 300, 1000),
            },
            elapsed: Some(Millis(40)),
        });
        r.results.push(ProcessResult {
            key: kb,
            image: ImageName::new("b.exe"),
            outcome: ProcessOutcome::Trimmed {
                before: mem(200, 150, 50),
                after: mem(0, 150, 50),
            },
            elapsed: Some(Millis(20)),
        });
        r.results.push(ProcessResult {
            key: ProcessKey::new(12, 1),
            image: ImageName::new("c.exe"),
            outcome: ProcessOutcome::Failed(FailureKind::AccessDenied),
            elapsed: None,
        });
        r.system_after = sys(4590, 9000, Some(30));
        r.follow_ups.push(FollowUp {
            point: FollowUpPoint::Plus10s,
            actual_offset: Millis(10_100),
            system: sys(4400, 9010, Some(400)),
            processes: vec![
                (ka, Some(mem(110, 300, 26_600))),
                (kb, Some(mem(20, 150, 5_000))),
            ],
        });
        r.follow_ups.push(FollowUp {
            point: FollowUpPoint::Plus60s,
            actual_offset: Millis(65_000),
            system: sys(4100, 9020, Some(60)),
            processes: vec![(ka, Some(mem(310, 300, 80_000))), (kb, None)],
        });
        r
    }

    #[test]
    fn per_process_deltas_and_rebound() {
        let a = analyse(&sample_record(RunMode::Trim));
        assert_eq!(a.totals.trimmed, 2);
        assert_eq!(a.totals.failed, 1);
        let pa = &a.processes[0];
        assert_eq!(pa.reduced, Bytes::from_mib(390));
        assert_eq!(pa.private_bytes_delta, ByteDelta::ZERO);
        assert_eq!(pa.rebound[0].regained, Bytes::from_mib(100));
        assert_eq!(pa.rebound[0].percent, Some(25));
        assert_eq!(pa.rebound[1].percent, Some(76));
        assert!(pa.rebound[1].late);
        assert_eq!(pa.faults_after, Some(79_000));
        assert!(!pa.exited_during_follow_up);

        let pb = &a.processes[1];
        assert!(pb.exited_during_follow_up);
        assert_eq!(pb.rebound.len(), 1);
        assert_eq!(pb.faults_after, Some(4_950));
        assert_eq!(a.faults_after, 79_000 + 4_950);
    }

    #[test]
    fn aggregate_rebound_ignores_exited_processes() {
        let a = analyse(&sample_record(RunMode::Trim));
        // +10 s: (100 + 20) / (390 + 200) = 20 %.
        assert_eq!(a.rebound[0].regained, Bytes::from_mib(120));
        assert_eq!(a.rebound[0].percent, Some(20));
        // +60 s: only a is still running: 300 / 390 = 76 %.
        let last = a.final_rebound().unwrap();
        assert_eq!(last.point, FollowUpPoint::Plus60s);
        assert_eq!(last.percent, Some(76));
        assert!(a.any_late);
    }

    #[test]
    fn system_changes_and_paging_cost() {
        let a = analyse(&sample_record(RunMode::Trim));
        assert_eq!(a.system[0].point, None);
        assert_eq!(
            a.system[0].available,
            Bytes::from_mib(590).delta_from(Bytes::ZERO)
        );
        assert_eq!(a.system[0].committed, ByteDelta::ZERO);
        assert_eq!(a.system.len(), 3);
        assert_eq!(a.paging_input_before, Some(20));
        assert_eq!(a.paging_input_peak, Some(400));
        assert!(a.paging_cost_material());
    }

    #[test]
    fn paging_cost_thresholds() {
        let mut a = analyse(&sample_record(RunMode::Trim));
        a.paging_input_before = Some(500);
        a.paging_input_peak = Some(900);
        assert!(!a.paging_cost_material(), "below double");
        a.paging_input_before = Some(10);
        a.paging_input_peak = Some(60);
        assert!(!a.paging_cost_material(), "below +100");
        a.paging_input_before = None;
        a.paging_input_peak = Some(100);
        assert!(a.paging_cost_material());
        a.paging_input_peak = None;
        assert!(!a.paging_cost_material());
    }

    #[test]
    fn nothing_trimmed() {
        let mut r = sample_record(RunMode::DryRun);
        r.results.retain(|x| !x.outcome.is_success());
        let a = analyse(&r);
        assert!(a.processes.is_empty());
        assert_eq!(a.rebound[0].percent, None);
        assert_eq!(a.faults_after, 0);
        assert_eq!(percent(Bytes(1), Bytes::ZERO), None);
    }
}
