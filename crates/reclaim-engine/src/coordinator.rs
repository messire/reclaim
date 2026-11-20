//! Run coordinator: the run state machine (ARCHITECTURE §7), driven step by step so that
//! it can be tested deterministically with the fake platform and clock.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};

use reclaim_core::model::{ProcessInfo, SystemSnapshot};
use reclaim_core::outcome::{
    Completion, FailureKind, FollowUp, FollowUpPoint, ProcessOutcome, ProcessResult, RunMode,
    RunRecord,
};
use reclaim_core::plan::{Plan, Selection, Target};
use reclaim_core::policy::{Context, Eligibility, Policy, evaluate};
use reclaim_core::ports::{
    Clock, PlatformError, ProcessHandle, ProcessSource, ReclaimAction, Reclaimer, SessionInfo,
    SystemMetrics,
};
use reclaim_core::units::Millis;

/// Everything the engine needs from a platform adapter.
pub trait Platform: SystemMetrics + ProcessSource + Reclaimer + SessionInfo + Send {}
impl<T: SystemMetrics + ProcessSource + Reclaimer + SessionInfo + Send> Platform for T {}

/// The follow-up points, in order.
pub const FOLLOW_UPS: [FollowUpPoint; 2] = [FollowUpPoint::Plus10s, FollowUpPoint::Plus60s];

/// Why a command was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    /// No plan with this id is waiting.
    UnknownPlan,
    /// A run is executing.
    Busy,
    /// The platform failed.
    Platform(PlatformError),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::UnknownPlan => f.write_str("the plan is no longer current"),
            CommandError::Busy => f.write_str("a run is in progress"),
            CommandError::Platform(e) => e.fmt(f),
        }
    }
}

/// Observable state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing happening.
    Idle,
    /// A plan waits for confirmation.
    Planned,
    /// Targets are being processed.
    Running,
    /// Waiting for follow-up samples.
    FollowUp,
}

/// Result of one [`Coordinator::step`].
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Nothing to do.
    Idle,
    /// One target processed.
    Progress {
        /// Targets done so far.
        done: usize,
        /// Targets in the run.
        total: usize,
        /// The result just recorded.
        result: ProcessResult,
    },
    /// All targets processed (or cancelled); follow-up starts.
    Completed(Box<RunRecord>),
}

struct Run {
    record: RunRecord,
    queue: VecDeque<Target>,
    total: usize,
    held: Vec<Box<dyn ProcessHandle>>,
    started: Millis,
    ended: Option<Millis>,
    next_point: usize,
    cancel: bool,
}

/// A table row: a process and what policy says about it.
pub type Row = (ProcessInfo, Eligibility);

/// The run state machine over a platform.
pub struct Coordinator<P: Platform> {
    platform: P,
    clock: Box<dyn Clock + Send>,
    /// Current policy.
    pub policy: Policy,
    next_plan_id: u64,
    plan: Option<Plan>,
    run: Option<Run>,
}

impl<P: Platform> std::fmt::Debug for Coordinator<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coordinator")
            .field("phase", &self.phase())
            .finish_non_exhaustive()
    }
}

impl<P: Platform> Coordinator<P> {
    /// Creates a coordinator.
    pub fn new(platform: P, clock: Box<dyn Clock + Send>, policy: Policy) -> Self {
        Self {
            platform,
            clock,
            policy,
            next_plan_id: 1,
            plan: None,
            run: None,
        }
    }

    /// The platform (for sampling and capabilities).
    pub fn platform(&mut self) -> &mut P {
        &mut self.platform
    }

    /// Monotonic now.
    pub fn now(&self) -> Millis {
        self.clock.monotonic()
    }

    /// Current phase.
    pub fn phase(&self) -> Phase {
        match (&self.run, &self.plan) {
            (Some(r), _) if r.ended.is_none() => Phase::Running,
            (Some(_), _) => Phase::FollowUp,
            (None, Some(_)) => Phase::Planned,
            (None, None) => Phase::Idle,
        }
    }

    fn context(&self) -> Context {
        Context {
            self_pid: self.platform.self_pid(),
            foreground: self.platform.foreground_process(),
        }
    }

