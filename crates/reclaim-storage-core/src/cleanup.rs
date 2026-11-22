//! Cleanup categories, candidates, plans and results (the storage cleaner's domain).
//!
//! Categories are data: each one lists the locations it may touch (a known folder plus
//! a relative pattern), which files inside qualify, how safe removal is, whether the
//! data comes back by itself and how it is removed. Discovery (engine) only ever looks
//! inside these locations, never follows links, and is read-only. Deletion goes through
//! a reviewed [`CleanupPlan`] and the [`crate::guard::Guard`] twice (at planning and,
//! on the final path of the opened file, right before deleting).
//!
//! Locations are chosen from the vendors' documented cache folders and from Roomy's
//! catalogue (MIT, functional reference only; see docs/storage/RESEARCH.md).

use serde::{Deserialize, Serialize};

use crate::entry::ErrorKind;
use crate::guard::{Denied, Folder};

/// How safe removal is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Safety {
    /// Disposable cache or temporary data that the owning program recreates.
    Safe,
    /// Recreated, but at a noticeable cost (re-downloads, shader recompilation,
    /// slower first start) — not selected by default.
    Caution,
    /// May be personal data: every file is the user's decision; never selected by
    /// default and moved to the Recycle Bin, not deleted.
    Review,
}

impl Safety {
    /// Stable id.
    pub fn id(self) -> &'static str {
        match self {
            Safety::Safe => "safe",
            Safety::Caution => "caution",
            Safety::Review => "review",
        }
    }
}

/// Whether the removed data comes back by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Regenerates {
    /// Recreated on demand by the owning program.
    Yes,
    /// Partly (for example caches refill only for what is used again).
    Partly,
    /// Gone for good.
    No,
}

/// How a category is cleaned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Permanent deletion of the candidate files (a cache in the Recycle Bin would
    /// still occupy the disk).
    Delete,
    /// Move the selected files to the Recycle Bin.
    Recycle,
    /// Empty the Recycle Bin through the shell (`SHEmptyRecycleBinW`).
    EmptyRecycleBin,
    /// Not deleted by Reclaim: Windows' own cleanup tool is opened instead (it handles
    /// elevation and knows which system files are safe).
    WindowsCleanup,
}

/// One place a category looks into.
#[derive(Debug, Clone, Copy)]
pub struct Location {
    /// Base folder.
    pub base: Folder,
    /// Relative path below `base`, `\`-separated; a component may contain `*` and `?`
    /// (matched against directory names, never across links).
    pub sub: &'static str,
    /// Descend into subdirectories.
    pub recurse: bool,
    /// File name patterns that qualify (empty: all files).
    pub include: &'static [&'static str],
    /// Only files not modified for this many hours (a guard against files in use by a
    /// running installer, not a criterion of disposability).
    pub min_age_hours: u32,
    /// Only files at least this large.
    pub min_size: u64,
}

const fn loc(base: Folder, sub: &'static str) -> Location {
    Location {
        base,
        sub,
        recurse: true,
        include: &[],
        min_age_hours: 0,
        min_size: 0,
    }
}

/// A cleanup category.
#[derive(Debug, Clone, Copy)]
pub struct Category {
    /// Stable id (settings, history, CLI).
    pub id: &'static str,
    /// Short name.
    pub name: &'static str,
    /// What the data is.
    pub description: &'static str,
    /// What happens after cleaning.
    pub consequence: &'static str,
    /// Safety.
    pub safety: Safety,
    /// Whether it comes back.
    pub regenerates: Regenerates,
    /// How it is cleaned.
    pub action: Action,
    /// Reading (and cleaning) needs administrator rights.
    pub needs_admin: bool,
    /// Preselected in the review list.
    pub default_selected: bool,
    /// Where it looks.
    pub locations: &'static [Location],
}

use Folder::{
    Downloads, LocalAppData, Profile, ProgramData, RoamingAppData, SystemDrive, Temp, Windows,
};

const CHROMIUM_PROFILES: &[&str] = &[
    "Google\\Chrome\\User Data",
    "Microsoft\\Edge\\User Data",
    "BraveSoftware\\Brave-Browser\\User Data",
    "Vivaldi\\User Data",
    "Chromium\\User Data",
];

