//! Domain model: process identity, memory snapshots, system metrics and capabilities
//! (ARCHITECTURE §5.1–5.3).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::units::{Bytes, Count};

/// Identity of a process: a PID alone is never used to act (ARCHITECTURE §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProcessKey {
    /// Process id at the time of observation.
    pub pid: u32,
    /// Creation time in the platform's native unit (FILETIME on Windows).
    pub start_time: u64,
}

impl ProcessKey {
    /// Creates a key.
    pub const fn new(pid: u32, start_time: u64) -> Self {
        Self { pid, start_time }
    }
}

impl fmt::Display for ProcessKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PID {}", self.pid)
    }
}

/// An executable image name such as `explorer.exe`. Compared case-insensitively.
#[derive(Debug, Clone, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImageName(String);

impl ImageName {
    /// Creates an image name. Any directory part is removed so paths are never stored.
    pub fn new(name: &str) -> Self {
        let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
        Self(base.to_owned())
    }

    /// The name as given (original case).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Case-insensitive comparison with a plain string.
    pub fn matches(&self, other: &str) -> bool {
        self.0.eq_ignore_ascii_case(other)
    }
}

impl PartialEq for ImageName {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl std::hash::Hash for ImageName {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for b in self.0.bytes() {
            state.write_u8(b.to_ascii_lowercase());
        }
    }
}

impl fmt::Display for ImageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where an image lives, as far as policy needs to know (no path is stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ImageLocation {
    /// Inside the OS directory (`%SystemRoot%` on Windows).
    System,
    /// Anywhere else.
    Other,
    /// Could not be determined (for example access denied).
    #[default]
    Unknown,
}

/// Predicted privilege relation between Reclaim and the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum AccessHint {
    /// Same user and same or lower integrity: trimming is expected to work.
    Accessible,
    /// Runs at a higher integrity level (elevated) than Reclaim.
    RequiresElevation,
    /// Belongs to another user or session.
    OtherUser,
    /// Could not be determined; the open attempt decides.
    #[default]
    Unknown,
}

/// Memory counters of one process at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MemorySnapshot {
    /// Physical pages currently mapped (shared pages included).
    pub working_set: Bytes,
    /// Private part of the working set, when the platform provides it.
    pub private_working_set: Option<Bytes>,
    /// Private committed memory. Trimming does not change it.
    pub private_bytes: Bytes,
    /// Cumulative soft and hard faults.
    pub page_faults: Count,
    /// Cumulative bytes read and written, when available.
    pub io_bytes: Option<Bytes>,
}

/// Everything policy and the UI need to know about one process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Identity.
    pub key: ProcessKey,
    /// Image name without path.
    pub image: ImageName,
    /// Location class of the image.
    pub location: ImageLocation,
    /// Terminal-services session id.
    pub session_id: u32,
    /// Predicted access.
    pub access: AccessHint,
    /// `Some(true)` if the OS marks the process critical; `None` if unknown.
    pub critical: Option<bool>,
    /// Memory counters; `None` if the process could not be queried.
    pub memory: Option<MemorySnapshot>,
}

/// Coarse memory pressure as signalled by the OS (or derived).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Pressure {
    /// The OS low-memory signal is set.
    High,
    /// Neither signal is set.
    #[default]
    Normal,
    /// The OS high-memory (plenty) signal is set.
    Low,
}

/// System-wide memory metrics (ARCHITECTURE §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SystemSnapshot {
    /// Installed physical memory visible to the OS.
    pub phys_total: Bytes,
    /// Free + zeroed + standby pages.
    pub phys_available: Bytes,
    /// Committed virtual memory.
    pub commit_charge: Bytes,
    /// RAM + page files.
    pub commit_limit: Bytes,
    /// Standby (cached) pages, when the optional breakdown is loaded.
    pub standby: Option<Bytes>,
    /// Modified pages, when the optional breakdown is loaded.
    pub modified: Option<Bytes>,
    /// Pages read from disk per second (system-wide), when measured.
    pub pages_input_per_sec: Option<u64>,
    /// OS pressure signal.
    pub pressure: Pressure,
}