    /// Enumerates processes with their eligibility under the current policy.
    pub fn table(&mut self) -> Result<Vec<Row>, PlatformError> {
        let ctx = self.context();
        let list = self.platform.enumerate()?;
        Ok(list
            .into_iter()
            .map(|p| {
                let e = evaluate(&p, &self.policy, &ctx);
                (p, e)
            })
            .collect())
    }

    /// Builds a plan; replaces any waiting plan.
    pub fn plan(&mut self, selection: &Selection) -> Result<Plan, CommandError> {
        if self.phase() == Phase::Running {
            return Err(CommandError::Busy);
        }
        let ctx = self.context();
        let list = self.platform.enumerate().map_err(CommandError::Platform)?;
        let plan = Plan::build(self.next_plan_id, &list, &self.policy, &ctx, selection);
        self.next_plan_id += 1;
        self.plan = Some(plan.clone());
        Ok(plan)
    }

    /// Discards a waiting plan.
    pub fn discard(&mut self) {
        self.plan = None;
    }

    /// Starts the waiting plan. A run still in follow-up is finalized early (truncated) and
    /// returned so it can be stored.
    pub fn start(
        &mut self,
        plan_id: u64,
        mode: RunMode,
    ) -> Result<Option<RunRecord>, CommandError> {
        if self.phase() == Phase::Running {
            return Err(CommandError::Busy);
        }
        match &self.plan {
            Some(p) if p.id == plan_id => {}
            _ => return Err(CommandError::UnknownPlan),
        }
        let previous = self.run.is_some().then(|| self.finalize(true));
        let plan = self.plan.take().ok_or(CommandError::UnknownPlan)?;
        self.platform.set_details(true);
        let before = self
            .platform
            .sample_detailed()
            .map_err(CommandError::Platform)?;
        let queue: VecDeque<Target> = plan.targets.iter().cloned().collect();
        let total = queue.len();
        let record = RunRecord::new(plan, mode, self.clock.wall(), before);
        self.run = Some(Run {
            record,
            queue,
            total,
            held: Vec::new(),
            started: self.clock.monotonic(),
            ended: None,
            next_point: 0,
            cancel: false,
        });
        Ok(previous)
    }

    /// Requests cancellation. During the run, remaining targets become `Cancelled`; during
    /// follow-up, the run is finalized with a truncated follow-up and returned.
    pub fn cancel(&mut self) -> Option<RunRecord> {
        match self.phase() {
            Phase::Running => {
                if let Some(r) = self.run.as_mut() {
                    r.cancel = true;
                }
                None
            }
            Phase::FollowUp => Some(self.finalize(true)),
            Phase::Planned => {
                self.plan = None;
                None
            }
            Phase::Idle => None,
        }
    }

