//! Protection lists, user exclusions and the eligibility evaluator
//! (ARCHITECTURE §5.4, OS-CAPABILITIES §4).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::model::{AccessHint, ImageLocation, ImageName, ProcessInfo, ProcessKey};
use crate::units::Bytes;

/// Version of the built-in lists; stored in run records so results stay interpretable.
pub const LISTS_VERSION: u32 = 1;

/// Pseudo-process ids that are never touched.
pub const SYSTEM_PIDS: [u32; 2] = [0, 4];

/// System-critical images: never trimmed, not overridable. The list is authoritative;
/// `IsProcessCritical` misses `lsass.exe` and `winlogon.exe` (spike A4).
pub const SYSTEM_CRITICAL: &[&str] = &[
    "System",
    "Registry",
    "Memory Compression",
    "Secure System",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "LsaIso.exe",
    "fontdrvhost.exe",
    "MsMpEng.exe",
    "NisSrv.exe",
    "MsSense.exe",
    "SecurityHealthService.exe",
];

/// Why a caution entry is on the list (shown to the user).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CautionKind {
    /// Desktop compositor: visible frame drops.
    Compositor,
    /// Audio engine: glitches.
    Audio,
    /// Shell: latency on next use.
    Shell,
    /// Text input: input latency.
    Input,
    /// Session infrastructure.
    Session,
}

/// Caution images: excluded by default, the user may opt in with a warning.
pub const CAUTION: &[(&str, CautionKind)] = &[
    ("dwm.exe", CautionKind::Compositor),
    ("audiodg.exe", CautionKind::Audio),
    ("explorer.exe", CautionKind::Shell),
    ("ShellExperienceHost.exe", CautionKind::Shell),
    ("StartMenuExperienceHost.exe", CautionKind::Shell),
    ("SearchHost.exe", CautionKind::Shell),
    ("ctfmon.exe", CautionKind::Input),
    ("TextInputHost.exe", CautionKind::Input),
    ("sihost.exe", CautionKind::Session),
    ("taskhostw.exe", CautionKind::Session),
    ("RuntimeBroker.exe", CautionKind::Session),
];

/// A user exclusion: never trim processes with this image name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExclusionRule {
    /// Image name, compared case-insensitively. Wildcards are not supported in the MVP.
    pub image: String,
}

impl ExclusionRule {
    /// Creates a rule for an image name.
    pub fn image(name: &str) -> Self {
        Self {
            image: ImageName::new(name).as_str().to_owned(),
        }
    }

    fn matches(&self, image: &ImageName) -> bool {
        image.matches(&self.image)
    }
}

/// User-configurable policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// Processes below this working set are not worth the fault cost.
    pub min_working_set: Bytes,
    /// User exclusions.
    pub exclusions: Vec<ExclusionRule>,
    /// Caution images the user explicitly opted in to (lower-case names).
    pub allowed_caution: BTreeSet<String>,
    /// Include the foreground process (normally a per-run override).
    pub include_foreground: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            min_working_set: Bytes::from_mib(32),
            exclusions: Vec::new(),
            allowed_caution: BTreeSet::new(),
            include_foreground: false,
        }
    }
}

impl Policy {
    /// Opts in to a caution image.
    pub fn allow_caution(&mut self, image: &str) {
        self.allowed_caution.insert(image.to_ascii_lowercase());
    }

    fn caution_allowed(&self, image: &ImageName) -> bool {
        self.allowed_caution
            .contains(&image.as_str().to_ascii_lowercase())
    }
}

/// Facts about the current session needed by the evaluator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Context {
    /// Reclaim's own PID.
    pub self_pid: u32,
    /// Owner of the foreground window, if any.
    pub foreground: Option<ProcessKey>,
}

/// Result of evaluating one process. Every variant except `Eligible` is a skip reason.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Eligibility {
    /// Reclaim itself.
    SelfProcess,
    /// Built-in system-critical list, PID 0/4, or OS critical flag (or unknown flag).
    SystemCritical,
    /// Matched a user exclusion.
    UserExcluded(ExclusionRule),
    /// On the caution list and not opted in.
    Protected(CautionKind),
    /// Owns the foreground window.
    Foreground,
    /// Belongs to another user or session.
    OtherUser,
    /// Runs elevated; Reclaim does not request administrator rights.
    RequiresElevation,
    /// Memory counters could not be read.
    NotAccessible,
    /// Working set under the threshold.
    BelowThreshold,
    /// May be trimmed.
    Eligible,
}

impl Eligibility {
    /// True for `Eligible`.
    pub fn is_eligible(&self) -> bool {
        matches!(self, Eligibility::Eligible)
    }

