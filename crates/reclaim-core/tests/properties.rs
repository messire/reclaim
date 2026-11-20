//! Property tests (ARCHITECTURE §10): totals never count skipped or failed processes as
//! success, and reports never contain forbidden phrases.

use proptest::prelude::*;
use reclaim_core::analysis::analyse;
use reclaim_core::model::{ImageName, MemorySnapshot, Pressure, ProcessKey, SystemSnapshot};
use reclaim_core::outcome::{
    Completion, FailureKind, FollowUp, FollowUpPoint, ProcessOutcome, ProcessResult, RunMode,
    RunRecord, Totals,
};
use reclaim_core::plan::Plan;
use reclaim_core::policy::Eligibility;
use reclaim_core::report::{FORBIDDEN, report};
use reclaim_core::units::{Bytes, Millis};

fn mem() -> impl Strategy<Value = MemorySnapshot> {
    (0u64..4 << 30, 0u64..4 << 30, 0u64..10_000_000).prop_map(|(ws, pb, pf)| MemorySnapshot {
        working_set: Bytes(ws),
        private_working_set: None,
        private_bytes: Bytes(pb),
        page_faults: pf,
        io_bytes: None,
    })
}

fn outcome() -> impl Strategy<Value = ProcessOutcome> {
    prop_oneof![
        (mem(), mem()).prop_map(|(before, after)| ProcessOutcome::Trimmed { before, after }),
        Just(ProcessOutcome::Skipped(Eligibility::Foreground)),
        Just(ProcessOutcome::Skipped(Eligibility::SystemCritical)),
        Just(ProcessOutcome::Failed(FailureKind::AccessDenied)),
        Just(ProcessOutcome::Failed(FailureKind::Exited)),
        (0i64..100).prop_map(|c| ProcessOutcome::Failed(FailureKind::Os(c))),
        Just(ProcessOutcome::Cancelled),
    ]
}

fn sys() -> impl Strategy<Value = SystemSnapshot> {
    (
        0u64..32 << 30,
        0u64..64 << 30,
        proptest::option::of(0u64..5000),
    )
        .prop_map(|(avail, commit, pin)| SystemSnapshot {
            phys_total: Bytes(32 << 30),
            phys_available: Bytes(avail),
            commit_charge: Bytes(commit),
            commit_limit: Bytes(64 << 30),
            pages_input_per_sec: pin,
            ..Default::default()
        })
}

fn record() -> impl Strategy<Value = RunRecord> {
    (
        proptest::collection::vec(outcome(), 0..20),
        sys(),
        sys(),
        proptest::collection::vec((sys(), 9_000u64..80_000, proptest::bool::ANY), 0..3),
        prop_oneof![Just(RunMode::Trim), Just(RunMode::DryRun)],
        prop_oneof![
            Just(Completion::Complete),
            Just(Completion::Cancelled),
            Just(Completion::FollowUpTruncated)
        ],
        mem(),
    )
        .prop_map(|(outcomes, before, after, fus, mode, completion, later)| {
            let plan = Plan {
                id: 1,
                lists_version: 1,
                targets: vec![],
                skipped: vec![],
            };
            let mut r = RunRecord::new(plan, mode, Millis(0), before);
            r.system_after = after;
            r.completion = completion;
            r.results = outcomes
                .into_iter()
                .enumerate()
                .map(|(i, outcome)| ProcessResult {
                    key: ProcessKey::new(i as u32 + 10, 1),
                    image: ImageName::new("p.exe"),
                    outcome,
                    elapsed: None,
                })
                .collect();
            let keys: Vec<ProcessKey> = r.results.iter().map(|x| x.key).collect();
            for (i, (system, offset, alive)) in fus.into_iter().enumerate() {
                r.follow_ups.push(FollowUp {
                    point: if i == 0 {
                        FollowUpPoint::Plus10s
                    } else {
                        FollowUpPoint::Plus60s
                    },
                    actual_offset: Millis(offset),
                    system,
                    processes: keys.iter().map(|k| (*k, alive.then_some(later))).collect(),
                });
            }
            r
        })
}

proptest! {
    #[test]
    fn totals_never_count_failures_as_success(r in record()) {
        let t: Totals = r.totals();
        let successes = r.results.iter().filter(|x| x.outcome.is_success()).count();
        prop_assert_eq!(t.trimmed, successes);
        prop_assert_eq!(t.total(), r.results.len());
        // Only trimmed processes contribute to the reduction.
        let expected: Bytes = r.results.iter().filter_map(|x| match &x.outcome {
            ProcessOutcome::Trimmed { before, after } => Some(before.working_set.saturating_sub(after.working_set)),
            _ => None,
        }).sum();
        prop_assert_eq!(t.working_set_reduced, expected);
    }

    #[test]
    fn reports_never_use_forbidden_phrases(r in record(), p in prop_oneof![Just(Pressure::High), Just(Pressure::Normal), Just(Pressure::Low)]) {
        let a = analyse(&r);
        let statements = report(&a, p);
        prop_assert_eq!(statements.last().map(|s| s.key), Some("report.responsiveness"));
        let has_commit = statements.iter().any(|s| s.key == "report.commit");
        prop_assert!(has_commit, "committed memory must always be shown");
        for s in &statements {
            let text = s.render_en().to_lowercase();
            prop_assert!(!text.is_empty());
            let unfilled = text.contains('{');
            prop_assert!(!unfilled, "unfilled placeholder");
            for f in FORBIDDEN {
                prop_assert!(!text.contains(f), "{} contains {}", text, f);
            }
        }
        if r.mode == RunMode::DryRun {
            prop_assert!(statements.iter().all(|s| s.key != "report.ws_reduced"));
        }
    }

    #[test]
    fn records_round_trip(r in record()) {
        let back = RunRecord::from_json(&r.to_json()).unwrap();
        prop_assert_eq!(back, r);
    }
}
