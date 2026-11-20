//! `summary.md` writer and pass/regression rules (BENCHMARKS §6, §7).

use std::fmt::Write;

use reclaim_core::outcome::{FollowUpPoint, ProcessOutcome, RunMode};
use reclaim_core::units::{ByteDelta, Bytes};

use crate::run::{Arm, Resources};
use crate::stats::{percentile, trends_upward};

/// A trim arm is a regression when the latency probe's p95 after trimming exceeds the
/// control's by more than this many percent (BENCHMARKS §6).
pub const LATENCY_REGRESSION_PCT: f64 = 20.0;

fn mode(m: RunMode) -> &'static str {
    match m {
        RunMode::Trim => "trim",
        RunMode::DryRun => "control (dry run)",
    }
}

fn change_at(a: &Arm, p: Option<FollowUpPoint>) -> (String, String) {
    a.analysis
        .system
        .iter()
        .find(|s| s.point == p)
        .map_or(("–".into(), "–".into()), |s| {
            let late = if s.late { " (late)" } else { "" };
            (format!("{}{late}", s.available), format!("{}", s.committed))
        })
}

fn p95_ms(v: &[f64]) -> Option<f64> {
    percentile(v, 95.0).map(|us| us / 1000.0)
}

fn opt_ms(v: Option<f64>) -> String {
    v.map_or("–".into(), |ms| format!("{ms:.1} ms"))
}

/// Unsafe or incorrect behaviour: a control arm that changed working sets, or internal
/// failures. Any of these fails the scenario.
pub fn faults(arms: &[Arm]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, a) in arms.iter().enumerate() {
        // A dry run reports targets as "trimmed" with before = after; memory must not move.
        let moved = a.mode == RunMode::DryRun
            && a.record.results.iter().any(|r| match &r.outcome {
                ProcessOutcome::Trimmed { before, after } => {
                    after.working_set.get() < before.working_set.get() / 2
                }
                _ => false,
            });
        if moved {
            out.push(format!(
                "arm {}: control arm shows trimmed working sets",
                i + 1
            ));
        }
        let internal = a
            .record
            .results
            .iter()
            .filter(|r| {
                matches!(
                    r.outcome,
                    ProcessOutcome::Failed(reclaim_core::outcome::FailureKind::Internal(_))
                )
            })
            .count();
        if internal > 0 {
            out.push(format!("arm {}: {internal} internal failures", i + 1));
        }
    }
    out
}

/// Latency regressions of trim arms against the control (BENCHMARKS §6). A regression is a
/// result to report, not a harness failure.
pub fn regressions(arms: &[Arm]) -> Vec<String> {
    let control = arms
        .iter()
        .filter(|a| a.mode == RunMode::DryRun)
        .filter_map(|a| p95_ms(&a.latency_after))
        .reduce(f64::max);
    let mut out = Vec::new();
    for (i, a) in arms.iter().enumerate() {
        if let (RunMode::Trim, Some(c), Some(t)) = (a.mode, control, p95_ms(&a.latency_after))
            && c > 0.0
            && (t - c) / c * 100.0 > LATENCY_REGRESSION_PCT
        {
            out.push(format!(
                "arm {}: regression: latency p95 after trim {t:.1} ms vs control {c:.1} ms (> {LATENCY_REGRESSION_PCT} %)",
                i + 1
            ));
        }
    }
    out
}

/// Faults, then regressions.
pub fn findings(arms: &[Arm]) -> Vec<String> {
    let mut f = faults(arms);
    f.extend(regressions(arms));
    f
}

/// Renders the scenario summary.
pub fn scenario_md(title: &str, context: &[(&str, String)], arms: &[Arm]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# {title}\n");
    for (k, v) in context {
        let _ = writeln!(s, "- {k}: {v}");
    }
    let _ = writeln!(
        s,
        "\n| Arm | Mode | Targets | Trimmed | Skipped | Failed | Elapsed (B5) | WS reduced | Avail Δ T1 | Avail Δ +10 s | Avail Δ +60 s | Commit Δ +60 s | Rebound +60 s | Faults after | Paging peak | Latency p95 before | Latency p95 after |"
    );
    let _ = writeln!(s, "|{}", "---|".repeat(17));
    for (i, a) in arms.iter().enumerate() {
        let t = &a.analysis.totals;
        let (t1, _) = change_at(a, None);
        let (p10, _) = change_at(a, Some(FollowUpPoint::Plus10s));
        let (p60, c60) = change_at(a, Some(FollowUpPoint::Plus60s));
        let rebound = a.analysis.final_rebound().map_or("–".into(), |r| {
            r.percent
                .map_or(format!("{}", r.regained), |p| format!("{p} %"))
        });
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} ms | {} | {t1} | {p10} | {p60} | {c60} | {rebound} | {} | {} | {} | {} |",
            i + 1,
            mode(a.mode),
            a.record.plan.targets.len(),
            t.trimmed,
            t.skipped + a.record.plan.skipped.len(),
            t.failed,
            a.record.elapsed.0,
            t.working_set_reduced,
            a.analysis.faults_after,
            a.analysis
                .paging_input_peak
                .map_or("–".into(), |p| format!("{p}/s")),
            opt_ms(p95_ms(&a.latency_before)),
            opt_ms(p95_ms(&a.latency_after)),
        );
    }
    let mut reasons: std::collections::BTreeMap<&str, usize> = Default::default();
    for a in arms.iter().take(1) {
        for (k, n) in a.record.plan.skip_counts() {
            *reasons.entry(k).or_default() += n;
        }
    }
    if !reasons.is_empty() {
        let _ = writeln!(s, "\nSkip reasons (arm 1): {reasons:?}");
    }
    let f = findings(arms);
    let _ = writeln!(s, "\n## Findings\n");
    if f.is_empty() {
        let _ = writeln!(
            s,
            "No regression or unsafe behaviour detected. A lower working set alone is not a pass: read the rebound and latency columns."
        );
    }
    for line in f {
        let _ = writeln!(s, "- {line}");
    }
    s
}

