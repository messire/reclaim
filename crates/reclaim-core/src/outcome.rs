//! Run outcomes and the run record (ARCHITECTURE §5.5). The record is what history stores
//! (`history.jsonl`, schema version [`SCHEMA_VERSION`]).

use serde::{Deserialize, Serialize};

use crate::model::{ImageName, MemorySnapshot, ProcessKey, SystemSnapshot};
use crate::plan::Plan;
use crate::policy::Eligibility;
use crate::units::{Bytes, Millis};

/// Version of the serialized [`RunRecord`].
pub const SCHEMA_VERSION: u32 = 1;

/// Trim for real, or measure the same timeline without acting (control).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RunMode {
    /// Trim working sets.
    Trim,
    /// Do everything except the trim call: the control measurement.
    DryRun,
}

/// Why acting on a process failed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailureKind {
    /// The OS denied access.
    AccessDenied,
    /// The process exited before or during the attempt.
    Exited,
    /// The PID now belongs to another process (start time changed).
    IdentityChanged,
    /// The action is not supported for this process.
    Unsupported,
    /// Any other OS error code.
    Os(i64),
    /// A bug in Reclaim was contained (panic in the engine).
    Internal(String),
}

impl FailureKind {
    /// Stable text key.
    pub fn key(&self) -> &'static str {
        match self {
            FailureKind::AccessDenied => "failure.access_denied",
            FailureKind::Exited => "failure.exited",
            FailureKind::IdentityChanged => "failure.identity_changed",
            FailureKind::Unsupported => "failure.unsupported",
            FailureKind::Os(_) => "failure.os",
            FailureKind::Internal(_) => "failure.internal",
        }
    }
}

/// What happened to one process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProcessOutcome {
    /// The trim call succeeded (or, in a dry run, was deliberately not made).
    Trimmed {
        /// Counters just before.
        before: MemorySnapshot,
        /// Counters just after.
        after: MemorySnapshot,
    },
    /// Decided before acting.
    Skipped(Eligibility),
    /// Attempted and failed.
    Failed(FailureKind),
    /// Not attempted because the run was cancelled.
    Cancelled,
}

impl ProcessOutcome {
    /// Only `Trimmed` counts as success, in every total.
    pub fn is_success(&self) -> bool {
        matches!(self, ProcessOutcome::Trimmed { .. })
    }
}

/// One process line of a run record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessResult {
    /// Identity.
    pub key: ProcessKey,
    /// Image name.
    pub image: ImageName,
    /// Outcome.
    pub outcome: ProcessOutcome,
    /// Time the call took, for trimmed processes.
    pub elapsed: Option<Millis>,
}

/// Which follow-up point a sample belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FollowUpPoint {
    /// About ten seconds after the run.
    Plus10s,
    /// About sixty seconds after the run.
    Plus60s,
}

impl FollowUpPoint {
    /// Target offset from the end of the run.
    pub const fn offset(self) -> Millis {
        match self {
            FollowUpPoint::Plus10s => Millis(10_000),
            FollowUpPoint::Plus60s => Millis(60_000),
        }
    }
}

/// A sample later than this past its target is flagged (review F24).
pub const LATE_TOLERANCE: Millis = Millis(2_000);

/// System and per-process sample taken after the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowUp {
    /// Which point.
    pub point: FollowUpPoint,
    /// Actual offset from the end of the run.
    pub actual_offset: Millis,
    /// System metrics.
    pub system: SystemSnapshot,
    /// Memory of trimmed processes; `None` if the process has exited.
    pub processes: Vec<(ProcessKey, Option<MemorySnapshot>)>,
}

impl FollowUp {
    /// True if the sample is more than [`LATE_TOLERANCE`] after its target.
    pub fn is_late(&self) -> bool {
        self.actual_offset.since(self.point.offset()) > LATE_TOLERANCE
    }
}