impl SystemSnapshot {
    /// Used physical memory in percent (0–100).
    pub fn load_percent(&self) -> u8 {
        if self.phys_total.get() == 0 {
            return 0;
        }
        let used = self.phys_total.saturating_sub(self.phys_available).get() as u128;
        ((used * 100) / u128::from(self.phys_total.get())).min(100) as u8
    }
}

/// Whether a capability can be used in this session (ARCHITECTURE §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    /// Usable.
    Available,
    /// Only with administrator rights, which Reclaim does not request.
    RequiresElevation,
    /// Not supported on this system.
    Unsupported {
        /// Short reason for the UI.
        reason: String,
    },
}

impl Availability {
    /// True if the capability can be used.
    pub fn is_available(&self) -> bool {
        matches!(self, Availability::Available)
    }
}

/// Capabilities of the platform adapter, discovered at run time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Removing pages from another process's working set.
    pub trim_working_set: Availability,
    /// Private working set per process.
    pub private_working_set: Availability,
    /// Standby / modified page breakdown.
    pub standby_breakdown: Availability,
    /// System paging-input rate.
    pub paging_io_rate: Availability,
    /// OS memory-pressure signal.
    pub pressure_signal: Availability,
    /// Per-process I/O counters.
    pub per_process_io: Availability,
}

impl Capabilities {
    /// Everything unsupported (used before discovery and on platforms without an adapter).
    pub fn none(reason: &str) -> Self {
        let u = || Availability::Unsupported {
            reason: reason.to_owned(),
        };
        Self {
            trim_working_set: u(),
            private_working_set: u(),
            standby_breakdown: u(),
            paging_io_rate: u(),
            pressure_signal: u(),
            per_process_io: u(),
        }
    }

    /// Everything available (fake platform, tests).
    pub fn all() -> Self {
        Self {
            trim_working_set: Availability::Available,
            private_working_set: Availability::Available,
            standby_breakdown: Availability::Available,
            paging_io_rate: Availability::Available,
            pressure_signal: Availability::Available,
            per_process_io: Availability::Available,
        }
    }
}

/// Stable metric identifiers shared by UI, reports and tests (ARCHITECTURE §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetricId {
    /// Installed memory.
    PhysTotal,
    /// Available memory.
    PhysAvailable,
    /// Committed memory.
    CommitCharge,
    /// Commit limit.
    CommitLimit,
    /// Standby cache.
    Standby,
    /// Modified list.
    Modified,
    /// Pressure signal.
    Pressure,
    /// Working set.
    WorkingSet,
    /// Private working set.
    PrivateWorkingSet,
    /// Private bytes.
    PrivateBytes,
    /// Page faults.
    PageFaults,
    /// Per-process I/O.
    Io,
    /// System paging input.
    HardFaultsRate,
}

impl MetricId {
    /// All metrics, in glossary order.
    pub const ALL: [MetricId; 13] = [
        MetricId::PhysTotal,
        MetricId::PhysAvailable,
        MetricId::CommitCharge,
        MetricId::CommitLimit,
        MetricId::Standby,
        MetricId::Modified,
        MetricId::Pressure,
        MetricId::WorkingSet,
        MetricId::PrivateWorkingSet,
        MetricId::PrivateBytes,
        MetricId::PageFaults,
        MetricId::Io,
        MetricId::HardFaultsRate,
    ];