macro_rules! chromium {
    ($root:literal) => {
        [
            loc(LocalAppData, concat!($root, "\\Default\\Cache")),
            loc(LocalAppData, concat!($root, "\\Default\\Code Cache")),
            loc(LocalAppData, concat!($root, "\\Default\\GPUCache")),
            loc(LocalAppData, concat!($root, "\\Profile *\\Cache")),
            loc(LocalAppData, concat!($root, "\\Profile *\\Code Cache")),
            loc(LocalAppData, concat!($root, "\\Profile *\\GPUCache")),
            loc(LocalAppData, concat!($root, "\\Guest Profile\\Cache")),
            loc(LocalAppData, concat!($root, "\\ShaderCache")),
            loc(LocalAppData, concat!($root, "\\GrShaderCache")),
            loc(LocalAppData, concat!($root, "\\GraphiteDawnCache")),
        ]
    };
}

macro_rules! electron {
    ($base:expr, $root:literal) => {
        [
            loc($base, concat!($root, "\\Cache")),
            loc($base, concat!($root, "\\Code Cache")),
            loc($base, concat!($root, "\\GPUCache")),
        ]
    };
}

const fn concat3<const A: usize, const B: usize, const C: usize, const N: usize>(
    a: [Location; A],
    b: [Location; B],
    c: [Location; C],
) -> [Location; N] {
    let mut out = [loc(Temp, ""); N];
    let mut i = 0;
    while i < A {
        out[i] = a[i];
        i += 1;
    }
    let mut j = 0;
    while j < B {
        out[A + j] = b[j];
        j += 1;
    }
    let mut k = 0;
    while k < C {
        out[A + B + k] = c[k];
        k += 1;
    }
    out
}

const BROWSERS_A: [Location; 30] = concat3(
    chromium!("Google\\Chrome\\User Data"),
    chromium!("Microsoft\\Edge\\User Data"),
    chromium!("BraveSoftware\\Brave-Browser\\User Data"),
);
const BROWSERS_B: [Location; 23] = concat3(
    chromium!("Vivaldi\\User Data"),
    chromium!("Chromium\\User Data"),
    [
        loc(LocalAppData, "Mozilla\\Firefox\\Profiles\\*\\cache2"),
        loc(LocalAppData, "Opera Software\\Opera Stable\\Cache"),
        loc(LocalAppData, "Opera Software\\Opera GX Stable\\Cache"),
    ],
);
const BROWSERS: [Location; 53] = concat3(BROWSERS_A, BROWSERS_B, []);

const APPS_A: [Location; 15] = concat3(
    electron!(RoamingAppData, "discord"),
    electron!(RoamingAppData, "Slack"),
    concat3::<3, 3, 3, 9>(
        electron!(RoamingAppData, "Microsoft\\Teams"),
        electron!(RoamingAppData, "Code"),
        [
            loc(RoamingAppData, "Code\\CachedData"),
            loc(RoamingAppData, "Code\\CachedExtensionVSIXs"),
            loc(LocalAppData, "Steam\\htmlcache"),
        ],
    ),
);
const APPS: [Location; 22] = concat3(
    APPS_A,
    electron!(
        LocalAppData,
        "Packages\\MSTeams_8wekyb3d8bbwe\\LocalCache\\Microsoft\\MSTeams\\EBWebView\\Default"
    ),
    [
        loc(LocalAppData, "Spotify\\Data"),
        loc(LocalAppData, "EpicGamesLauncher\\Saved\\webcache*"),
        loc(LocalAppData, "Microsoft\\Windows\\INetCache"),
        Location {
            recurse: false,
            include: &["thumbcache_*.db", "iconcache_*.db"],
            ..loc(LocalAppData, "Microsoft\\Windows\\Explorer")
        },
    ],
);

