//! User-visible text built in Rust (PRODUCT §7: localization-ready).
//!
//! Static labels live in the Slint markup behind `@tr(...)`; text assembled from data
//! (status lines, reasons, confirmations) comes from this table by key, so a translation
//! replaces one table and never touches logic. Report statements come from
//! `reclaim_core::report` templates.

use reclaim_core::model::Pressure;

/// English strings by key. Placeholders are `{name}`.
pub const EN: &[(&str, &str)] = &[
    ("reason.self", "Reclaim itself"),
    ("reason.system_critical", "System process"),
    ("reason.user_excluded", "Excluded in settings"),
    ("reason.protected", "Protected by default"),
    ("reason.foreground", "In the foreground"),
    ("reason.other_user", "Another user's process"),
    ("reason.requires_elevation", "Needs administrator rights"),
    ("reason.not_accessible", "Access denied"),
    ("reason.below_threshold", "Below size threshold"),
    ("reason.eligible", "Eligible"),
    ("pressure.high", "high"),
    ("pressure.normal", "normal"),
    ("pressure.low", "low"),
    ("value.unknown", "not available"),
    ("overview.committed", "{used} of {limit}"),
    ("overview.load", "{percent} % in use"),
    ("overview.never", "No run yet."),
    (
        "overview.last_run",
        "Last run {when}: {trimmed} processes trimmed, working set reduced by {amount}.",
    ),
    (
        "overview.last_dry_run",
        "Last dry run {when}: {targets} processes would have been trimmed.",
    ),
    (
        "plan.title",
        "Trim the working sets of {count} processes ({amount} in total)?",
    ),
    (
        "plan.title_dry",
        "Dry run over {count} processes ({amount} in total)? Nothing will be trimmed.",
    ),
    (
        "plan.empty",
        "No process is eligible for trimming right now.",
    ),
    ("plan.skipped", "{count} skipped: {reason}"),
    (
        "plan.caveat",
        "Trimming moves pages out of working sets. It does not free committed memory, and the processes will page their memory back in when they use it.",
    ),
    ("run.progress", "Trimming {done} of {total}…"),
    ("run.follow_up", "Measuring the follow-up ({point})…"),
    ("run.follow_up_done", "Follow-up complete."),
    ("run.cancelled", "Run cancelled."),
    (
        "run.truncated",
        "Follow-up cut short by a newer run or by closing Reclaim.",
    ),
    ("point.10s", "10 s"),
    ("point.60s", "60 s"),
    ("mode.trim", "Trim"),
    ("mode.dry_run", "Dry run"),
    (
        "history.row",
        "{when} · {mode} · {trimmed} trimmed · {amount}",
    ),
    ("history.empty", "No runs recorded."),
    ("outcome.trimmed", "Trimmed: {before} → {after}"),
    ("outcome.cancelled", "Not attempted"),
    ("failure.access_denied", "Failed: access denied"),
    ("failure.exited", "Failed: the process exited"),
    (
        "failure.identity_changed",
        "Failed: the PID now belongs to another process",
    ),
    ("failure.unsupported", "Failed: not supported"),
    ("failure.os", "Failed: system error {code}"),
    ("failure.internal", "Failed: internal error"),
    ("tray.tooltip", "Reclaim"),
    ("tray.tooltip_available", "Reclaim: {available} available"),
    (
        "settings.invalid_threshold",
        "The size threshold must be a whole number of MiB from 0 to 65536.",
    ),
    ("settings.saved", "Settings saved."),
    (
        "already_running",
        "Reclaim is already running; the open window was brought to the front.",
    ),
];

/// Text for `key`; the key itself if missing (visible in tests, never blank).
pub fn tr(key: &str) -> &str {
    EN.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .unwrap_or(key)
}

/// Text for `key` with `{name}` placeholders replaced.
pub fn trf(key: &str, params: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut s = tr(key).to_owned();
    for (name, v) in params {
        s = s.replace(&format!("{{{name}}}"), &v.to_string());
    }
    s
}

/// Pressure word.
pub fn pressure(p: Pressure) -> &'static str {
    match p {
        Pressure::High => tr("pressure.high"),
        Pressure::Normal => tr("pressure.normal"),
        Pressure::Low => tr("pressure.low"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn keys_unique_and_lookup() {
        let mut seen = HashSet::new();
        for (k, v) in EN {
            assert!(seen.insert(*k), "duplicate {k}");
            assert!(!v.is_empty());
        }
        assert_eq!(tr("missing.key"), "missing.key");
        assert_eq!(
            trf("run.progress", &[("done", &3), ("total", &7)]),
            "Trimming 3 of 7…"
        );
        assert_eq!(pressure(Pressure::High), "high");
    }

    #[test]
    fn every_reason_has_text() {
        use reclaim_core::policy::Eligibility::*;
        for e in [
            SelfProcess,
            SystemCritical,
            Foreground,
            OtherUser,
            RequiresElevation,
            NotAccessible,
            BelowThreshold,
            Eligible,
        ] {
            assert_ne!(tr(e.key()), e.key());
        }
    }
}