/// How the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Completion {
    /// All targets attempted, follow-up complete.
    Complete,
    /// The user cancelled during the run.
    Cancelled,
    /// Follow-up was cut short (new run, quit, cancel).
    FollowUpTruncated,
}

/// Everything known about one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    /// [`SCHEMA_VERSION`] at the time of writing.
    pub schema_version: u32,
    /// Reclaim version that wrote the record.
    pub app_version: String,
    /// Trim or dry run.
    pub mode: RunMode,
    /// Wall-clock start (Unix milliseconds).
    pub started_at: Millis,
    /// Duration of the trimming phase.
    pub elapsed: Millis,
    /// The plan that was executed.
    pub plan: Plan,
    /// One line per process in the plan (targets and skips).
    pub results: Vec<ProcessResult>,
    /// System before the run (T0).
    pub system_before: SystemSnapshot,
    /// System right after the run (T1).
    pub system_after: SystemSnapshot,
    /// Follow-up samples, in time order.
    pub follow_ups: Vec<FollowUp>,
    /// How the run ended.
    pub completion: Completion,
}

impl RunRecord {
    /// Starts a record for a plan; results and samples are filled in by the engine.
    pub fn new(
        plan: Plan,
        mode: RunMode,
        started_at: Millis,
        system_before: SystemSnapshot,
    ) -> Self {
        let results = plan
            .skipped
            .iter()
            .map(|s| ProcessResult {
                key: s.key,
                image: s.image.clone(),
                outcome: ProcessOutcome::Skipped(s.reason.clone()),
                elapsed: None,
            })
            .collect();
        Self {
            schema_version: SCHEMA_VERSION,
            app_version: crate::VERSION.to_owned(),
            mode,
            started_at,
            elapsed: Millis(0),
            plan,
            results,
            system_before,
            system_after: system_before,
            follow_ups: Vec::new(),
            completion: Completion::Complete,
        }
    }

    /// Serializes to one JSON line.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parses a JSON line; records with a newer schema are rejected.
    pub fn from_json(s: &str) -> Result<Self, RecordError> {
        let r: RunRecord =
            serde_json::from_str(s).map_err(|e| RecordError::Malformed(e.to_string()))?;
        if r.schema_version > SCHEMA_VERSION {
            return Err(RecordError::NewerSchema(r.schema_version));
        }
        Ok(r)
    }

    /// Totals over the results.
    pub fn totals(&self) -> Totals {
        Totals::of(&self.results)
    }
}

/// Error reading a stored record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    /// Not valid JSON for this schema.
    Malformed(String),
    /// Written by a newer Reclaim.
    NewerSchema(u32),
}

/// Counts and sums over outcomes. Skipped, failed and cancelled processes never add to the
/// success figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Totals {
    /// Trimmed processes.
    pub trimmed: usize,
    /// Skipped processes.
    pub skipped: usize,
    /// Failed processes.
    pub failed: usize,
    /// Cancelled processes.
    pub cancelled: usize,
    /// Sum of working-set reductions of trimmed processes, measured right after each call.
    pub working_set_reduced: Bytes,
    /// Private bytes of trimmed processes before the call.
    pub private_bytes_before: Bytes,
    /// Private bytes of trimmed processes after the call.
    pub private_bytes_after: Bytes,
}

impl Totals {
    /// Computes totals over a slice of results.
    pub fn of(results: &[ProcessResult]) -> Totals {
        let mut t = Totals::default();
        for r in results {
            match &r.outcome {
                ProcessOutcome::Trimmed { before, after } => {
                    t.trimmed += 1;
                    t.working_set_reduced += before.working_set.saturating_sub(after.working_set);
                    t.private_bytes_before += before.private_bytes;
                    t.private_bytes_after += after.private_bytes;
                }
                ProcessOutcome::Skipped(_) => t.skipped += 1,
                ProcessOutcome::Failed(_) => t.failed += 1,
                ProcessOutcome::Cancelled => t.cancelled += 1,
            }
        }
        t
    }