/// The Windows categories, in review order.
pub const WINDOWS_CATEGORIES: &[Category] = &[
    Category {
        id: "user-temp",
        name: "Temporary files (user)",
        description: "Files programs left in your temporary folder.",
        consequence: "Nothing for closed programs. Files changed in the last 24 hours and files in use are kept.",
        safety: Safety::Safe,
        regenerates: Regenerates::Yes,
        action: Action::Delete,
        needs_admin: false,
        default_selected: true,
        locations: &[Location {
            min_age_hours: 24,
            ..loc(Temp, "")
        }],
    },
    Category {
        id: "browser-cache",
        name: "Browser caches",
        description: "Cached web content, compiled scripts and GPU caches of Chrome, Edge, Brave, Vivaldi, Chromium, Firefox and Opera. Cookies, history, passwords and tabs are not touched.",
        consequence: "Sites load a little slower until the cache refills. Close the browser first; files it holds open are skipped.",
        safety: Safety::Safe,
        regenerates: Regenerates::Yes,
        action: Action::Delete,
        needs_admin: false,
        default_selected: true,
        locations: &BROWSERS,
    },
    Category {
        id: "app-cache",
        name: "Application caches",
        description: "Caches of Discord, Slack, Teams, VS Code, Spotify (streaming cache, not offline downloads), Steam and Epic web views, the Internet cache and Explorer's thumbnail cache.",
        consequence: "Apps rebuild these on next start; thumbnails are redrawn. Running apps keep their locked files.",
        safety: Safety::Safe,
        regenerates: Regenerates::Yes,
        action: Action::Delete,
        needs_admin: false,
        default_selected: true,
        locations: &APPS,
    },
    Category {
        id: "dev-cache",
        name: "Developer tool caches",
        description: "Download caches of npm, Yarn, pip, NuGet (HTTP cache), Go build cache and Cargo's downloaded crate archives.",
        consequence: "The next install or build downloads or rebuilds what it needs; offline builds may fail until then. Installed packages and projects are not touched.",
        safety: Safety::Caution,
        regenerates: Regenerates::Yes,
        action: Action::Delete,
        needs_admin: false,
        default_selected: false,
        locations: &[
            loc(LocalAppData, "npm-cache\\_cacache"),
            loc(LocalAppData, "Yarn\\Cache"),
            loc(LocalAppData, "pip\\Cache"),
            loc(LocalAppData, "NuGet\\v3-cache"),
            loc(LocalAppData, "NuGet\\plugins-cache"),
            loc(LocalAppData, "go-build"),
            loc(Profile, ".cargo\\registry\\cache"),
        ],
    },
    Category {
        id: "shader-cache",
        name: "GPU shader caches",
        description: "Compiled shaders kept by DirectX and the NVIDIA, AMD and Intel drivers.",
        consequence: "Games and apps compile shaders again: the first runs can stutter or start slower.",
        safety: Safety::Caution,
        regenerates: Regenerates::Yes,
        action: Action::Delete,
        needs_admin: false,
        default_selected: false,
        locations: &[
            loc(LocalAppData, "D3DSCache"),
            loc(LocalAppData, "NVIDIA\\DXCache"),
            loc(LocalAppData, "NVIDIA\\GLCache"),
            loc(LocalAppData, "NVIDIA Corporation\\NV_Cache"),
            loc(LocalAppData, "AMD\\DxCache"),
            loc(LocalAppData, "AMD\\DxcCache"),
            loc(LocalAppData, "AMD\\GLCache"),
            loc(LocalAppData, "AMD\\VkCache"),
            loc(LocalAppData, "Intel\\ShaderCache"),
        ],
    },
    Category {
        id: "crash-dumps",
        name: "Crash dumps and error reports (user)",
        description: "Memory dumps of crashed programs and Windows Error Reporting files of your account.",
        consequence: "Developers and support can no longer use these reports to diagnose past crashes.",
        safety: Safety::Safe,
        regenerates: Regenerates::No,
        action: Action::Delete,
        needs_admin: false,
        default_selected: true,
        locations: &[
            loc(LocalAppData, "CrashDumps"),
            loc(LocalAppData, "Microsoft\\Windows\\WER\\ReportArchive"),
            loc(LocalAppData, "Microsoft\\Windows\\WER\\ReportQueue"),
        ],
    },
    Category {
        id: "recycle-bin",
        name: "Recycle Bin",
        description: "Files you deleted earlier, on all drives.",
        consequence: "They can no longer be restored.",
        safety: Safety::Review,
        regenerates: Regenerates::No,
        action: Action::EmptyRecycleBin,
        needs_admin: false,
        default_selected: false,
        locations: &[],
    },
    Category {
        id: "old-installers",
        name: "Old installers in Downloads",
        description: "Setup programs (.exe, .msi, .msix, .appx) directly in Downloads, unchanged for 30 days. Old is not the same as useless: check each one.",
        consequence: "Moved to the Recycle Bin; space is freed when the bin is emptied. You may need to download an installer again.",
        safety: Safety::Review,
        regenerates: Regenerates::No,
        action: Action::Recycle,
        needs_admin: false,
        default_selected: false,
        locations: &[Location {
            recurse: false,
            include: &[
                "*.exe",
                "*.msi",
                "*.msix",
                "*.msixbundle",
                "*.appx",
                "*.appxbundle",
            ],
            min_age_hours: 30 * 24,
            ..loc(Downloads, "")
        }],
    },
    Category {
        id: "large-downloads",
        name: "Large old downloads",
        description: "Files of 250 MiB or more in Downloads, unchanged for 90 days. Listed for review only: size and age say nothing about value.",
        consequence: "Moved to the Recycle Bin; space is freed when the bin is emptied.",
        safety: Safety::Review,
        regenerates: Regenerates::No,
        action: Action::Recycle,
        needs_admin: false,
        default_selected: false,
        locations: &[Location {
            min_age_hours: 90 * 24,
            min_size: 250 << 20,
            ..loc(Downloads, "")
        }],
    },
    Category {
        id: "windows-system",
        name: "Windows system files",
        description: "Windows Update downloads, Delivery Optimization cache, the system temporary folder, system crash dumps and error reports, and a previous Windows installation (Windows.old).",
        consequence: "Handled by Windows Disk Cleanup, which knows which update files are still needed and asks for administrator rights itself. Reclaim only measures these folders where it can read them.",
        safety: Safety::Caution,
        regenerates: Regenerates::Partly,
        action: Action::WindowsCleanup,
        needs_admin: true,
        default_selected: false,
        locations: &[
            loc(Windows, "SoftwareDistribution\\Download"),
            loc(
                Windows,
                "ServiceProfiles\\NetworkService\\AppData\\Local\\Microsoft\\Windows\\DeliveryOptimization\\Cache",
            ),
            loc(Windows, "Temp"),
            loc(Windows, "Minidump"),
            loc(ProgramData, "Microsoft\\Windows\\WER\\ReportArchive"),
            loc(ProgramData, "Microsoft\\Windows\\WER\\ReportQueue"),
            loc(SystemDrive, "Windows.old"),
        ],
    },
];