    /// Stable id string, also the prefix of its text keys.
    pub const fn id(self) -> &'static str {
        match self {
            MetricId::PhysTotal => "sys.phys_total",
            MetricId::PhysAvailable => "sys.phys_available",
            MetricId::CommitCharge => "sys.commit_charge",
            MetricId::CommitLimit => "sys.commit_limit",
            MetricId::Standby => "sys.standby",
            MetricId::Modified => "sys.modified",
            MetricId::Pressure => "sys.pressure",
            MetricId::WorkingSet => "proc.working_set",
            MetricId::PrivateWorkingSet => "proc.private_ws",
            MetricId::PrivateBytes => "proc.private_bytes",
            MetricId::PageFaults => "proc.page_faults",
            MetricId::Io => "proc.io",
            MetricId::HardFaultsRate => "sys.hard_faults_rate",
        }
    }

    /// English label (the UI looks up `<id>.label` for other languages).
    pub const fn label(self) -> &'static str {
        match self {
            MetricId::PhysTotal => "Installed memory",
            MetricId::PhysAvailable => "Available",
            MetricId::CommitCharge => "Committed",
            MetricId::CommitLimit => "Commit limit",
            MetricId::Standby => "Standby (cached)",
            MetricId::Modified => "Modified",
            MetricId::Pressure => "Memory pressure",
            MetricId::WorkingSet => "Working set",
            MetricId::PrivateWorkingSet => "Private working set",
            MetricId::PrivateBytes => "Private bytes (commit)",
            MetricId::PageFaults => "Page faults",
            MetricId::Io => "I/O",
            MetricId::HardFaultsRate => "Pages read from disk /s",
        }
    }

    /// One-sentence English explanation shown next to the metric.
    pub const fn explanation(self) -> &'static str {
        match self {
            MetricId::PhysTotal => "Physical memory visible to Windows.",
            MetricId::PhysAvailable => {
                "Memory that can be given to programs right away, including the standby cache."
            }
            MetricId::CommitCharge => {
                "Memory promised to programs and the system; trimming working sets does not reduce it."
            }
            MetricId::CommitLimit => {
                "Physical memory plus page files: the most that can be committed."
            }
            MetricId::Standby => {
                "Cached pages that Windows can reuse at once; they count as available."
            }
            MetricId::Modified => {
                "Changed pages waiting to be written to disk; trimming can increase this."
            }
            MetricId::Pressure => "Windows' own low-memory signal.",
            MetricId::WorkingSet => {
                "Physical memory currently mapped for the process, shared pages included."
            }
            MetricId::PrivateWorkingSet => {
                "The part of the working set not shared with other processes."
            }
            MetricId::PrivateBytes => {
                "Memory the process has committed for itself; trimming does not reduce it."
            }
            MetricId::PageFaults => {
                "How often the process touched a page that was not in its working set."
            }
            MetricId::Io => "Bytes the process has read and written.",
            MetricId::HardFaultsRate => {
                "Pages Windows read back from disk per second, a cost of trimming."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn image_names_are_case_insensitive_and_path_free() {
        let a = ImageName::new(r"C:\Windows\explorer.exe");
        assert_eq!(a.as_str(), "explorer.exe");
        assert_eq!(a, ImageName::new("EXPLORER.EXE"));
        assert!(a.matches("Explorer.exe"));
        let set: HashSet<_> = [a.clone(), ImageName::new("Explorer.EXE")].into();
        assert_eq!(set.len(), 1);
        assert_eq!(ImageName::new("/usr/bin/foo").to_string(), "foo");
    }

    #[test]
    fn load_percent() {
        let s = SystemSnapshot {
            phys_total: Bytes::from_mib(1000),
            phys_available: Bytes::from_mib(250),
            ..Default::default()
        };
        assert_eq!(s.load_percent(), 75);
        assert_eq!(SystemSnapshot::default().load_percent(), 0);
    }

    #[test]
    fn metric_glossary_is_complete_and_unique() {
        let ids: HashSet<_> = MetricId::ALL.iter().map(|m| m.id()).collect();
        assert_eq!(ids.len(), MetricId::ALL.len());
        for m in MetricId::ALL {
            assert!(!m.label().is_empty());
            assert!(m.explanation().ends_with('.'));
        }
        // The two metrics trimming does not change must say so (WS ≠ commit).
        assert!(
            MetricId::CommitCharge
                .explanation()
                .contains("does not reduce")
        );
        assert!(
            MetricId::PrivateBytes
                .explanation()
                .contains("does not reduce")
        );
    }

    #[test]
    fn capabilities_helpers() {
        assert!(Capabilities::all().trim_working_set.is_available());
        let none = Capabilities::none("no adapter");
        assert!(!none.per_process_io.is_available());
        assert_eq!(
            none.trim_working_set,
            Availability::Unsupported {
                reason: "no adapter".into()
            }
        );
        assert_eq!(ProcessKey::new(7, 1).to_string(), "PID 7");
    }
}