    /// Stable text key of the reason.
    pub fn key(&self) -> &'static str {
        match self {
            Eligibility::SelfProcess => "reason.self",
            Eligibility::SystemCritical => "reason.system_critical",
            Eligibility::UserExcluded(_) => "reason.user_excluded",
            Eligibility::Protected(_) => "reason.protected",
            Eligibility::Foreground => "reason.foreground",
            Eligibility::OtherUser => "reason.other_user",
            Eligibility::RequiresElevation => "reason.requires_elevation",
            Eligibility::NotAccessible => "reason.not_accessible",
            Eligibility::BelowThreshold => "reason.below_threshold",
            Eligibility::Eligible => "reason.eligible",
        }
    }

    /// Whether the user can override this reason from the UI.
    pub fn overridable(&self) -> bool {
        matches!(
            self,
            Eligibility::UserExcluded(_)
                | Eligibility::Protected(_)
                | Eligibility::Foreground
                | Eligibility::BelowThreshold
        )
    }
}

/// True if a system-list entry applies to an image at this location.
/// A user binary called `dwm.exe` outside the OS directory is not the compositor; an
/// unknown location is treated as the system one (protective default).
fn system_location(location: ImageLocation) -> bool {
    matches!(location, ImageLocation::System | ImageLocation::Unknown)
}

fn is_system_critical(p: &ProcessInfo) -> bool {
    if SYSTEM_PIDS.contains(&p.key.pid) {
        return true;
    }
    if p.critical != Some(false) {
        return true;
    }
    system_location(p.location) && SYSTEM_CRITICAL.iter().any(|n| p.image.matches(n))
}

fn caution_kind(p: &ProcessInfo) -> Option<CautionKind> {
    if !system_location(p.location) {
        return None;
    }
    CAUTION
        .iter()
        .find(|(n, _)| p.image.matches(n))
        .map(|(_, k)| *k)
}