/// Looks a category up by id.
pub fn category(id: &str) -> Option<&'static Category> {
    WINDOWS_CATEGORIES.iter().find(|c| c.id == id)
}

/// Chromium-family profile roots (for "close the browser" hints).
pub fn chromium_roots() -> &'static [&'static str] {
    CHROMIUM_PROFILES
}

/// A file a category would remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Full path.
    pub path: String,
    /// Logical bytes.
    pub logical: u64,
    /// Allocated bytes (the estimate of what deleting frees).
    pub allocated: u64,
    /// Identity at discovery, checked again before deleting.
    pub file_id: Option<u64>,
    /// Last write, Unix seconds.
    pub mtime: i64,
    /// Has other hard links: deleting this name frees nothing.
    pub hardlinked: bool,
    /// Index of the location root (in [`CategoryScan::roots`]) it was found under.
    pub root: usize,
}

impl Candidate {
    /// Bytes deleting it is expected to free.
    pub fn reclaimable(&self) -> u64 {
        if self.hardlinked { 0 } else { self.allocated }
    }
}

/// One resolved location of a category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundRoot {
    /// Full path of the folder.
    pub path: String,
    /// Why it could not be (fully) read, if so.
    pub error: Option<ErrorKind>,
}

/// Discovery result of one category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryScan {
    /// Category id.
    pub id: String,
    /// Locations that exist on this computer.
    pub roots: Vec<FoundRoot>,
    /// Files that qualify.
    pub candidates: Vec<Candidate>,
    /// Links and reparse points found and left alone.
    pub skipped_links: u64,
    /// Files kept because they are too new (age guard).
    pub too_new: u64,
    /// Directories that could not be read.
    pub unreadable: u64,
    /// Recycle Bin: items and bytes reported by the shell.
    pub bin: Option<(u64, u64)>,
}

impl CategoryScan {
    /// Number of candidate files.
    pub fn files(&self) -> u64 {
        self.bin.map_or(self.candidates.len() as u64, |b| b.0)
    }