    /// Number of processes considered.
    pub fn total(&self) -> usize {
        self.trimmed + self.skipped + self.failed + self.cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Plan, Selection};
    use crate::policy::tests::proc_;
    use crate::policy::{Context, Policy};

    fn mem(ws: u64, pb: u64) -> MemorySnapshot {
        MemorySnapshot {
            working_set: Bytes::from_mib(ws),
            private_bytes: Bytes::from_mib(pb),
            ..Default::default()
        }
    }

    fn result(outcome: ProcessOutcome) -> ProcessResult {
        ProcessResult {
            key: ProcessKey::new(1, 1),
            image: ImageName::new("a.exe"),
            outcome,
            elapsed: None,
        }
    }

    #[test]
    fn totals_count_only_trimmed_as_success() {
        let rs = vec![
            result(ProcessOutcome::Trimmed {
                before: mem(300, 200),
                after: mem(10, 200),
            }),
            result(ProcessOutcome::Trimmed {
                before: mem(5, 5),
                after: mem(9, 5),
            }),
            result(ProcessOutcome::Failed(FailureKind::AccessDenied)),
            result(ProcessOutcome::Skipped(Eligibility::Foreground)),
            result(ProcessOutcome::Cancelled),
        ];
        let t = Totals::of(&rs);
        assert_eq!((t.trimmed, t.failed, t.skipped, t.cancelled), (2, 1, 1, 1));
        assert_eq!(t.total(), 5);
        // A process that grew is counted as 0, not as a negative reduction.
        assert_eq!(t.working_set_reduced, Bytes::from_mib(290));
        assert_eq!(t.private_bytes_before, t.private_bytes_after);
        assert!(rs[0].outcome.is_success());
        assert!(!rs[2].outcome.is_success());
    }

    #[test]
    fn late_follow_up() {
        let mut f = FollowUp {
            point: FollowUpPoint::Plus10s,
            actual_offset: Millis(12_000),
            system: SystemSnapshot::default(),
            processes: vec![],
        };
        assert!(!f.is_late());
        f.actual_offset = Millis(12_001);
        assert!(f.is_late());
        f.point = FollowUpPoint::Plus60s;
        assert!(!f.is_late());
    }

    #[test]
    fn record_round_trip_and_schema_check() {
        let procs = vec![proc_(10, "a.exe", 100), proc_(4, "System", 10)];
        let ctx = Context {
            self_pid: 1,
            foreground: None,
        };
        let plan = Plan::build(3, &procs, &Policy::default(), &ctx, &Selection::AllEligible);
        let mut r = RunRecord::new(plan, RunMode::DryRun, Millis(5), SystemSnapshot::default());
        assert_eq!(r.results.len(), 1, "skips are recorded up front");
        r.results
            .push(result(ProcessOutcome::Failed(FailureKind::Os(87))));
        let line = r.to_json();
        assert!(!line.contains('\n'));
        let back = RunRecord::from_json(&line).unwrap();
        assert_eq!(back, r);

        let newer = line.replacen("\"schema_version\":1", "\"schema_version\":2", 1);
        assert_eq!(
            RunRecord::from_json(&newer),
            Err(RecordError::NewerSchema(2))
        );
        assert!(matches!(
            RunRecord::from_json("{"),
            Err(RecordError::Malformed(_))
        ));
        assert_eq!(r.totals().failed, 1);
    }

    #[test]
    fn failure_keys_unique() {
        let all = [
            FailureKind::AccessDenied,
            FailureKind::Exited,
            FailureKind::IdentityChanged,
            FailureKind::Unsupported,
            FailureKind::Os(5),
            FailureKind::Internal("x".into()),
        ];
        let keys: std::collections::HashSet<_> = all.iter().map(FailureKind::key).collect();
        assert_eq!(keys.len(), all.len());
    }
}