/// Evaluates one process. First match wins (ARCHITECTURE §5.4).
pub fn evaluate(p: &ProcessInfo, policy: &Policy, ctx: &Context) -> Eligibility {
    if p.key.pid == ctx.self_pid {
        return Eligibility::SelfProcess;
    }
    if is_system_critical(p) {
        return Eligibility::SystemCritical;
    }
    if let Some(rule) = policy.exclusions.iter().find(|r| r.matches(&p.image)) {
        return Eligibility::UserExcluded(rule.clone());
    }
    if let Some(kind) = caution_kind(p)
        && !policy.caution_allowed(&p.image)
    {
        return Eligibility::Protected(kind);
    }
    if ctx.foreground == Some(p.key) && !policy.include_foreground {
        return Eligibility::Foreground;
    }
    match p.access {
        AccessHint::OtherUser => return Eligibility::OtherUser,
        AccessHint::RequiresElevation => return Eligibility::RequiresElevation,
        AccessHint::Accessible | AccessHint::Unknown => {}
    }
    let Some(mem) = p.memory else {
        return Eligibility::NotAccessible;
    };
    if mem.working_set < policy.min_working_set {
        return Eligibility::BelowThreshold;
    }
    Eligibility::Eligible
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::MemorySnapshot;

    pub(crate) fn proc_(pid: u32, image: &str, ws_mib: u64) -> ProcessInfo {
        ProcessInfo {
            key: ProcessKey::new(pid, 1000 + u64::from(pid)),
            image: ImageName::new(image),
            location: ImageLocation::Other,
            session_id: 1,
            access: AccessHint::Accessible,
            critical: Some(false),
            memory: Some(MemorySnapshot {
                working_set: Bytes::from_mib(ws_mib),
                private_working_set: Some(Bytes::from_mib(ws_mib / 2)),
                private_bytes: Bytes::from_mib(ws_mib),
                page_faults: 0,
                io_bytes: None,
            }),
        }
    }

    fn ctx() -> Context {
        Context {
            self_pid: 99,
            foreground: None,
        }
    }

    #[test]
    fn rule_order_first_match_wins() {
        let policy = Policy::default();
        // Self beats everything, even a critical name.
        let mut me = proc_(99, "lsass.exe", 100);
        me.location = ImageLocation::System;
        assert_eq!(evaluate(&me, &policy, &ctx()), Eligibility::SelfProcess);

        // System-critical beats a user exclusion.
        let mut lsass = proc_(10, "LSASS.EXE", 100);
        lsass.location = ImageLocation::System;
        let mut p = policy.clone();
        p.exclusions.push(ExclusionRule::image("lsass.exe"));
        assert_eq!(evaluate(&lsass, &p, &ctx()), Eligibility::SystemCritical);

        // User exclusion beats caution.
        let mut dwm = proc_(11, "dwm.exe", 100);
        dwm.location = ImageLocation::System;
        let mut p = policy.clone();
        p.exclusions.push(ExclusionRule::image(r"C:\x\dwm.exe"));
        assert!(matches!(
            evaluate(&dwm, &p, &ctx()),
            Eligibility::UserExcluded(_)
        ));
        assert_eq!(
            evaluate(&dwm, &policy, &ctx()),
            Eligibility::Protected(CautionKind::Compositor)
        );
    }

    #[test]
    fn system_pids_and_critical_flag() {
        let policy = Policy::default();
        for pid in SYSTEM_PIDS {
            assert_eq!(
                evaluate(&proc_(pid, "whatever", 100), &policy, &ctx()),
                Eligibility::SystemCritical
            );
        }
        let mut flagged = proc_(20, "app.exe", 100);
        flagged.critical = Some(true);
        assert_eq!(
            evaluate(&flagged, &policy, &ctx()),
            Eligibility::SystemCritical
        );
        flagged.critical = None;
        assert_eq!(
            evaluate(&flagged, &policy, &ctx()),
            Eligibility::SystemCritical
        );
    }

    #[test]
    fn critical_names_need_system_location() {
        let policy = Policy::default();
        for name in SYSTEM_CRITICAL {
            let mut p = proc_(30, name, 100);
            p.location = ImageLocation::System;
            assert_eq!(
                evaluate(&p, &policy, &ctx()),
                Eligibility::SystemCritical,
                "{name}"
            );
            p.location = ImageLocation::Unknown;
            assert_eq!(
                evaluate(&p, &policy, &ctx()),
                Eligibility::SystemCritical,
                "{name}"
            );
        }
        // A user binary that happens to be called csrss.exe is judged on its own merits.
        let fake = proc_(31, "csrss.exe", 100);
        assert_eq!(evaluate(&fake, &policy, &ctx()), Eligibility::Eligible);
    }

    #[test]
    fn caution_list_and_opt_in() {
        let mut policy = Policy::default();
        for (name, kind) in CAUTION {
            let mut p = proc_(40, name, 100);
            p.location = ImageLocation::System;
            assert_eq!(evaluate(&p, &policy, &ctx()), Eligibility::Protected(*kind));
            let other = proc_(41, name, 100);
            assert_eq!(evaluate(&other, &policy, &ctx()), Eligibility::Eligible);
        }
        let mut explorer = proc_(42, "explorer.exe", 100);
        explorer.location = ImageLocation::System;
        policy.allow_caution("Explorer.exe");
        assert_eq!(evaluate(&explorer, &policy, &ctx()), Eligibility::Eligible);
    }

    #[test]
    fn foreground_access_threshold() {
        let mut policy = Policy::default();
        let p = proc_(50, "game.exe", 100);
        let fg = Context {
            self_pid: 99,
            foreground: Some(p.key),
        };
        assert_eq!(evaluate(&p, &policy, &fg), Eligibility::Foreground);
        policy.include_foreground = true;
        assert_eq!(evaluate(&p, &policy, &fg), Eligibility::Eligible);
        // A reused PID with another start time is not the foreground process.
        let mut reused = p.clone();
        reused.key.start_time += 1;
        policy.include_foreground = false;
        assert_eq!(evaluate(&reused, &policy, &fg), Eligibility::Eligible);

        let mut other = proc_(51, "a.exe", 100);
        other.access = AccessHint::OtherUser;
        assert_eq!(evaluate(&other, &policy, &ctx()), Eligibility::OtherUser);
        other.access = AccessHint::RequiresElevation;
        assert_eq!(
            evaluate(&other, &policy, &ctx()),
            Eligibility::RequiresElevation
        );
        other.access = AccessHint::Unknown;
        assert_eq!(evaluate(&other, &policy, &ctx()), Eligibility::Eligible);
        other.memory = None;
        assert_eq!(
            evaluate(&other, &policy, &ctx()),
            Eligibility::NotAccessible
        );

        let small = proc_(52, "b.exe", 31);
        assert_eq!(
            evaluate(&small, &policy, &ctx()),
            Eligibility::BelowThreshold
        );
        let edge = proc_(53, "c.exe", 32);
        assert_eq!(evaluate(&edge, &policy, &ctx()), Eligibility::Eligible);
    }

    #[test]
    fn reason_keys_and_overrides() {
        let all = [
            Eligibility::SelfProcess,
            Eligibility::SystemCritical,
            Eligibility::UserExcluded(ExclusionRule::image("x")),
            Eligibility::Protected(CautionKind::Audio),
            Eligibility::Foreground,
            Eligibility::OtherUser,
            Eligibility::RequiresElevation,
            Eligibility::NotAccessible,
            Eligibility::BelowThreshold,
            Eligibility::Eligible,
        ];
        let keys: std::collections::HashSet<_> = all.iter().map(Eligibility::key).collect();
        assert_eq!(keys.len(), all.len());
        assert!(!Eligibility::SystemCritical.overridable());
        assert!(!Eligibility::SelfProcess.overridable());
        assert!(Eligibility::Protected(CautionKind::Shell).overridable());
        assert!(Eligibility::Eligible.is_eligible());
        assert!(!Eligibility::Foreground.is_eligible());
    }

    #[test]
    fn lists_have_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for n in SYSTEM_CRITICAL.iter().chain(CAUTION.iter().map(|(n, _)| n)) {
            assert!(seen.insert(n.to_ascii_lowercase()), "duplicate {n}");
        }
        assert_eq!(LISTS_VERSION, 1);
    }
}
