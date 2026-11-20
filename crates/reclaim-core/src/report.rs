//! Report statements and wording rules (ARCHITECTURE §5.6).
//!
//! A report is a list of [`Statement`]s: a stable text key plus typed parameters and a
//! reference to the evidence it rests on. The UI renders keys through its string
//! resources; [`Statement::render_en`] is the English reference used by the CLI and by
//! the tests that enforce the wording rules.

use serde::{Deserialize, Serialize};

use crate::analysis::RunAnalysis;
use crate::model::Pressure;
use crate::outcome::{Completion, FollowUpPoint, RunMode};
use crate::units::{ByteDelta, Bytes, Count};

/// How a statement is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tone {
    /// Neutral fact.
    Info,
    /// Something the user should notice (for example large rebound).
    Warning,
    /// A cost the run caused.
    Cost,
    /// A caveat about the measurement itself.
    Note,
}

/// What a statement is based on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Evidence {
    /// Run mode.
    Mode,
    /// Per-process outcome totals.
    Totals,
    /// System sample right after the run.
    SystemAfter,
    /// A follow-up sample.
    FollowUp(FollowUpPoint),
    /// Page-fault counters of trimmed processes.
    PageFaults,
    /// System paging-input counter.
    PagingInput,
    /// Nothing: a statement about what is not measured.
    None,
}

/// A typed parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Param {
    /// An amount of memory.
    Bytes(Bytes),
    /// A signed change.
    Delta(ByteDelta),
    /// A count.
    Count(Count),
    /// A percentage.
    Percent(u32),
    /// A follow-up point.
    Point(FollowUpPoint),
    /// A pressure level.
    Pressure(Pressure),
}

/// One sentence of a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    /// Text key.
    pub key: &'static str,
    /// Presentation.
    pub tone: Tone,
    /// Named parameters, in template order.
    pub params: Vec<(&'static str, Param)>,
    /// Evidence reference.
    pub evidence: Evidence,
}

impl Statement {
    fn new(key: &'static str, tone: Tone, evidence: Evidence) -> Self {
        Self {
            key,
            tone,
            params: Vec::new(),
            evidence,
        }
    }

    fn with(mut self, name: &'static str, p: Param) -> Self {
        self.params.push((name, p));
        self
    }

    /// Renders the English reference text.
    pub fn render_en(&self) -> String {
        let mut s = template_en(self.key).to_owned();
        for (name, p) in &self.params {
            let v = match p {
                Param::Bytes(b) => b.to_string(),
                Param::Delta(d) => d.to_string(),
                Param::Count(c) => c.to_string(),
                Param::Percent(p) => format!("{p} %"),
                Param::Point(FollowUpPoint::Plus10s) => "10 seconds".to_owned(),
                Param::Point(FollowUpPoint::Plus60s) => "60 seconds".to_owned(),
                Param::Pressure(Pressure::High) => "high".to_owned(),
                Param::Pressure(Pressure::Normal) => "normal".to_owned(),
                Param::Pressure(Pressure::Low) => "low".to_owned(),
            };
            s = s.replace(&format!("{{{name}}}"), &v);
        }
        s
    }
}

/// Rebound above this share of the trimmed amount is reported prominently.
pub const HIGH_REBOUND_PERCENT: u32 = 50;

/// All statement keys with their English templates.
pub const TEMPLATES_EN: &[(&str, &str)] = &[
    (
        "report.dry_run",
        "Dry run: nothing was trimmed. These numbers show what changed without trimming and serve as the control measurement.",
    ),
    (
        "report.totals",
        "{trimmed} processes trimmed, {skipped} skipped, {failed} failed, {cancelled} not attempted.",
    ),
    (
        "report.totals_dry",
        "{measured} processes measured without trimming, {skipped} skipped, {failed} failed, {cancelled} not attempted.",
    ),
    (
        "report.ws_reduced",
        "Working set reduced by {amount} in total, measured right after each process was trimmed.",
    ),
    (
        "report.commit",
        "Committed memory changed by {delta}. Trimming working sets does not release committed memory.",
    ),
    (
        "report.available",
        "Available memory changed by {delta} right after the run (memory pressure: {pressure}).",
    ),
    (
        "report.rebound",
        "{percent} of the trimmed working set came back within {point}.",
    ),
    (
        "report.rebound_high",
        "Most of the trimmed working set came back: {percent} within {point}. These processes were still using that memory.",
    ),
    (
        "report.faults",
        "Trimmed processes took {faults} page faults to bring pages back into memory.",
    ),
    (
        "report.faults_dry",
        "Measured processes took {faults} page faults during the follow-up without any trimming (their normal activity).",
    ),
    (
        "report.paging_peak",
        "Pages read from disk peaked at {peak} per second during the follow-up; the rate before the run was not measured.",
    ),
    (
        "report.paging_cost",
        "Pages read from disk rose from {before} to {peak} per second during the follow-up.",
    ),
    (
        "report.failures",
        "{failed} processes could not be trimmed; the list shows the reason for each.",
    ),
    (
        "report.cancelled",
        "The run was cancelled; remaining processes were not attempted.",
    ),
    (
        "report.follow_up_truncated",
        "The follow-up was cut short, so rebound figures are incomplete.",
    ),
    (
        "report.late_sample",
        "A follow-up sample was taken late (for example after sleep); its timing is approximate.",
    ),
    (
        "report.responsiveness",
        "Responsiveness: not measured by Reclaim.",
    ),
];