    /// Processes one target, or completes the run when none are left.
    pub fn step(&mut self) -> Step {
        let Some(run) = self.run.as_mut() else {
            return Step::Idle;
        };
        if run.ended.is_some() {
            return Step::Idle;
        }
        if !run.cancel
            && let Some(target) = run.queue.pop_front()
        {
            let mode = run.record.mode;
            let platform = &mut self.platform;
            let clock = &self.clock;
            let (result, handle) = match catch_unwind(AssertUnwindSafe(|| {
                act(platform, clock.as_ref(), &target, mode)
            })) {
                Ok(r) => r,
                Err(panic) => {
                    let msg = panic
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_owned())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "panic".to_owned());
                    (
                        ProcessResult {
                            key: target.key,
                            image: target.image.clone(),
                            outcome: ProcessOutcome::Failed(FailureKind::Internal(msg)),
                            elapsed: None,
                        },
                        None,
                    )
                }
            };
            run.held.extend(handle);
            run.record.results.push(result.clone());
            let done = run.total - run.queue.len();
            return Step::Progress {
                done,
                total: run.total,
                result,
            };
        }
        // Finished or cancelled.
        let cancelled = run.cancel && !run.queue.is_empty();
        for t in run.queue.drain(..) {
            run.record.results.push(ProcessResult {
                key: t.key,
                image: t.image,
                outcome: ProcessOutcome::Cancelled,
                elapsed: None,
            });
        }
        if cancelled {
            run.record.completion = Completion::Cancelled;
        }
        let now = self.clock.monotonic();
        run.record.elapsed = now.since(run.started);
        run.record.system_after = self
            .platform
            .sample_detailed()
            .unwrap_or(run.record.system_before);
        run.ended = Some(now);
        Step::Completed(Box::new(run.record.clone()))
    }

    /// When the next follow-up sample is due (monotonic), if any.
    pub fn next_follow_up(&self) -> Option<Millis> {
        let run = self.run.as_ref()?;
        let ended = run.ended?;
        FOLLOW_UPS
            .get(run.next_point)
            .map(|p| ended.plus(p.offset().0))
    }

    /// Takes due follow-up samples. Returns the samples taken and, after the last one, the
    /// finalized record.
    pub fn tick(&mut self) -> (Vec<FollowUpPoint>, Option<RunRecord>) {
        let mut taken = Vec::new();
        loop {
            let Some(due) = self.next_follow_up() else {
                return (taken, None);
            };
            let now = self.clock.monotonic();
            if now < due {
                return (taken, None);
            }
            let system = self.platform.sample_detailed().unwrap_or_default();
            let Some(run) = self.run.as_mut() else {
                return (taken, None);
            };
            let ended = run.ended.unwrap_or(now);
            let point = FOLLOW_UPS[run.next_point];
            let processes = run
                .held
                .iter()
                .map(|h| {
                    let m = if h.has_exited() {
                        None
                    } else {
                        h.memory().ok()
                    };
                    (h.key(), m)
                })
                .collect();
            run.record.follow_ups.push(FollowUp {
                point,
                actual_offset: now.since(ended),
                system,
                processes,
            });
            run.next_point += 1;
            taken.push(point);
            if run.next_point >= FOLLOW_UPS.len() {
                return (taken, Some(self.finalize(false)));
            }
        }
    }

    /// Ends the current run, releasing held handles and the optional counters.
    pub fn finalize(&mut self, truncated: bool) -> RunRecord {
        let Some(run) = self.run.take() else {
            return RunRecord::new(
                Plan {
                    id: 0,
                    lists_version: 0,
                    targets: vec![],
                    skipped: vec![],
                },
                RunMode::DryRun,
                Millis(0),
                SystemSnapshot::default(),
            );
        };
        let mut record = run.record;
        if truncated
            && run.next_point < FOLLOW_UPS.len()
            && record.completion == Completion::Complete
        {
            record.completion = Completion::FollowUpTruncated;
        }
        drop(run.held);
        self.platform.set_details(false);
        record
    }
}