    /// Estimated reclaimable bytes (allocated, hard-linked files excluded). For the
    /// Recycle Bin: the size the shell reports.
    pub fn estimate(&self) -> u64 {
        if let Some((_, b)) = self.bin {
            return b;
        }
        self.candidates.iter().map(Candidate::reclaimable).sum()
    }

    /// Logical bytes of the candidates.
    pub fn logical(&self) -> u64 {
        self.candidates.iter().map(|c| c.logical).sum()
    }

    /// Whether the scan is incomplete because a location could not be read.
    pub fn incomplete(&self) -> bool {
        self.unreadable > 0 || self.roots.iter().any(|r| r.error.is_some())
    }
}

/// Why a candidate was not removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipReason {
    /// Open in another program.
    InUse,
    /// Changed since discovery (identity, type or location).
    Changed,
    /// Already gone.
    Missing,
    /// A link or reparse point (on the path or the file itself).
    Link,
    /// Refused by the safety policy.
    Protected(String),
    /// Not attempted: the run was cancelled.
    Cancelled,
}

impl SkipReason {
    /// Maps a guard refusal.
    pub fn from_denied(d: &Denied) -> Self {
        SkipReason::Protected(d.to_string())
    }
}

/// What happened to one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemOutcome {
    /// Deleted permanently; allocated bytes it had.
    Deleted(u64),
    /// Moved to the Recycle Bin; allocated bytes it had.
    Recycled(u64),
    /// Left alone.
    Skipped(SkipReason),
    /// The operating system refused.
    Failed(ErrorKind),
}

/// Totals of a run, per category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryResult {
    /// Category id.
    pub id: String,
    /// Files deleted.
    pub deleted: u64,
    /// Bytes (allocated) of deleted files.
    pub deleted_bytes: u64,
    /// Files moved to the Recycle Bin.
    pub recycled: u64,
    /// Bytes of recycled files (not freed until the bin is emptied).
    pub recycled_bytes: u64,
    /// Files skipped, with the reason of the first few.
    pub skipped: u64,
    /// Files that failed.
    pub failed: u64,
    /// Empty directories removed below the location roots.
    pub dirs_removed: u64,
    /// First reasons for skips and failures (bounded), as `reason: path`.
    pub notes: Vec<String>,
}

/// Bounded number of notes per category.
pub const MAX_NOTES: usize = 20;

impl CategoryResult {
    /// Adds an outcome.
    pub fn add(&mut self, path: &str, o: &ItemOutcome) {
        match o {
            ItemOutcome::Deleted(b) => {
                self.deleted += 1;
                self.deleted_bytes += b;
            }
            ItemOutcome::Recycled(b) => {
                self.recycled += 1;
                self.recycled_bytes += b;
            }
            ItemOutcome::Skipped(r) => {
                self.skipped += 1;
                if self.notes.len() < MAX_NOTES {
                    self.notes.push(format!("skipped ({r:?}): {path}"));
                }
            }
            ItemOutcome::Failed(k) => {
                self.failed += 1;
                if self.notes.len() < MAX_NOTES {
                    self.notes.push(format!("failed ({}): {path}", k.id()));
                }
            }
        }
    }
}

/// Free space of a volume before and after a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeDelta {
    /// Volume root.
    pub volume: String,
    /// Free bytes before.
    pub free_before: u64,
    /// Free bytes after.
    pub free_after: u64,
}

impl VolumeDelta {
    /// Measured change (other programs writing at the same time are included).
    pub fn change(&self) -> i128 {
        i128::from(self.free_after) - i128::from(self.free_before)
    }
}

/// Record of one cleanup run (also stored in the local history, without paths).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupReport {
    /// Unix milliseconds at start.
    pub started_ms: u64,
    /// Unix milliseconds at end.
    pub finished_ms: u64,
    /// Estimated reclaimable bytes of the plan.
    pub estimated: u64,
    /// Per-category results.
    pub categories: Vec<CategoryResult>,
    /// Measured free space change per affected volume.
    pub volumes: Vec<VolumeDelta>,
    /// Cancelled before the end.
    pub cancelled: bool,
    /// Recycle Bin emptied (items, bytes as reported before emptying).
    pub bin_emptied: Option<(u64, u64)>,
    /// Windows cleanup tool opened.
    pub windows_cleanup_opened: bool,
}