fn template_en(key: &str) -> &'static str {
    TEMPLATES_EN
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, t)| *t)
        .unwrap_or("")
}

/// Phrases a report must never contain (case-insensitive substrings).
pub const FORBIDDEN: &[&str] = &[
    "freed",
    "free up",
    "freeing",
    "clean",
    "boost",
    "faster",
    "speed",
    "improv",
    "optimi",
    "accelerat",
    "released memory",
    "ram cleaned",
];

/// Builds the report for an analysed run.
pub fn report(a: &RunAnalysis, pressure_after: Pressure) -> Vec<Statement> {
    let t = &a.totals;
    let mut out = Vec::new();
    if a.mode == RunMode::DryRun {
        out.push(Statement::new("report.dry_run", Tone::Note, Evidence::Mode));
    }
    // In a dry run a "trimmed" outcome is a measured process whose trim call was omitted.
    let dry = a.mode == RunMode::DryRun;
    out.push(
        Statement::new(
            if dry {
                "report.totals_dry"
            } else {
                "report.totals"
            },
            Tone::Info,
            Evidence::Totals,
        )
        .with(
            if dry { "measured" } else { "trimmed" },
            Param::Count(t.trimmed as Count),
        )
        .with("skipped", Param::Count(t.skipped as Count))
        .with("failed", Param::Count(t.failed as Count))
        .with("cancelled", Param::Count(t.cancelled as Count)),
    );
    if a.mode == RunMode::Trim && t.trimmed > 0 {
        out.push(
            Statement::new("report.ws_reduced", Tone::Info, Evidence::Totals)
                .with("amount", Param::Bytes(t.working_set_reduced)),
        );
    }
    if let Some(t1) = a.system.first() {
        out.push(
            Statement::new("report.available", Tone::Info, Evidence::SystemAfter)
                .with("delta", Param::Delta(t1.available))
                .with("pressure", Param::Pressure(pressure_after)),
        );
        out.push(
            Statement::new("report.commit", Tone::Info, Evidence::SystemAfter)
                .with("delta", Param::Delta(t1.committed)),
        );
    }
    if let Some(rb) = a.final_rebound()
        && let Some(pct) = rb.percent
    {
        let key = if pct > HIGH_REBOUND_PERCENT {
            ("report.rebound_high", Tone::Warning)
        } else {
            ("report.rebound", Tone::Info)
        };
        out.push(
            Statement::new(key.0, key.1, Evidence::FollowUp(rb.point))
                .with("percent", Param::Percent(pct))
                .with("point", Param::Point(rb.point)),
        );
    }
    if a.faults_after > 0 {
        out.push(
            Statement::new(
                if dry {
                    "report.faults_dry"
                } else {
                    "report.faults"
                },
                if dry { Tone::Note } else { Tone::Cost },
                Evidence::PageFaults,
            )
            .with("faults", Param::Count(a.faults_after)),
        );
    }
    if a.paging_cost_material() {
        let peak = Param::Count(a.paging_input_peak.unwrap_or(0));
        // Never claim a rise from a baseline that was not measured.
        out.push(match a.paging_input_before {
            Some(before) => Statement::new("report.paging_cost", Tone::Cost, Evidence::PagingInput)
                .with("before", Param::Count(before))
                .with("peak", peak),
            None => Statement::new("report.paging_peak", Tone::Note, Evidence::PagingInput)
                .with("peak", peak),
        });
    }
    if t.failed > 0 {
        out.push(
            Statement::new("report.failures", Tone::Info, Evidence::Totals)
                .with("failed", Param::Count(t.failed as Count)),
        );
    }
    match a.completion {
        Completion::Complete => {}
        Completion::Cancelled => {
            out.push(Statement::new(
                "report.cancelled",
                Tone::Note,
                Evidence::Totals,
            ));
        }
        Completion::FollowUpTruncated => out.push(Statement::new(
            "report.follow_up_truncated",
            Tone::Note,
            Evidence::None,
        )),
    }
    if a.any_late {
        out.push(Statement::new(
            "report.late_sample",
            Tone::Note,
            Evidence::None,
        ));
    }
    out.push(Statement::new(
        "report.responsiveness",
        Tone::Note,
        Evidence::None,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyse;
    use crate::analysis::tests::sample_record;

    fn keys(s: &[Statement]) -> Vec<&'static str> {
        s.iter().map(|s| s.key).collect()
    }

    #[test]
    fn trim_report() {
        let a = analyse(&sample_record(RunMode::Trim));
        let r = report(&a, Pressure::High);
        assert_eq!(
            keys(&r),
            vec![
                "report.totals",
                "report.ws_reduced",
                "report.available",
                "report.commit",
                "report.rebound_high",
                "report.faults",
                "report.paging_cost",
                "report.failures",
                "report.late_sample",
                "report.responsiveness",
            ]
        );
        let text: Vec<String> = r.iter().map(Statement::render_en).collect();
        assert_eq!(
            text[0],
            "2 processes trimmed, 0 skipped, 1 failed, 0 not attempted."
        );
        assert_eq!(
            text[1],
            "Working set reduced by 590 MiB in total, measured right after each process was trimmed."
        );
        assert_eq!(
            text[2],
            "Available memory changed by +590 MiB right after the run (memory pressure: high)."
        );
        assert!(text[3].contains("does not release committed memory"));
        assert!(
            text[4]
                .starts_with("Most of the trimmed working set came back: 76 % within 60 seconds.")
        );
        assert_eq!(
            text[6],
            "Pages read from disk rose from 20 to 400 per second during the follow-up."
        );
        for t in &text {
            assert!(!t.contains('{'), "unfilled placeholder in {t}");
        }
    }

    #[test]
    fn dry_run_report_is_the_control() {
        let mut rec = sample_record(RunMode::DryRun);
        rec.completion = Completion::FollowUpTruncated;
        let a = analyse(&rec);
        let r = report(&a, Pressure::Normal);
        assert_eq!(r[0].key, "report.dry_run");
        assert_eq!(r[1].key, "report.totals_dry");
        assert!(
            r[1].render_en()
                .starts_with("2 processes measured without trimming")
        );
        assert!(!keys(&r).contains(&"report.ws_reduced"));
        assert!(!keys(&r).contains(&"report.faults"));
        assert!(keys(&r).contains(&"report.follow_up_truncated"));
        assert_eq!(r.last().unwrap().key, "report.responsiveness");
    }

    #[test]
    fn paging_peak_without_baseline_claims_no_rise() {
        let mut a = analyse(&sample_record(RunMode::Trim));
        a.paging_input_before = None;
        let r = report(&a, Pressure::Normal);
        assert!(!keys(&r).contains(&"report.paging_cost"));
        let s = r.iter().find(|s| s.key == "report.paging_peak").unwrap();
        assert_eq!(
            s.render_en(),
            "Pages read from disk peaked at 400 per second during the follow-up; the rate before the run was not measured."
        );
    }

    #[test]
    fn low_rebound_and_cancel() {
        let mut rec = sample_record(RunMode::Trim);
        rec.follow_ups.truncate(1);
        rec.completion = Completion::Cancelled;
        let a = analyse(&rec);
        let r = report(&a, Pressure::Low);
        let rb = r.iter().find(|s| s.key == "report.rebound").unwrap();
        assert_eq!(
            rb.render_en(),
            "20 % of the trimmed working set came back within 10 seconds."
        );
        assert_eq!(rb.tone, Tone::Info);
        assert!(keys(&r).contains(&"report.cancelled"));
        assert!(!keys(&r).contains(&"report.late_sample"));
    }

    #[test]
    fn every_key_has_a_template_and_no_template_is_forbidden() {
        for (k, t) in TEMPLATES_EN {
            assert!(!t.is_empty(), "{k}");
            let lower = t.to_lowercase();
            for f in FORBIDDEN {
                assert!(!lower.contains(f), "{k} contains {f}");
            }
        }
        assert_eq!(template_en("missing"), "");
    }
}