/// Renders the soak summary (B8) and whether it passed.
pub fn soak_md(marks: &[(u64, Resources)], failed: u64) -> (String, bool) {
    let mut s = String::from(
        "# Soak (B8)\n\n| Cycle | Handles | GDI | USER | Private bytes |\n|---|---|---|---|---|\n",
    );
    for (c, r) in marks {
        let _ = writeln!(
            s,
            "| {c} | {} | {} | {} | {} |",
            r.handles,
            r.gdi,
            r.user,
            Bytes(r.private_bytes)
        );
    }
    let series = |f: fn(&Resources) -> u64| marks.iter().map(|(_, r)| f(r)).collect::<Vec<_>>();
    let leaks: Vec<&str> = [
        ("handles", trends_upward(&series(|r| r.handles), 10, 16)),
        ("GDI objects", trends_upward(&series(|r| r.gdi), 10, 8)),
        ("USER objects", trends_upward(&series(|r| r.user), 10, 8)),
        (
            "private bytes",
            trends_upward(&series(|r| r.private_bytes), 10, 4 * 1024 * 1024),
        ),
    ]
    .into_iter()
    .filter(|(_, up)| *up)
    .map(|(n, _)| n)
    .collect();
    let pass = leaks.is_empty();
    let _ = writeln!(
        s,
        "\nFailed cycles: {failed}. Result: {}",
        if pass {
            "pass (no upward trend)".to_owned()
        } else {
            format!("FAIL, upward trend in {}", leaks.join(", "))
        }
    );
    (s, pass)
}

/// Total available-memory change at +60 s, for quick comparisons.
pub fn available_at_60s(a: &Arm) -> Option<ByteDelta> {
    a.analysis
        .system
        .iter()
        .find(|s| s.point == Some(FollowUpPoint::Plus60s))
        .map(|s| s.available)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{arm, run_arm};
    use reclaim_core::plan::Selection;
    use reclaim_core::policy::Policy;
    use reclaim_engine::Coordinator;
    use reclaim_platform_fake::{FakePlatform, process};

    fn arms(trim_latency: f64) -> Vec<Arm> {
        let f = FakePlatform::new();
        f.add(process(10, "a.exe", 300));
        f.add(process(11, "tiny.exe", 1));
        let mut c = Coordinator::new(f.clone(), Box::new(f.clone()), Policy::default());
        let g = f.clone();
        let mut wait = |d: std::time::Duration| g.advance(d.as_millis() as u64);
        let ctl = run_arm(
            &mut c,
            RunMode::DryRun,
            &Selection::AllEligible,
            true,
            &mut wait,
        )
        .unwrap();
        let trim = run_arm(
            &mut c,
            RunMode::Trim,
            &Selection::AllEligible,
            true,
            &mut wait,
        )
        .unwrap();
        vec![
            arm(RunMode::DryRun, ctl, vec![1000.0], vec![1000.0]),
            arm(RunMode::Trim, trim, vec![1000.0], vec![trim_latency]),
        ]
    }

    #[test]
    fn summary_flags_latency_regressions_only() {
        let ok = arms(1100.0);
        assert!(findings(&ok).is_empty(), "{:?}", findings(&ok));
        let md = scenario_md("S2", &[("machine", "fake".into())], &ok);
        assert!(md.contains("| 2 | trim | 1 | 1 |"), "{md}");
        assert!(md.contains("No regression"));
        assert!(md.contains("reason.below_threshold"));
        assert!(available_at_60s(&ok[1]).is_some());

        let bad = arms(1500.0);
        let f = findings(&bad);
        assert_eq!(f.len(), 1);
        assert!(f[0].contains("regression"));
        assert!(faults(&bad).is_empty());
        assert_eq!(regressions(&bad).len(), 1);
        assert!(scenario_md("S2", &[], &bad).contains("- arm 2: regression"));
    }

    #[test]
    fn soak_summary() {
        let r = |h, p| Resources {
            handles: h,
            gdi: 0,
            user: 0,
            private_bytes: p,
        };
        let (md, pass) = soak_md(
            &[
                (0, r(100, 5 << 20)),
                (10, r(120, 6 << 20)),
                (100, r(121, 6 << 20)),
                (1000, r(122, 6 << 20)),
            ],
            0,
        );
        assert!(pass, "{md}");
        let (md, pass) = soak_md(
            &[
                (0, r(100, 5 << 20)),
                (10, r(120, 6 << 20)),
                (100, r(400, 6 << 20)),
                (1000, r(4000, 6 << 20)),
            ],
            2,
        );
        assert!(!pass);
        assert!(md.contains("FAIL, upward trend in handles"));
        assert!(md.contains("Failed cycles: 2"));
    }
}