impl CleanupReport {
    /// Bytes deleted permanently (allocated sizes).
    pub fn deleted_bytes(&self) -> u64 {
        self.categories.iter().map(|c| c.deleted_bytes).sum::<u64>()
            + self.bin_emptied.map_or(0, |b| b.1)
    }

    /// Bytes moved to the Recycle Bin.
    pub fn recycled_bytes(&self) -> u64 {
        self.categories.iter().map(|c| c.recycled_bytes).sum()
    }

    /// Measured free-space change over all volumes.
    pub fn measured(&self) -> i128 {
        self.volumes.iter().map(VolumeDelta::change).sum()
    }

    /// The same record without notes (which contain paths), for the history file.
    pub fn for_history(&self) -> Self {
        let mut r = self.clone();
        for c in &mut r.categories {
            c.notes.clear();
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_consistent() {
        let mut ids = std::collections::HashSet::new();
        for c in WINDOWS_CATEGORIES {
            assert!(ids.insert(c.id), "duplicate id {}", c.id);
            // Review means user data: never preselected, never deleted permanently.
            if c.safety == Safety::Review {
                assert!(!c.default_selected, "{}", c.id);
                assert_ne!(c.action, Action::Delete, "{}", c.id);
            }
            if c.safety == Safety::Caution {
                assert!(!c.default_selected, "{}", c.id);
            }
            if c.needs_admin {
                assert_eq!(c.action, Action::WindowsCleanup, "{}", c.id);
            }
            for l in c.locations {
                assert!(!l.sub.contains(".."), "{}", c.id);
                assert!(!l.sub.contains('/'), "{}", c.id);
                // No rule may target a whole known folder except temp and Downloads
                // (whose contents are filtered).
                if l.sub.is_empty() {
                    assert!(
                        matches!(l.base, Folder::Temp | Folder::Downloads),
                        "{}",
                        c.id
                    );
                }
            }
        }
        assert_eq!(BROWSERS.len(), 53);
        assert!(BROWSERS.iter().all(|l| !l.sub.is_empty()));
        assert!(APPS.iter().all(|l| !l.sub.is_empty()));
        assert_eq!(category("user-temp").unwrap().safety, Safety::Safe);
        assert!(category("nope").is_none());
        assert_eq!(chromium_roots().len(), 5);
        assert_eq!(Safety::Review.id(), "review");
    }

    #[test]
    fn estimates_and_results() {
        let c = |a, h| Candidate {
            path: "x".into(),
            logical: a,
            allocated: a,
            file_id: None,
            mtime: 0,
            hardlinked: h,
            root: 0,
        };
        let s = CategoryScan {
            id: "t".into(),
            candidates: vec![c(10, false), c(20, true)],
            ..CategoryScan::default()
        };
        assert_eq!(s.estimate(), 10);
        assert_eq!(s.logical(), 30);
        assert_eq!(s.files(), 2);
        assert!(!s.incomplete());
        let bin = CategoryScan {
            bin: Some((3, 300)),
            ..CategoryScan::default()
        };
        assert_eq!((bin.files(), bin.estimate()), (3, 300));

        let mut r = CategoryResult::default();
        r.add("a", &ItemOutcome::Deleted(5));
        r.add("b", &ItemOutcome::Recycled(7));
        r.add("c", &ItemOutcome::Skipped(SkipReason::InUse));
        r.add("d", &ItemOutcome::Failed(ErrorKind::AccessDenied));
        assert_eq!((r.deleted, r.recycled, r.skipped, r.failed), (1, 1, 1, 1));
        assert_eq!(r.notes.len(), 2);
        let rep = CleanupReport {
            categories: vec![r],
            volumes: vec![VolumeDelta {
                volume: "C:\\".into(),
                free_before: 100,
                free_after: 90,
            }],
            bin_emptied: Some((1, 50)),
            ..CleanupReport::default()
        };
        assert_eq!(rep.deleted_bytes(), 55);
        assert_eq!(rep.recycled_bytes(), 7);
        assert_eq!(rep.measured(), -10);
        assert!(rep.for_history().categories[0].notes.is_empty());
        assert!(matches!(
            SkipReason::from_denied(&Denied::VolumeRoot),
            SkipReason::Protected(_)
        ));
    }
}