fn act<P: Platform>(
    platform: &mut P,
    clock: &dyn Clock,
    target: &Target,
    mode: RunMode,
) -> (ProcessResult, Option<Box<dyn ProcessHandle>>) {
    let result = |outcome, elapsed| ProcessResult {
        key: target.key,
        image: target.image.clone(),
        outcome,
        elapsed,
    };
    let h = match platform.open(target.key) {
        Ok(h) => h,
        Err(e) => return (result(ProcessOutcome::Failed(e.into()), None), None),
    };
    let before = match h.memory() {
        Ok(m) => m,
        Err(_) if h.has_exited() => {
            return (
                result(ProcessOutcome::Failed(FailureKind::Exited), None),
                None,
            );
        }
        Err(e) => {
            return (
                result(
                    ProcessOutcome::Failed(FailureKind::Os(e.code.unwrap_or(-1))),
                    None,
                ),
                None,
            );
        }
    };
    let t0 = clock.monotonic();
    if mode == RunMode::Trim
        && let Err(e) = platform.reclaim(h.as_ref(), ReclaimAction::TrimWorkingSet)
    {
        return (result(ProcessOutcome::Failed(e.into()), None), None);
    }
    let elapsed = clock.monotonic().since(t0);
    let after = h.memory().unwrap_or(before);
    (
        result(ProcessOutcome::Trimmed { before, after }, Some(elapsed)),
        Some(h),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use reclaim_core::analysis::analyse;
    use reclaim_core::model::ImageLocation;
    use reclaim_core::units::Bytes;
    use reclaim_platform_fake::{Behaviour, FakePlatform, process};

    fn setup() -> (FakePlatform, Coordinator<FakePlatform>) {
        let f = FakePlatform::new();
        let c = Coordinator::new(f.clone(), Box::new(f.clone()), Policy::default());
        (f, c)
    }

    fn run_all(c: &mut Coordinator<FakePlatform>) -> RunRecord {
        loop {
            if let Step::Completed(r) = c.step() {
                return *r;
            }
        }
    }

    #[test]
    fn full_run_with_follow_up() {
        let (f, mut c) = setup();
        let a = f.add(
            process(10, "a.exe", 400)
                .ws_after_trim_mib(10)
                .trim_cost(30),
        );
        let b = f.add(process(11, "b.exe", 200).trim_cost(20));
        f.add(process(12, "denied.exe", 300).behaviour(Behaviour::DenyOpen));
        f.add(process(4, "System", 500).location(ImageLocation::System));
        f.add(process(13, "tiny.exe", 5));

        let plan = c.plan(&Selection::AllEligible).unwrap();
        assert_eq!(c.phase(), Phase::Planned);
        assert_eq!(plan.targets.len(), 3);
        assert_eq!(
            c.start(plan.id + 1, RunMode::Trim),
            Err(CommandError::UnknownPlan)
        );
        assert_eq!(c.start(plan.id, RunMode::Trim), Ok(None));
        assert_eq!(c.phase(), Phase::Running);
        assert_eq!(c.plan(&Selection::AllEligible), Err(CommandError::Busy));

        let mut progress = vec![];
        let record = loop {
            match c.step() {
                Step::Progress {
                    done,
                    total,
                    result,
                } => {
                    assert_eq!(total, 3);
                    progress.push((done, result.key.pid));
                }
                Step::Completed(r) => break *r,
                Step::Idle => panic!("idle during run"),
            }
        };
        assert_eq!(progress, vec![(1, 10), (2, 12), (3, 11)], "largest first");
        assert_eq!(record.elapsed, Millis(50));
        assert_eq!(c.phase(), Phase::FollowUp);
        assert_eq!(c.step(), Step::Idle);

        // Rebound before the +10 s sample.
        f.touch(a, 110, 5000);
        assert_eq!(c.next_follow_up(), Some(Millis(50 + 10_000)));
        f.advance(9_999);
        assert_eq!(c.tick(), (vec![], None));
        f.advance(1);
        assert_eq!(c.tick().0, vec![FollowUpPoint::Plus10s]);
        f.exit(b);
        f.advance(50_000);
        let (pts, done) = c.tick();
        assert_eq!(pts, vec![FollowUpPoint::Plus60s]);
        let rec = done.expect("finalized after +60 s");
        assert_eq!(c.phase(), Phase::Idle);
        assert_eq!(rec.completion, Completion::Complete);
        assert_eq!(rec.follow_ups.len(), 2);

        let t = rec.totals();
        assert_eq!((t.trimmed, t.failed, t.skipped), (2, 1, 2));
        let an = analyse(&rec);
        let pa = an.processes.iter().find(|p| p.key == a).unwrap();
        assert_eq!(pa.reduced, Bytes::from_mib(390));
        assert_eq!(pa.rebound[0].percent, Some(25));
        assert!(
            an.processes
                .iter()
                .find(|p| p.key == b)
                .unwrap()
                .exited_during_follow_up
        );
        assert!(!an.any_late);
    }

    #[test]
    fn dry_run_does_not_trim() {
        let (f, mut c) = setup();
        let a = f.add(process(10, "a.exe", 400));
        let plan = c.plan(&Selection::AllEligible).unwrap();
        c.start(plan.id, RunMode::DryRun).unwrap();
        let rec = run_all(&mut c);
        assert_eq!(f.get(a).unwrap().trims, 0);
        assert!(!f.log().iter().any(|l| l.starts_with("trim")));
        match &rec.results[0].outcome {
            ProcessOutcome::Trimmed { before, after } => assert_eq!(before, after),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn cancel_during_run_and_follow_up() {
        let (f, mut c) = setup();
        for pid in 10..15 {
            f.add(process(pid, "a.exe", 100 + u64::from(pid)));
        }
        let plan = c.plan(&Selection::AllEligible).unwrap();
        c.start(plan.id, RunMode::Trim).unwrap();
        assert!(matches!(c.step(), Step::Progress { done: 1, .. }));
        assert_eq!(c.cancel(), None);
        let rec = run_all(&mut c);
        assert_eq!(rec.completion, Completion::Cancelled);
        let t = rec.totals();
        assert_eq!((t.trimmed, t.cancelled), (1, 4));
        // Follow-up still runs for what was trimmed; cancelling it truncates.
        let rec = c.cancel().expect("finalized");
        assert_eq!(
            rec.completion,
            Completion::Cancelled,
            "cancel keeps precedence"
        );
        assert_eq!(c.phase(), Phase::Idle);
        assert_eq!(c.cancel(), None);
        assert_eq!(c.step(), Step::Idle);
    }

    #[test]
    fn new_run_truncates_follow_up_and_failures_are_classified() {
        let (f, mut c) = setup();
        f.add(process(10, "a.exe", 100));
        f.add(process(11, "exits.exe", 100).behaviour(Behaviour::ExitBeforeTrim));
        f.add(process(12, "gone.exe", 100).behaviour(Behaviour::ExitBeforeOpen));
        f.add(process(13, "reused.exe", 100).behaviour(Behaviour::PidReused));
        f.add(process(14, "denytrim.exe", 100).behaviour(Behaviour::DenyTrim));
        let plan = c.plan(&Selection::AllEligible).unwrap();
        c.start(plan.id, RunMode::Trim).unwrap();
        let rec = run_all(&mut c);
        let kinds: Vec<_> = rec
            .results
            .iter()
            .filter_map(|r| match &r.outcome {
                ProcessOutcome::Failed(k) => Some((r.key.pid, k.clone())),
                _ => None,
            })
            .collect();
        assert!(kinds.contains(&(11, FailureKind::Exited)));
        assert!(kinds.contains(&(12, FailureKind::Exited)));
        assert!(kinds.contains(&(13, FailureKind::IdentityChanged)));
        assert!(kinds.contains(&(14, FailureKind::AccessDenied)));

        f.advance(10_000);
        assert_eq!(c.tick().0.len(), 1);
        let plan2 = c.plan(&Selection::AllEligible).unwrap();
        assert_eq!(
            c.phase(),
            Phase::FollowUp,
            "planning during follow-up is allowed"
        );
        let prev = c
            .start(plan2.id, RunMode::DryRun)
            .unwrap()
            .expect("previous run returned");
        assert_eq!(prev.completion, Completion::FollowUpTruncated);
        assert_eq!(prev.follow_ups.len(), 1);
        assert_eq!(c.phase(), Phase::Running);
    }

    #[test]
    fn late_samples_are_flagged() {
        let (f, mut c) = setup();
        f.add(process(10, "a.exe", 100));
        let plan = c.plan(&Selection::AllEligible).unwrap();
        c.start(plan.id, RunMode::Trim).unwrap();
        run_all(&mut c);
        // Machine slept: both points are overdue at the same time.
        f.advance(120_000);
        let (pts, rec) = c.tick();
        assert_eq!(pts.len(), 2);
        let rec = rec.unwrap();
        assert!(rec.follow_ups.iter().all(FollowUp::is_late));
        assert!(analyse(&rec).any_late);
    }

    #[test]
    fn table_planning_and_discard() {
        let (f, mut c) = setup();
        let k = f.add(process(10, "a.exe", 100));
        f.set_foreground(Some(k));
        let rows = c.table().unwrap();
        assert_eq!(rows[0].1, Eligibility::Foreground);
        let plan = c.plan(&Selection::AllEligible).unwrap();
        assert!(plan.targets.is_empty());
        c.discard();
        assert_eq!(c.phase(), Phase::Idle);
        c.plan(&Selection::AllEligible).unwrap();
        assert_eq!(c.cancel(), None);
        assert_eq!(c.phase(), Phase::Idle);
        f.fail_enumerate(true);
        assert!(matches!(
            c.plan(&Selection::AllEligible),
            Err(CommandError::Platform(_))
        ));
        assert!(c.table().is_err());
        assert_eq!(CommandError::Busy.to_string(), "a run is in progress");
        assert!(format!("{c:?}").contains("Idle"));
        let _ = c.platform();
        assert_eq!(c.now(), Millis(0));
        let empty = c.finalize(false);
        assert_eq!(empty.plan.id, 0);
    }
}
