//! Storage view model: disk usage analysis, cleanup and duplicates as plain Rust.
//!
//! [`Storage`] owns the scan index and the state of the three storage screens. Long work
//! (scans, searches, cleanup discovery and execution, duplicate hashing, recycling) runs
//! on short-lived background threads that report through [`Notify`]; the adapter hands
//! each [`StorageEvent`] back to [`Storage::apply`] on the UI thread. Nothing runs until
//! the user asks for it: no scan at startup, no resident threads.
//!
//! Every method returns a [`StorageDirty`] naming the parts of the screen to refresh.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use reclaim_core::units::{Bytes, Millis};
use reclaim_storage_core::cleanup::{
    Action, CategoryScan, CleanupReport, ItemOutcome, Regenerates, Safety, SkipReason,
    WINDOWS_CATEGORIES,
};
use reclaim_storage_core::dupes::{
    Confidence, DupFile, DupGroup, SelectionError, validate_selection,
};
use reclaim_storage_core::entry::{Attrs, Kind};
use reclaim_storage_core::guard::{Guard, KnownFolders};
use reclaim_storage_core::ports::{DriveKind, Fs, Remover, Target, VolumeInfo, Volumes};
use reclaim_storage_core::query::{
    ExtStat, Filter, FilterResult, KindFilter, SortKey, TextMatch, TopKind, extension_stats,
    largest, palette, parse_size, sort_nodes,
};
use reclaim_storage_core::tree::{NodeId, SizeBasis, TOP, Tree};
use reclaim_storage_core::treemap::{Cell, LayoutOptions, Rect, hit, icicle, treemap};
use reclaim_storage_engine::cleanup::{
    CleanupPlan, CleanupProgress, DiscoverOptions, PlanEntry, discover, execute,
};
use reclaim_storage_engine::dupes::{
    DupOptions, DupProgress, DupResult, DupStage, files_from_tree,
};
use reclaim_storage_engine::export::{self, ExportOptions};
use reclaim_storage_engine::scan::{
    self, ScanControl, ScanOptions, ScanProgress, ScanState, ScanSummary, ScanTarget,
    default_threads,
};
use reclaim_storage_engine::store::{StorageSettings, StorageStore};

use crate::format::date_time;

/// How many children of one directory the tree shows before a "more" row.
pub const TREE_CHILD_CAP: usize = 300;
/// Rows in the "largest" lists.
pub const TOP_LIMIT: usize = 200;
/// Rows in search results.
pub const SEARCH_LIMIT: usize = 1000;
/// Treemap tiles drawn at most (software renderer budget).
pub const MAX_TILES: usize = 2500;
/// Colour slot of a directory frame in the treemap.
pub const SLOT_DIR: u8 = 15;
/// Colour slot of a merged "rest" tile.
pub const SLOT_REST: u8 = 16;

/// Statistics a fast (whole-volume) scan reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FastStats {
    /// Files indexed.
    pub files: u64,
    /// Directories indexed.
    pub dirs: u64,
    /// Duration.
    pub elapsed_ms: u64,
    /// Stopped by the user.
    pub cancelled: bool,
}

/// Fast whole-volume scan: `(volume root, tree, control, progress(files))`. An error is
/// the reason it cannot run; the caller then falls back to the normal scan.
pub type FastScan = Arc<
    dyn Fn(&str, &RwLock<Tree>, &ScanControl, &dyn Fn(u64)) -> Result<FastStats, String>
        + Send
        + Sync,
>;

/// Event sink, called from background threads.
pub type Notify = Arc<dyn Fn(StorageEvent) + Send + Sync>;

/// Clock in Unix milliseconds (injected so tests are deterministic).
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Operating-system services the storage screens use.
#[derive(Clone)]
pub struct StoragePlatform {
    /// File system reader.
    pub fs: Arc<dyn Fs>,
    /// Volume information.
    pub volumes: Arc<dyn Volumes>,
    /// Deletion, Recycle Bin and system cleanup.
    pub remover: Arc<dyn Remover>,
    /// Known folders of the current user.
    pub folders: KnownFolders,
    /// Protected-path policy.
    pub guard: Arc<Guard>,
    /// Fast whole-volume scan, when the platform has one.
    pub fast_scan: Option<FastScan>,
    /// Whether the process runs elevated (the fast scan needs it).
    pub elevated: bool,
    /// Settings and history store (`None`: nothing persisted).
    pub store: Option<StorageStore>,
    /// Current time.
    pub clock: Clock,
}

impl std::fmt::Debug for StoragePlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoragePlatform")
            .field("elevated", &self.elevated)
            .field("fast_scan", &self.fast_scan.is_some())
            .finish_non_exhaustive()
    }
}

/// Results computed off the UI thread after a scan or a change to the index.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    /// Extension statistics, largest first.
    pub ext: Vec<ExtStat>,
    /// Colour slot per extension id.
    pub palette: Vec<u8>,
    /// Largest files.
    pub files: Vec<NodeId>,
    /// Largest directories.
    pub dirs: Vec<NodeId>,
}

impl Analysis {
    fn of(t: &Tree, basis: SizeBasis) -> Self {
        let ext = extension_stats(t, TOP, basis);
        let palette = palette(&ext, t.extensions().len());
        Self {
            files: largest(t, TOP, TopKind::Files, basis, TOP_LIMIT),
            dirs: largest(t, TOP, TopKind::Dirs, basis, TOP_LIMIT),
            ext,
            palette,
        }
    }
}

/// Something a background job reports.
#[derive(Debug)]
pub enum StorageEvent {
    /// Scan counters.
    ScanProgress {
        /// Job number.
        job: u64,
        /// Counters.
        progress: ScanProgress,
    },
    /// A scan or refresh ended.
    ScanFinished {
        /// Job number.
        job: u64,
        /// Outcome.
        summary: ScanSummary,
        /// Derived lists.
        analysis: Analysis,
        /// Whether the fast NTFS scan produced the index.
        fast: bool,
        /// Why the fast scan was not used, if it was attempted.
        note: Option<String>,
    },
    /// Search results.
    SearchDone {
        /// Search generation.
        generation: u64,
        /// Matches.
        result: FilterResult,
    },
    /// One cleanup category was examined.
    CleanupFound(CategoryScan),
    /// Cleanup discovery ended.
    CleanupScanDone {
        /// Stopped by the user.
        cancelled: bool,
    },
    /// Cleanup progress.
    CleanupProgress(CleanupProgress),
    /// Cleanup ended.
    CleanupDone(CleanupReport),
    /// Duplicate search progress.
    DupProgress(DupProgress),
    /// Duplicate search ended.
    DupDone(DupResult),
    /// An export ended: the file written, or why it failed.
    ExportDone(Result<String, String>),
    /// Files were moved to the Recycle Bin from the analyzer or the duplicate list.
    RemovalDone {
        /// What happened to each path.
        outcomes: Vec<(String, ItemOutcome)>,
        /// Derived lists after removing the recycled entries from the index.
        analysis: Analysis,
    },
}

/// Parts of the storage screens to refresh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageDirty {
    /// Drive list.
    pub drives: bool,
    /// Scan status line and buttons.
    pub status: bool,
    /// Directory tree rows.
    pub tree: bool,
    /// Treemap tiles.
    pub map: bool,
    /// Selection details (and the selected row/tile).
    pub selection: bool,
    /// Bottom lists (largest, extensions, search, errors).
    pub lists: bool,
    /// Cleanup screen.
    pub cleanup: bool,
    /// Duplicates screen.
    pub dupes: bool,
    /// Confirmation dialog.
    pub confirm: bool,
    /// Storage settings form.
    pub settings: bool,
}

impl StorageDirty {
    /// Everything.
    pub fn all() -> Self {
        Self {
            drives: true,
            status: true,
            tree: true,
            map: true,
            selection: true,
            lists: true,
            cleanup: true,
            dupes: true,
            confirm: true,
            settings: true,
        }
    }

    /// Whether anything changed.
    pub fn any(&self) -> bool {
        *self != Self::default()
    }

    /// Union.
    pub fn merge(&mut self, o: StorageDirty) {
        self.drives |= o.drives;
        self.status |= o.status;
        self.tree |= o.tree;
        self.map |= o.map;
        self.selection |= o.selection;
        self.lists |= o.lists;
        self.cleanup |= o.cleanup;
        self.dupes |= o.dupes;
        self.confirm |= o.confirm;
        self.settings |= o.settings;
    }

    fn index() -> Self {
        Self {
            status: true,
            tree: true,
            map: true,
            selection: true,
            lists: true,
            dupes: true,
            ..Self::default()
        }
    }
}

/// Visualisation of the selected subtree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MapMode {
    /// Squarified treemap.
    #[default]
    Treemap,
    /// Icicle (top-down flame graph).
    Icicle,
}

/// Bottom panel tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListTab {
    /// Largest files.
    #[default]
    Files,
    /// Largest directories.
    Dirs,
    /// Extensions.
    Extensions,
    /// Search results.
    Search,
    /// Unreadable locations.
    Errors,
}

impl ListTab {
    /// Tabs in display order.
    pub const ALL: [ListTab; 5] = [
        ListTab::Files,
        ListTab::Dirs,
        ListTab::Extensions,
        ListTab::Search,
        ListTab::Errors,
    ];

    /// Tab index.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Tab at an index.
    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }
}

/// Search form, as the user typed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchForm {
    /// Name: text, glob (`*.log`) or `/regex/`.
    pub name: String,
    /// Full path: text, glob or `/regex/`.
    pub path: String,
    /// Extensions separated by spaces or commas.
    pub extensions: String,
    /// Minimum size ("100 MB").
    pub min_size: String,
    /// Maximum size.
    pub max_size: String,
    /// Not modified for at least this many days.
    pub older_than_days: String,
    /// Modified within this many days.
    pub newer_than_days: String,
    /// 0 any, 1 files, 2 directories.
    pub kind: usize,
}

impl SearchForm {
    /// Builds the filter, or says which field is wrong.
    pub fn parse(&self, basis: SizeBasis, now_s: i64) -> Result<Filter, String> {
        let text =
            |label: &str, s: &str| TextMatch::parse(s.trim()).map_err(|e| format!("{label}: {e}"));
        let size = |label: &str, s: &str| -> Result<Option<u64>, String> {
            let s = s.trim();
            if s.is_empty() {
                return Ok(None);
            }
            parse_size(s)
                .map(Some)
                .ok_or_else(|| format!("{label}: not a size (try 100 MB or 2 GiB)"))
        };
        let days = |label: &str, s: &str| -> Result<Option<i64>, String> {
            let s = s.trim();
            if s.is_empty() {
                return Ok(None);
            }
            s.parse::<u32>()
                .map(|d| Some(now_s - i64::from(d) * 86_400))
                .map_err(|_| format!("{label}: enter a whole number of days"))
        };
        Ok(Filter {
            name: text("Name", &self.name)?,
            path: text("Path", &self.path)?,
            extensions: self
                .extensions
                .split([',', ' ', ';'])
                .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                .filter(|e| !e.is_empty())
                .collect(),
            min_size: size("Minimum size", &self.min_size)?,
            max_size: size("Maximum size", &self.max_size)?,
            modified_before: days("Older than", &self.older_than_days)?,
            modified_after: days("Newer than", &self.newer_than_days)?,
            kind: match self.kind {
                1 => KindFilter::Files,
                2 => KindFilter::Dirs,
                _ => KindFilter::Any,
            },
            basis,
        })
    }
}

/// Storage settings as form fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageForm {
    /// Scan exclusions, one per line.
    pub scan_exclusions: String,
    /// Cleanup exclusions, one per line.
    pub cleanup_exclusions: String,
    /// Scan threads (0 = automatic).
    pub scan_threads: String,
    /// Fast NTFS scan when elevated.
    pub fast_ntfs_scan: bool,
    /// Sizes on disk.
    pub allocated_sizes: bool,
    /// Duplicate minimum size in KiB.
    pub duplicate_min_kib: String,
    /// Keep cleanup history.
    pub keep_cleanup_history: bool,
}

impl StorageForm {
    /// Form showing `s`.
    pub fn of(s: &StorageSettings) -> Self {
        Self {
            scan_exclusions: s.scan_exclusions.join("\n"),
            cleanup_exclusions: s.cleanup_exclusions.join("\n"),
            scan_threads: s.scan_threads.to_string(),
            fast_ntfs_scan: s.fast_ntfs_scan,
            allocated_sizes: s.allocated_sizes,
            duplicate_min_kib: s.duplicate_min_kib.to_string(),
            keep_cleanup_history: s.keep_cleanup_history,
        }
    }

    /// Validates the form.
    pub fn parse(&self, base: &StorageSettings) -> Result<StorageSettings, String> {
        let lines = |s: &str| -> Vec<String> {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let threads: usize = self
            .scan_threads
            .trim()
            .parse()
            .ok()
            .filter(|n| *n <= 64)
            .ok_or("Scan threads: enter 0 (automatic) to 64.")?;
        let min_kib: u64 = self
            .duplicate_min_kib
            .trim()
            .parse()
            .map_err(|_| "Duplicate minimum size: enter a whole number of KiB.")?;
        Ok(StorageSettings {
            scan_exclusions: lines(&self.scan_exclusions),
            cleanup_exclusions: lines(&self.cleanup_exclusions),
            scan_threads: threads,
            fast_ntfs_scan: self.fast_ntfs_scan,
            allocated_sizes: self.allocated_sizes,
            duplicate_min_kib: min_kib,
            keep_cleanup_history: self.keep_cleanup_history,
            ..base.clone()
        })
    }
}

/// One drive in the drive list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveView {
    /// Root path.
    pub root: String,
    /// "C:\ System (NTFS)".
    pub title: String,
    /// "120 GiB free of 476 GiB".
    pub detail: String,
    /// Used share, per mille.
    pub used_permille: u16,
}

/// Scan status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusView {
    /// One line.
    pub line: String,
    /// A scan is running.
    pub scanning: bool,
    /// The scan is paused.
    pub paused: bool,
    /// There is an index.
    pub has_index: bool,
    /// Pause is available (not for the fast scan).
    pub can_pause: bool,
    /// The index is not complete (cancelled or unreadable parts).
    pub partial: bool,
}

/// One row of the directory tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRowView {
    /// Node (for a "more" row: the parent).
    pub id: NodeId,
    /// Indentation level.
    pub depth: usize,
    /// Name.
    pub name: String,
    /// Size under the basis.
    pub size: String,
    /// Share of the parent, per mille (bar).
    pub share: u16,
    /// "41.2 %" of parent.
    pub parent_pct: String,
    /// "3.1 %" of everything scanned.
    pub total_pct: String,
    /// Files below.
    pub files: String,
    /// Subdirectories below.
    pub dirs: String,
    /// Last write.
    pub modified: String,
    /// Has children to expand.
    pub expandable: bool,
    /// Expanded.
    pub expanded: bool,
    /// Selected.
    pub selected: bool,
    /// Directory.
    pub is_dir: bool,
    /// Short flags ("unreadable", "excluded", …).
    pub flags: String,
    /// A "N more" row.
    pub more: bool,
    /// Colour slot (files).
    pub slot: u8,
}

/// One treemap tile in map coordinates (pixels of the area given to [`Storage::set_map_size`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileView {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
    /// Colour slot: 0..=14 by extension, [`SLOT_DIR`], [`SLOT_REST`].
    pub slot: u8,
    /// Node.
    pub node: NodeId,
    /// Drawn filled (false: directory frame).
    pub leaf: bool,
}

/// Details of the selected node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionView {
    /// Full path.
    pub path: String,
    /// Several lines: sizes, counts, attributes.
    pub detail: String,
    /// Something is selected.
    pub any: bool,
    /// The selection may be moved to the Recycle Bin.
    pub can_recycle: bool,
    /// The selection is a directory that can be refreshed.
    pub can_refresh: bool,
    /// Zoom path shown above the map.
    pub zoom_path: String,
    /// Back is possible.
    pub can_back: bool,
    /// Forward is possible.
    pub can_forward: bool,
    /// Zoom out is possible.
    pub can_up: bool,
}

/// One row of a bottom list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRowView {
    /// Node (none for extensions and errors).
    pub id: Option<NodeId>,
    /// Name or extension.
    pub name: String,
    /// Path or description.
    pub detail: String,
    /// Size.
    pub size: String,
    /// Third column (modified, file count).
    pub extra: String,
    /// Colour slot.
    pub slot: u8,
}

/// Column the "largest" and search lists are sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListSort {
    /// Size.
    #[default]
    Size,
    /// Path.
    Path,
    /// Extension.
    Extension,
    /// Modified.
    Modified,
}

impl ListSort {
    /// From a column index.
    pub fn from_index(i: usize) -> Self {
        match i {
            1 => ListSort::Path,
            2 => ListSort::Extension,
            3 => ListSort::Modified,
            _ => ListSort::Size,
        }
    }

    /// Column index.
    pub fn index(self) -> usize {
        match self {
            ListSort::Size => 0,
            ListSort::Path => 1,
            ListSort::Extension => 2,
            ListSort::Modified => 3,
        }
    }

    fn key(self) -> SortKey {
        match self {
            ListSort::Size => SortKey::Size,
            ListSort::Path => SortKey::Path,
            ListSort::Extension => SortKey::Extension,
            ListSort::Modified => SortKey::Modified,
        }
    }
}

/// One cleanup category row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryView {
    /// Category id.
    pub id: String,
    /// Name.
    pub name: String,
    /// "Safe", "Caution", "Review each file".
    pub safety: String,
    /// Safety class (0 safe, 1 caution, 2 review).
    pub safety_level: u8,
    /// Estimate or state ("not examined", "examining…").
    pub size: String,
    /// "1 234 files".
    pub files: String,
    /// Selected for cleanup.
    pub selected: bool,
    /// Selectable (examined and something to do).
    pub enabled: bool,
    /// Focused (details shown).
    pub focused: bool,
}

/// Candidate file row of a review category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateView {
    /// Path.
    pub path: String,
    /// Size on disk.
    pub size: String,
    /// Modified.
    pub modified: String,
    /// Selected.
    pub selected: bool,
}

/// Cleanup screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CleanupView {
    /// Categories.
    pub categories: Vec<CategoryView>,
    /// Details of the focused category.
    pub detail: String,
    /// Per-file list of the focused category (review categories only).
    pub candidates: Vec<CandidateView>,
    /// The focused category is reviewed file by file.
    pub per_file: bool,
    /// Status line ("Selected: 2.1 GiB in 3 categories").
    pub status: String,
    /// Discovery or execution running.
    pub busy: bool,
    /// Progress 0..=1 during execution.
    pub progress: f32,
    /// Something is selected and nothing is running.
    pub can_clean: bool,
    /// Report of the last run.
    pub report: String,
    /// Stored runs, newest first.
    pub history: Vec<String>,
}

/// One row of the duplicate list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupRowView {
    /// Group index.
    pub group: usize,
    /// File index (`None`: group header).
    pub file: Option<usize>,
    /// Text.
    pub text: String,
    /// Size or savings.
    pub size: String,
    /// Marked for the Recycle Bin.
    pub marked: bool,
    /// Group header.
    pub header: bool,
}

/// Duplicates screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DupesView {
    /// Rows.
    pub rows: Vec<DupRowView>,
    /// Status line.
    pub status: String,
    /// Running.
    pub busy: bool,
    /// Progress 0..=1.
    pub progress: f32,
    /// A scan index exists to search in.
    pub can_find: bool,
    /// Marked files can be recycled.
    pub can_recycle: bool,
}

/// Confirmation dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmView {
    /// Question.
    pub title: String,
    /// Explanation (what is permanent, what is recycled).
    pub body: String,
    /// Label of the confirming button.
    pub ok: String,
    /// The action deletes permanently.
    pub danger: bool,
}

#[derive(Debug, Clone)]
enum Pending {
    RecycleNodes(Vec<NodeId>),
    Clean,
    RecycleDupes(Vec<DupFile>),
}

#[derive(Debug)]
struct ScanJob {
    job: u64,
    control: Arc<ScanControl>,
    progress: ScanProgress,
    fast: bool,
    refresh: bool,
}

#[derive(Debug, Default)]
struct CleanupState {
    scans: BTreeMap<String, CategoryScan>,
    /// Selected category ids.
    selected: BTreeSet<String>,
    /// Per-file selection of review categories.
    files: BTreeMap<String, BTreeSet<usize>>,
    focused: Option<String>,
    discovering: bool,
    running: Option<CleanupProgress>,
    cancel: Option<Arc<AtomicBool>>,
    report: Option<CleanupReport>,
}

#[derive(Debug, Default)]
struct DupState {
    result: Option<DupResult>,
    marked: BTreeSet<(usize, usize)>,
    progress: Option<DupProgress>,
    cancel: Option<Arc<AtomicBool>>,
    message: String,
}

/// The storage screens' state.
pub struct Storage {
    platform: StoragePlatform,
    notify: Notify,
    settings: StorageSettings,
    utc_offset_min: i32,
    drives: Vec<VolumeInfo>,
    tree: Arc<RwLock<Tree>>,
    has_index: bool,
    scan_roots: Vec<String>,
    job: u64,
    scan: Option<ScanJob>,
    summary: Option<ScanSummary>,
    fast_used: bool,
    note: Option<String>,
    analysis: Analysis,
    stale: bool,
    // Tree view.
    expanded: BTreeSet<NodeId>,
    more: BTreeMap<NodeId, usize>,
    rows: Vec<TreeRowView>,
    selected: Option<NodeId>,
    // Map.
    map_mode: MapMode,
    map_w: f32,
    map_h: f32,
    zoom: NodeId,
    back: Vec<NodeId>,
    forward: Vec<NodeId>,
    cells: Vec<Cell>,
    // Lists.
    tab: ListTab,
    sort: ListSort,
    ascending: bool,
    search_gen: u64,
    search: Option<FilterResult>,
    search_message: String,
    // Other screens.
    cleanup: CleanupState,
    dupes: DupState,
    history: Vec<CleanupReport>,
    pending: Option<(Pending, ConfirmView)>,
    removing: bool,
    message: String,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage")
            .field("has_index", &self.has_index)
            .field("scan_roots", &self.scan_roots)
            .field("selected", &self.selected)
            .finish_non_exhaustive()
    }
}

fn read(t: &RwLock<Tree>) -> std::sync::RwLockReadGuard<'_, Tree> {
    t.read().unwrap_or_else(|p| p.into_inner())
}

fn write(t: &RwLock<Tree>) -> std::sync::RwLockWriteGuard<'_, Tree> {
    t.write().unwrap_or_else(|p| p.into_inner())
}

fn bytes(b: u64) -> String {
    Bytes(b).to_string()
}

fn pct(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "–".into();
    }
    format!("{:.1} %", part as f64 * 100.0 / whole as f64)
}

fn permille(part: u64, whole: u64) -> u16 {
    if whole == 0 {
        0
    } else {
        (u128::from(part) * 1000 / u128::from(whole)).min(1000) as u16
    }
}

fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn flags_text(a: Attrs) -> String {
    let mut v = Vec::new();
    for (bit, name) in [
        (Attrs::ERROR, "unreadable"),
        (Attrs::INCOMPLETE, "not scanned"),
        (Attrs::EXCLUDED, "excluded"),
        (Attrs::OTHER_VOLUME, "other volume"),
        (Attrs::REPARSE, "link"),
        (Attrs::HARDLINK_DUP, "hard link, counted elsewhere"),
        (Attrs::HARDLINK, "hard-linked"),
        (Attrs::COMPRESSED, "compressed"),
        (Attrs::SPARSE, "sparse"),
        (Attrs::OFFLINE, "offline"),
    ] {
        if a.has(bit) {
            v.push(name);
        }
    }
    v.join(", ")
}

fn safety_text(s: Safety) -> &'static str {
    match s {
        Safety::Safe => "Safe",
        Safety::Caution => "Caution",
        Safety::Review => "Review each file",
    }
}

fn skip_text(r: &SkipReason) -> String {
    match r {
        SkipReason::InUse => "in use".into(),
        SkipReason::Changed => "changed since it was examined".into(),
        SkipReason::Missing => "already gone".into(),
        SkipReason::Link => "a link (never followed)".into(),
        SkipReason::Protected(why) => format!("protected ({why})"),
        SkipReason::Cancelled => "cancelled".into(),
    }
}

impl Storage {
    /// Creates the state. Nothing is scanned and no thread is started.
    pub fn new(platform: StoragePlatform, notify: Notify, utc_offset_min: i32) -> Self {
        let (settings, _) = platform
            .store
            .as_ref()
            .map(StorageStore::load)
            .unwrap_or_default();
        let style = platform.fs.style();
        Self {
            platform,
            notify,
            settings,
            utc_offset_min,
            drives: Vec::new(),
            tree: Arc::new(RwLock::new(Tree::new(style))),
            has_index: false,
            scan_roots: Vec::new(),
            job: 0,
            scan: None,
            summary: None,
            fast_used: false,
            note: None,
            analysis: Analysis::default(),
            stale: false,
            expanded: BTreeSet::new(),
            more: BTreeMap::new(),
            rows: Vec::new(),
            selected: None,
            map_mode: MapMode::Treemap,
            map_w: 0.0,
            map_h: 0.0,
            zoom: TOP,
            back: Vec::new(),
            forward: Vec::new(),
            cells: Vec::new(),
            tab: ListTab::Files,
            sort: ListSort::Size,
            ascending: false,
            search_gen: 0,
            search: None,
            search_message: String::new(),
            cleanup: CleanupState::default(),
            dupes: DupState::default(),
            history: Vec::new(),
            pending: None,
            removing: false,
            message: String::new(),
        }
    }

    fn basis(&self) -> SizeBasis {
        if self.settings.allocated_sizes {
            SizeBasis::Allocated
        } else {
            SizeBasis::Logical
        }
    }

    fn now_s(&self) -> i64 {
        ((self.platform.clock)() / 1000) as i64
    }

    /// The shared index (read by exports).
    pub fn tree(&self) -> &Arc<RwLock<Tree>> {
        &self.tree
    }

    /// Last scan summary.
    pub fn summary(&self) -> Option<&ScanSummary> {
        self.summary.as_ref()
    }

    /// Current settings.
    pub fn settings(&self) -> &StorageSettings {
        &self.settings
    }

    /// Loads the drive list and the cleanup history (first visit of a storage screen).
    pub fn activate(&mut self) -> StorageDirty {
        self.drives = self.platform.volumes.list();
        if let Some(st) = &self.platform.store {
            self.history = st.history();
        }
        StorageDirty {
            drives: true,
            cleanup: true,
            status: true,
            ..StorageDirty::default()
        }
    }

    // ---- Scanning ------------------------------------------------------------------

    /// Starts a scan of `roots` (drives or folders). A running scan is cancelled first.
    pub fn scan(&mut self, roots: Vec<String>) -> StorageDirty {
        let roots: Vec<String> = roots
            .into_iter()
            .map(|r| r.trim().to_owned())
            .filter(|r| !r.is_empty())
            .collect();
        if roots.is_empty() {
            self.message = "Choose a drive or a folder to scan.".into();
            return StorageDirty {
                status: true,
                ..StorageDirty::default()
            };
        }
        if let Some(s) = self.scan.take() {
            s.control.cancel();
        }
        self.job += 1;
        let job = self.job;
        let style = self.platform.fs.style();
        self.tree = Arc::new(RwLock::new(Tree::new(style)));
        self.has_index = true;
        self.scan_roots = roots.clone();
        self.summary = None;
        self.note = None;
        self.stale = false;
        self.analysis = Analysis::default();
        self.expanded.clear();
        self.more.clear();
        self.selected = None;
        self.zoom = TOP;
        self.back.clear();
        self.forward.clear();
        self.search = None;
        self.dupes = DupState::default();
        self.message.clear();

        let vol = self.platform.volumes.volume_of(&roots[0]);
        let threads = if self.settings.scan_threads > 0 {
            self.settings.scan_threads
        } else {
            vol.as_ref()
                .map_or(2, |v| default_threads(v.kind, v.seek_penalty))
        };
        let options = ScanOptions {
            threads,
            exclude: self.settings.scan_exclusions.clone(),
            stay_on_volume: true,
        };
        let whole_volume = roots.len() == 1
            && vol.as_ref().is_some_and(|v| {
                same_root(&v.root, &roots[0]) && v.fs.eq_ignore_ascii_case("ntfs")
            });
        let mut pre_note = None;
        let fast = match &self.platform.fast_scan {
            Some(f) if self.settings.fast_ntfs_scan && self.platform.elevated && whole_volume => {
                // The fast scan reads the whole volume and cannot apply exclusions.
                if self.settings.scan_exclusions.is_empty() {
                    Some(f.clone())
                } else {
                    pre_note = Some("scan exclusions are set".to_owned());
                    None
                }
            }
            _ => None,
        };
        let control = Arc::new(ScanControl::default());
        self.scan = Some(ScanJob {
            job,
            control: control.clone(),
            progress: ScanProgress::default(),
            fast: fast.is_some(),
            refresh: false,
        });
        let fs = self.platform.fs.clone();
        let tree = self.tree.clone();
        let notify = self.notify.clone();
        let basis = self.basis();
        let spawned = std::thread::Builder::new()
            .name("reclaim-storage-scan".into())
            .spawn(move || {
                let mut note = pre_note;
                if let Some(f) = fast {
                    let n2 = notify.clone();
                    let started = std::time::Instant::now();
                    let progress = move |files: u64| {
                        n2(StorageEvent::ScanProgress {
                            job,
                            progress: ScanProgress {
                                files,
                                elapsed_ms: started.elapsed().as_millis() as u64,
                                ..ScanProgress::default()
                            },
                        });
                    };
                    match f(&roots[0], &tree, &control, &progress) {
                        Ok(st) => {
                            let t = read(&tree);
                            let summary = ScanSummary {
                                state: if st.cancelled {
                                    ScanState::Cancelled
                                } else {
                                    ScanState::Complete
                                },
                                progress: ScanProgress {
                                    files: st.files,
                                    dirs: st.dirs,
                                    bytes: t.node(TOP).size(basis),
                                    errors: t.errors().len() as u64,
                                    elapsed_ms: st.elapsed_ms,
                                    paused: false,
                                },
                                incomplete_dirs: 0,
                                hardlink_dups: t.hardlink_duplicates(),
                                partial: st.cancelled || t.has_gaps(),
                            };
                            let analysis = Analysis::of(&t, basis);
                            drop(t);
                            notify(StorageEvent::ScanFinished {
                                job,
                                summary,
                                analysis,
                                fast: true,
                                note: None,
                            });
                            return;
                        }
                        Err(why) => {
                            note = Some(why);
                            *write(&tree) = Tree::new(fs.style());
                        }
                    }
                }
                let n2 = notify.clone();
                let handle = scan::start(
                    fs,
                    tree.clone(),
                    ScanTarget::Roots(roots),
                    options,
                    Box::new(move |p| n2(StorageEvent::ScanProgress { job, progress: p })),
                );
                // Share the control so pause and cancel reach this scan.
                let hc = handle.control().clone();
                let watcher = std::thread::spawn({
                    let control = control.clone();
                    move || {
                        let mut paused = false;
                        while !hc.cancelled() {
                            if control.cancelled() {
                                hc.cancel();
                                break;
                            }
                            let p = control.is_paused();
                            if p != paused {
                                hc.set_paused(p);
                                paused = p;
                            }
                            if handle.is_finished() {
                                return handle.join();
                            }
                            std::thread::sleep(std::time::Duration::from_millis(40));
                        }
                        handle.join()
                    }
                });
                let summary = watcher.join().ok().flatten();
                let t = read(&tree);
                let summary = summary.unwrap_or(ScanSummary {
                    state: ScanState::Cancelled,
                    progress: ScanProgress::default(),
                    incomplete_dirs: 0,
                    hardlink_dups: 0,
                    partial: true,
                });
                let analysis = Analysis::of(&t, basis);
                drop(t);
                notify(StorageEvent::ScanFinished {
                    job,
                    summary,
                    analysis,
                    fast: false,
                    note,
                });
            });
        if spawned.is_err() {
            self.scan = None;
            self.message = "Could not start the scan (no thread available).".into();
        }
        StorageDirty {
            status: true,
            ..StorageDirty::index()
        }
    }

    /// Rescans the selected directory (or everything when nothing is selected).
    pub fn refresh(&mut self) -> StorageDirty {
        if self.scan.is_some() || !self.has_index {
            return StorageDirty::default();
        }
        let target = {
            let t = read(&self.tree);
            self.selected
                .filter(|&id| t.get(id).is_some_and(|n| n.live() && n.is_dir()))
        };
        match target {
            Some(id) => self.refresh_node(id),
            None => {
                let roots = self.scan_roots.clone();
                self.scan(roots)
            }
        }
    }

    /// Rescans the directories containing `paths` (change notifications).
    pub fn refresh_paths(&mut self, paths: &[String]) -> StorageDirty {
        if self.scan.is_some() || !self.has_index || paths.is_empty() {
            return StorageDirty::default();
        }
        let node = {
            let t = read(&self.tree);
            let mut ids: Vec<NodeId> = paths.iter().filter_map(|p| nearest_dir(&t, p)).collect();
            ids.sort_unstable();
            ids.dedup();
            common_ancestor(&t, &ids)
        };
        match node {
            Some(id) => self.refresh_node(id),
            None => {
                self.stale = true;
                StorageDirty {
                    status: true,
                    ..StorageDirty::default()
                }
            }
        }
    }

    fn refresh_node(&mut self, id: NodeId) -> StorageDirty {
        self.job += 1;
        let job = self.job;
        let options = ScanOptions {
            threads: if self.settings.scan_threads > 0 {
                self.settings.scan_threads
            } else {
                2
            },
            exclude: self.settings.scan_exclusions.clone(),
            stay_on_volume: true,
        };
        let n2 = self.notify.clone();
        let handle = scan::start(
            self.platform.fs.clone(),
            self.tree.clone(),
            ScanTarget::Refresh(id),
            options,
            Box::new(move |p| n2(StorageEvent::ScanProgress { job, progress: p })),
        );
        let control = handle.control().clone();
        self.scan = Some(ScanJob {
            job,
            control,
            progress: ScanProgress::default(),
            fast: false,
            refresh: true,
        });
        let tree = self.tree.clone();
        let notify = self.notify.clone();
        let basis = self.basis();
        let _ = std::thread::Builder::new()
            .name("reclaim-storage-refresh".into())
            .spawn(move || {
                let summary = handle.join().unwrap_or(ScanSummary {
                    state: ScanState::Cancelled,
                    progress: ScanProgress::default(),
                    incomplete_dirs: 0,
                    hardlink_dups: 0,
                    partial: true,
                });
                let analysis = Analysis::of(&read(&tree), basis);
                notify(StorageEvent::ScanFinished {
                    job,
                    summary,
                    analysis,
                    fast: false,
                    note: None,
                });
            });
        StorageDirty {
            status: true,
            ..StorageDirty::default()
        }
    }

    /// Pauses or resumes the running scan.
    pub fn set_paused(&mut self, on: bool) -> StorageDirty {
        if let Some(s) = &self.scan
            && !s.fast
        {
            s.control.set_paused(on);
        }
        StorageDirty {
            status: true,
            ..StorageDirty::default()
        }
    }

    /// Cancels the running scan; what was indexed so far stays, marked incomplete.
    pub fn cancel_scan(&mut self) -> StorageDirty {
        if let Some(s) = &self.scan {
            s.control.cancel();
        }
        StorageDirty {
            status: true,
            ..StorageDirty::default()
        }
    }

    /// Whether a scan or refresh is running.
    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    // ---- Events --------------------------------------------------------------------

    /// Applies a background event.
    pub fn apply(&mut self, e: StorageEvent) -> StorageDirty {
        match e {
            StorageEvent::ScanProgress { job, progress } => match &mut self.scan {
                Some(s) if s.job == job => {
                    s.progress = progress;
                    let mut d = StorageDirty {
                        status: true,
                        ..StorageDirty::default()
                    };
                    // Partial results: the top level fills in while the scan runs.
                    if !s.refresh {
                        d.tree = true;
                    }
                    d
                }
                _ => StorageDirty::default(),
            },
            StorageEvent::ScanFinished {
                job,
                summary,
                analysis,
                fast,
                note,
            } => {
                if self.scan.as_ref().is_none_or(|s| s.job != job) {
                    return StorageDirty::default();
                }
                let refresh = self.scan.as_ref().is_some_and(|s| s.refresh);
                self.scan = None;
                if !refresh {
                    self.fast_used = fast;
                    self.note = note;
                    let roots = read(&self.tree).roots();
                    if roots.len() == 1 {
                        self.expanded.insert(roots[0]);
                    }
                }
                self.summary = Some(summary);
                self.analysis = analysis;
                self.stale = false;
                self.fix_selection();
                if self.tab == ListTab::Search && self.search.is_some() {
                    self.search = None;
                }
                StorageDirty::index()
            }
            StorageEvent::SearchDone { generation, result } => {
                if generation != self.search_gen {
                    return StorageDirty::default();
                }
                self.search_message = format!(
                    "{} matches, {}{}",
                    count(result.total),
                    bytes(result.bytes),
                    if result.total > result.ids.len() as u64 {
                        format!(" (first {} shown)", count(result.ids.len() as u64))
                    } else {
                        String::new()
                    }
                );
                self.search = Some(result);
                self.tab = ListTab::Search;
                StorageDirty {
                    lists: true,
                    ..StorageDirty::default()
                }
            }
            StorageEvent::CleanupFound(scan) => {
                let id = scan.id.clone();
                let cat = WINDOWS_CATEGORIES.iter().find(|c| c.id == id);
                if let Some(c) = cat
                    && c.default_selected
                    && c.safety != Safety::Review
                    && (scan.files() > 0 || c.action == Action::WindowsCleanup)
                {
                    self.cleanup.selected.insert(id.clone());
                }
                self.cleanup.scans.insert(id, scan);
                StorageDirty {
                    cleanup: true,
                    ..StorageDirty::default()
                }
            }
            StorageEvent::CleanupScanDone { .. } => {
                self.cleanup.discovering = false;
                self.cleanup.cancel = None;
                StorageDirty {
                    cleanup: true,
                    ..StorageDirty::default()
                }
            }
            StorageEvent::CleanupProgress(p) => {
                if self.cleanup.running.is_some() {
                    self.cleanup.running = Some(p);
                }
                StorageDirty {
                    cleanup: true,
                    ..StorageDirty::default()
                }
            }
            StorageEvent::CleanupDone(report) => {
                self.cleanup.running = None;
                self.cleanup.cancel = None;
                if self.settings.keep_cleanup_history
                    && let Some(st) = &self.platform.store
                {
                    let _ = st.append_history(&report);
                    self.history = st.history();
                }
                self.cleanup.report = Some(report);
                // What was examined is gone or changed: examine again before the next run.
                self.cleanup.scans.clear();
                self.cleanup.selected.clear();
                self.cleanup.files.clear();
                self.drives = self.platform.volumes.list();
                if self.has_index {
                    self.stale = true;
                }
                StorageDirty {
                    cleanup: true,
                    drives: true,
                    status: true,
                    ..StorageDirty::default()
                }
            }
            StorageEvent::DupProgress(p) => {
                if self.dupes.cancel.is_some() {
                    self.dupes.progress = Some(p);
                }
                StorageDirty {
                    dupes: true,
                    ..StorageDirty::default()
                }
            }
            StorageEvent::DupDone(r) => {
                self.dupes.progress = None;
                self.dupes.cancel = None;
                self.dupes.marked.clear();
                self.dupes.message.clear();
                self.dupes.result = Some(r);
                StorageDirty {
                    dupes: true,
                    ..StorageDirty::default()
                }
            }
            StorageEvent::ExportDone(r) => {
                self.message = match r {
                    Ok(path) => format!("Exported to {path}"),
                    Err(e) => format!("Export failed: {e}"),
                };
                StorageDirty {
                    status: true,
                    ..StorageDirty::default()
                }
            }
            StorageEvent::RemovalDone { outcomes, analysis } => {
                self.removing = false;
                let mut recycled = 0u64;
                let mut moved = 0u64;
                let mut problems = Vec::new();
                for (path, o) in &outcomes {
                    match o {
                        ItemOutcome::Recycled(b) | ItemOutcome::Deleted(b) => {
                            moved += 1;
                            recycled += b;
                        }
                        ItemOutcome::Skipped(r) => {
                            problems.push(format!("{path}: {}", skip_text(r)))
                        }
                        ItemOutcome::Failed(k) => problems.push(format!("{path}: {}", k.id())),
                    }
                }
                self.message = format!(
                    "Moved {} item(s), {}, to the Recycle Bin. Space is freed when the bin is emptied.{}",
                    count(moved),
                    bytes(recycled),
                    if problems.is_empty() {
                        String::new()
                    } else {
                        format!(" Not moved: {}", problems.join("; "))
                    }
                );
                self.analysis = analysis;
                self.fix_selection();
                // Duplicate groups referring to recycled files are no longer accurate.
                if let Some(r) = &mut self.dupes.result {
                    let gone: BTreeSet<&str> = outcomes
                        .iter()
                        .filter(|(_, o)| {
                            matches!(o, ItemOutcome::Recycled(_) | ItemOutcome::Deleted(_))
                        })
                        .map(|(p, _)| p.as_str())
                        .collect();
                    for g in &mut r.groups {
                        g.files.retain(|f| !gone.contains(f.path.as_str()));
                    }
                    r.groups.retain(|g| g.files.len() > 1);
                    self.dupes.marked.clear();
                    self.dupes.message = self.message.clone();
                }
                self.drives = self.platform.volumes.list();
                StorageDirty {
                    drives: true,
                    ..StorageDirty::index()
                }
            }
        }
    }

    fn fix_selection(&mut self) {
        let t = read(&self.tree);
        if self
            .selected
            .is_some_and(|id| t.get(id).is_none_or(|n| !n.live()))
        {
            self.selected = None;
        }
        if self.zoom != TOP && t.get(self.zoom).is_none_or(|n| !n.live()) {
            self.zoom = TOP;
        }
        self.back.retain(|&id| t.get(id).is_some_and(|n| n.live()));
        self.forward
            .retain(|&id| t.get(id).is_some_and(|n| n.live()));
    }

    // ---- Tree and selection --------------------------------------------------------

    /// Expands or collapses a directory row.
    pub fn toggle_expanded(&mut self, id: NodeId) -> StorageDirty {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        StorageDirty {
            tree: true,
            ..StorageDirty::default()
        }
    }

    /// Shows more children of a directory ("more" row).
    pub fn show_more(&mut self, id: NodeId) -> StorageDirty {
        *self.more.entry(id).or_insert(TREE_CHILD_CAP) += TREE_CHILD_CAP;
        StorageDirty {
            tree: true,
            ..StorageDirty::default()
        }
    }

    /// Selects a node everywhere (tree, map, details); its ancestors are expanded so the
    /// tree shows it.
    pub fn select(&mut self, id: NodeId) -> StorageDirty {
        let valid = read(&self.tree).get(id).is_some_and(|n| n.live()) && id != TOP;
        if !valid {
            return StorageDirty::default();
        }
        self.selected = Some(id);
        let t = read(&self.tree);
        let ancestors: Vec<NodeId> = t.ancestors(id).filter(|&a| a != id && a != TOP).collect();
        // A child beyond the shown cap: raise the cap for its parent.
        let parent = t.node(id).parent;
        let pos = sorted_children(&t, parent, self.basis())
            .iter()
            .position(|&c| c == id)
            .unwrap_or(0);
        drop(t);
        self.expanded.extend(ancestors);
        let cap = self.more.get(&parent).copied().unwrap_or(TREE_CHILD_CAP);
        if pos >= cap {
            self.more.insert(parent, pos + 1);
        }
        StorageDirty {
            tree: true,
            selection: true,
            map: true,
            ..StorageDirty::default()
        }
    }

    /// Selects by a treemap point (map coordinates); returns the selected node.
    pub fn select_at(&mut self, x: f32, y: f32) -> StorageDirty {
        match hit(&self.cells, x, y) {
            Some(c) => {
                let id = c.node;
                self.select(id)
            }
            None => StorageDirty::default(),
        }
    }

    /// Node under a treemap point.
    pub fn node_at(&self, x: f32, y: f32) -> Option<NodeId> {
        hit(&self.cells, x, y).map(|c| c.node)
    }

    /// Tooltip text for a node.
    pub fn describe(&self, id: NodeId) -> String {
        let t = read(&self.tree);
        let Some(n) = t.get(id).filter(|n| n.live()) else {
            return String::new();
        };
        let basis = self.basis();
        let total = t.node(TOP).size(basis);
        let mut s = format!(
            "{}\n{} ({} of everything scanned)",
            t.path(id),
            bytes(n.size(basis)),
            pct(n.size(basis), total)
        );
        if n.is_dir() {
            let c = t.counts(id);
            s.push_str(&format!(
                "\n{} files, {} folders",
                count(c.files),
                count(c.dirs)
            ));
        }
        let f = flags_text(n.attrs);
        if !f.is_empty() {
            s.push('\n');
            s.push_str(&f);
        }
        s
    }

    /// Zooms the map into a directory (or the selection's directory).
    pub fn zoom_in(&mut self, id: NodeId) -> StorageDirty {
        let target = {
            let t = read(&self.tree);
            match t.get(id) {
                Some(n) if n.live() && n.is_dir() => Some(id),
                Some(n) if n.live() => Some(n.parent),
                _ => None,
            }
        };
        match target {
            Some(z) if z != self.zoom => {
                self.back.push(self.zoom);
                self.forward.clear();
                self.zoom = z;
                StorageDirty {
                    map: true,
                    selection: true,
                    ..StorageDirty::default()
                }
            }
            _ => StorageDirty::default(),
        }
    }

    /// Zooms out one level.
    pub fn zoom_up(&mut self) -> StorageDirty {
        if self.zoom == TOP {
            return StorageDirty::default();
        }
        let parent = read(&self.tree).node(self.zoom).parent;
        self.back.push(self.zoom);
        self.forward.clear();
        self.zoom = parent;
        StorageDirty {
            map: true,
            selection: true,
            ..StorageDirty::default()
        }
    }

    /// Previous zoom.
    pub fn back(&mut self) -> StorageDirty {
        match self.back.pop() {
            Some(z) => {
                self.forward.push(self.zoom);
                self.zoom = z;
                StorageDirty {
                    map: true,
                    selection: true,
                    ..StorageDirty::default()
                }
            }
            None => StorageDirty::default(),
        }
    }

    /// Next zoom.
    pub fn forward(&mut self) -> StorageDirty {
        match self.forward.pop() {
            Some(z) => {
                self.back.push(self.zoom);
                self.zoom = z;
                StorageDirty {
                    map: true,
                    selection: true,
                    ..StorageDirty::default()
                }
            }
            None => StorageDirty::default(),
        }
    }

    /// Sets the map area in logical pixels.
    pub fn set_map_size(&mut self, w: f32, h: f32) -> StorageDirty {
        if (w - self.map_w).abs() < 0.5 && (h - self.map_h).abs() < 0.5 {
            return StorageDirty::default();
        }
        self.map_w = w.max(0.0);
        self.map_h = h.max(0.0);
        StorageDirty {
            map: true,
            ..StorageDirty::default()
        }
    }

    /// Switches between treemap and icicle.
    pub fn set_map_mode(&mut self, m: MapMode) -> StorageDirty {
        self.map_mode = m;
        StorageDirty {
            map: true,
            ..StorageDirty::default()
        }
    }

    /// Current map mode.
    pub fn map_mode(&self) -> MapMode {
        self.map_mode
    }

    /// Path of the selected node.
    pub fn selected_path(&self) -> Option<String> {
        let id = self.selected?;
        let t = read(&self.tree);
        t.get(id).filter(|n| n.live()).map(|_| t.path(id))
    }

    /// Selected node.
    pub fn selected(&self) -> Option<NodeId> {
        self.selected
    }

    // ---- Lists and search ----------------------------------------------------------

    /// Switches the bottom tab.
    pub fn set_tab(&mut self, tab: ListTab) -> StorageDirty {
        self.tab = tab;
        StorageDirty {
            lists: true,
            ..StorageDirty::default()
        }
    }

    /// Current bottom tab.
    pub fn tab(&self) -> ListTab {
        self.tab
    }

    /// Sorts the file, directory and search lists.
    pub fn sort_lists(&mut self, by: ListSort, ascending: bool) -> StorageDirty {
        self.sort = by;
        self.ascending = ascending;
        StorageDirty {
            lists: true,
            ..StorageDirty::default()
        }
    }

    /// Current list sort.
    pub fn list_sort(&self) -> (ListSort, bool) {
        (self.sort, self.ascending)
    }

    /// Runs a search over the index on a background thread (no rescan).
    pub fn search(&mut self, form: &SearchForm) -> StorageDirty {
        let d = StorageDirty {
            lists: true,
            ..StorageDirty::default()
        };
        if !self.has_index {
            self.search_message = "Scan a drive or folder first.".into();
            return d;
        }
        let filter = match form.parse(self.basis(), self.now_s()) {
            Ok(f) => f,
            Err(e) => {
                self.search_message = e;
                self.tab = ListTab::Search;
                return d;
            }
        };
        if filter.is_empty() {
            self.search = None;
            self.search_message = "Enter at least one criterion.".into();
            self.tab = ListTab::Search;
            return d;
        }
        self.search_gen += 1;
        let generation = self.search_gen;
        self.search_message = "Searching…".into();
        self.tab = ListTab::Search;
        let tree = self.tree.clone();
        let notify = self.notify.clone();
        let root = self.zoom;
        let _ = std::thread::Builder::new()
            .name("reclaim-storage-search".into())
            .spawn(move || {
                let result = filter.run(&read(&tree), root, SEARCH_LIMIT);
                notify(StorageEvent::SearchDone { generation, result });
            });
        d
    }

    /// Clears the search.
    pub fn clear_search(&mut self) -> StorageDirty {
        self.search_gen += 1;
        self.search = None;
        self.search_message.clear();
        StorageDirty {
            lists: true,
            ..StorageDirty::default()
        }
    }

    /// Search status line.
    pub fn search_message(&self) -> &str {
        &self.search_message
    }

    /// Rows of the current bottom tab.
    pub fn list_rows(&self) -> Vec<ListRowView> {
        let t = read(&self.tree);
        let basis = self.basis();
        let slot = |id: NodeId| self.slot_of(&t, id);
        let node_rows = |ids: &[NodeId]| -> Vec<ListRowView> {
            let mut ids: Vec<NodeId> = ids
                .iter()
                .copied()
                .filter(|&id| t.get(id).is_some_and(|n| n.live()))
                .collect();
            sort_nodes(&t, &mut ids, self.sort.key(), basis, self.ascending);
            ids.into_iter()
                .map(|id| {
                    let n = t.node(id);
                    ListRowView {
                        id: Some(id),
                        name: t.name(id).to_owned(),
                        detail: t.path(id),
                        size: bytes(n.size(basis)),
                        extra: if n.mtime > 0 {
                            date_time(Millis(n.mtime as u64 * 1000), self.utc_offset_min)
                        } else {
                            String::new()
                        },
                        slot: if n.is_dir() { SLOT_DIR } else { slot(id) },
                    }
                })
                .collect()
        };
        match self.tab {
            ListTab::Files => node_rows(&self.analysis.files),
            ListTab::Dirs => node_rows(&self.analysis.dirs),
            ListTab::Search => self
                .search
                .as_ref()
                .map(|r| node_rows(&r.ids))
                .unwrap_or_default(),
            ListTab::Extensions => {
                let total: u64 = self
                    .analysis
                    .ext
                    .iter()
                    .map(|e| match basis {
                        SizeBasis::Allocated => e.allocated,
                        SizeBasis::Logical => e.logical,
                    })
                    .sum();
                self.analysis
                    .ext
                    .iter()
                    .map(|e| {
                        let size = match basis {
                            SizeBasis::Allocated => e.allocated,
                            SizeBasis::Logical => e.logical,
                        };
                        ListRowView {
                            id: None,
                            name: if e.name.is_empty() {
                                "(no extension)".into()
                            } else {
                                format!(".{}", e.name)
                            },
                            detail: format!("{} files", count(e.files)),
                            size: bytes(size),
                            extra: pct(size, total),
                            slot: self
                                .analysis
                                .palette
                                .get(e.ext as usize)
                                .copied()
                                .unwrap_or(0),
                        }
                    })
                    .collect()
            }
            ListTab::Errors => {
                let mut v: Vec<ListRowView> = t
                    .errors()
                    .iter()
                    .map(|e| ListRowView {
                        id: t.find(&e.path),
                        name: e.kind.id(),
                        detail: e.path.clone(),
                        size: String::new(),
                        extra: String::new(),
                        slot: 0,
                    })
                    .collect();
                if t.errors_dropped() > 0 {
                    v.push(ListRowView {
                        id: None,
                        name: "more".into(),
                        detail: format!(
                            "{} more unreadable locations are not listed",
                            count(t.errors_dropped())
                        ),
                        size: String::new(),
                        extra: String::new(),
                        slot: 0,
                    });
                }
                v
            }
        }
    }

    fn slot_of(&self, t: &Tree, id: NodeId) -> u8 {
        self.analysis
            .palette
            .get(t.ext_id(id) as usize)
            .copied()
            .unwrap_or(0)
    }

    // ---- Views ---------------------------------------------------------------------

    /// Drives.
    pub fn drives(&self) -> Vec<DriveView> {
        self.drives
            .iter()
            .map(|v| {
                let kind = match v.kind {
                    DriveKind::Fixed => "",
                    DriveKind::Removable => ", removable",
                    DriveKind::Network => ", network",
                    DriveKind::Optical => ", optical",
                    DriveKind::Ram => ", RAM disk",
                    DriveKind::Unknown => "",
                };
                DriveView {
                    root: v.root.clone(),
                    title: format!(
                        "{} {} ({}{kind})",
                        v.root,
                        if v.label.is_empty() {
                            "Local disk"
                        } else {
                            &v.label
                        },
                        if v.fs.is_empty() { "?" } else { &v.fs }
                    )
                    .replace("  ", " "),
                    detail: format!("{} free of {}", bytes(v.free), bytes(v.total)),
                    used_permille: permille(v.total.saturating_sub(v.free), v.total),
                }
            })
            .collect()
    }

    /// Status line and buttons.
    pub fn status(&self) -> StatusView {
        if let Some(s) = &self.scan {
            let p = s.progress;
            let what = if s.fast {
                "Fast NTFS scan"
            } else if s.refresh {
                "Refreshing"
            } else {
                "Scanning"
            };
            let line = if s.fast {
                format!(
                    "{what}: {} files read from the file table, {:.1} s",
                    count(p.files),
                    p.elapsed_ms as f64 / 1000.0
                )
            } else {
                format!(
                    "{what}{}: {} files, {} folders, {}, {} unreadable, {:.1} s",
                    if p.paused { " (paused)" } else { "" },
                    count(p.files),
                    count(p.dirs),
                    bytes(p.bytes),
                    count(p.errors),
                    p.elapsed_ms as f64 / 1000.0
                )
            };
            return StatusView {
                line,
                scanning: true,
                paused: p.paused || s.control.is_paused(),
                has_index: true,
                can_pause: !s.fast,
                partial: true,
            };
        }
        let mut line = String::new();
        let mut partial = false;
        if let Some(s) = &self.summary {
            let t = read(&self.tree);
            let basis = self.basis();
            let c = t.counts(TOP);
            partial = s.partial || s.state != ScanState::Complete;
            line = format!(
                "{}: {} in {} files, {} folders, {:.1} s{}{}",
                match s.state {
                    ScanState::Complete if partial => "Scan finished with gaps",
                    ScanState::Complete => "Scan complete",
                    _ => "Scan cancelled, results are partial",
                },
                bytes(t.node(TOP).size(basis)),
                count(c.files),
                count(c.dirs),
                s.progress.elapsed_ms as f64 / 1000.0,
                if c.errors > 0 {
                    format!(", {} unreadable (see Errors)", count(c.errors))
                } else {
                    String::new()
                },
                if self.fast_used {
                    ", fast NTFS scan"
                } else {
                    ""
                },
            );
            if t.allocated_estimated() && basis == SizeBasis::Allocated {
                line.push_str(", some sizes on disk estimated");
            }
            if let Some(n) = &self.note {
                line.push_str(&format!(". Fast scan not used: {n}"));
            }
            if self.stale {
                line.push_str(". Files changed since this scan; press Refresh");
            }
        }
        if !self.message.is_empty() {
            if !line.is_empty() {
                line.push_str(". ");
            }
            line.push_str(&self.message);
        }
        if line.is_empty() {
            line =
                "Choose a drive or folder and press Scan. Nothing is scanned until you ask.".into();
        }
        StatusView {
            line,
            scanning: false,
            paused: false,
            has_index: self.has_index,
            can_pause: false,
            partial,
        }
    }

    /// Rebuilds and returns the tree rows.
    pub fn tree_rows(&mut self) -> &[TreeRowView] {
        enum Item {
            Node(NodeId, usize),
            More(NodeId, usize, usize, u64),
        }
        let t = read(&self.tree);
        let basis = self.basis();
        let total = t.node(TOP).size(basis);
        let mut rows = Vec::new();
        let roots = sorted_children(&t, TOP, basis);
        let mut stack: Vec<Item> = roots.into_iter().rev().map(|r| Item::Node(r, 0)).collect();
        while let Some(item) = stack.pop() {
            let (id, depth) = match item {
                Item::Node(id, depth) => (id, depth),
                Item::More(parent, depth, n, rest) => {
                    rows.push(TreeRowView {
                        id: parent,
                        depth,
                        name: format!("… {} more items", count(n as u64)),
                        size: bytes(rest),
                        share: 0,
                        parent_pct: String::new(),
                        total_pct: String::new(),
                        files: String::new(),
                        dirs: String::new(),
                        modified: String::new(),
                        expandable: false,
                        expanded: false,
                        selected: false,
                        is_dir: false,
                        flags: String::new(),
                        more: true,
                        slot: SLOT_REST,
                    });
                    continue;
                }
            };
            let n = t.node(id);
            let parent_size = if n.parent == TOP {
                total
            } else {
                t.node(n.parent).size(basis)
            };
            let size = n.size(basis);
            let c = t.counts(id);
            let expandable = n.is_dir() && t.child_count(id) > 0;
            let expanded = expandable && self.expanded.contains(&id);
            rows.push(TreeRowView {
                id,
                depth,
                name: t.name(id).to_owned(),
                size: bytes(size),
                share: permille(size, parent_size),
                parent_pct: pct(size, parent_size),
                total_pct: pct(size, total),
                files: if n.is_dir() {
                    count(c.files)
                } else {
                    String::new()
                },
                dirs: if n.is_dir() {
                    count(c.dirs)
                } else {
                    String::new()
                },
                modified: if n.mtime > 0 {
                    date_time(Millis(n.mtime as u64 * 1000), self.utc_offset_min)
                } else {
                    String::new()
                },
                expandable,
                expanded,
                selected: self.selected == Some(id),
                is_dir: n.is_dir(),
                flags: flags_text(n.attrs),
                more: false,
                slot: if n.is_dir() {
                    SLOT_DIR
                } else {
                    self.slot_of(&t, id)
                },
            });
            if expanded {
                let kids = sorted_children(&t, id, basis);
                let cap = self.more.get(&id).copied().unwrap_or(TREE_CHILD_CAP);
                if kids.len() > cap {
                    let rest: u64 = kids[cap..].iter().map(|&k| t.node(k).size(basis)).sum();
                    // Pushed first so it is shown after the children.
                    stack.push(Item::More(id, depth + 1, kids.len() - cap, rest));
                }
                for &k in kids.iter().take(cap).rev() {
                    stack.push(Item::Node(k, depth + 1));
                }
            }
        }
        drop(t);
        self.rows = rows;
        &self.rows
    }

    /// Row index of the selection in the last built tree rows.
    pub fn selected_row(&self) -> Option<usize> {
        let s = self.selected?;
        self.rows.iter().position(|r| !r.more && r.id == s)
    }

    /// Rebuilds and returns the map tiles.
    pub fn tiles(&mut self) -> Vec<TileView> {
        let t = read(&self.tree);
        let o = LayoutOptions {
            basis: self.basis(),
            max_cells: MAX_TILES,
            ..LayoutOptions::default()
        };
        let area = Rect::new(0.0, 0.0, self.map_w, self.map_h);
        let root = if t.get(self.zoom).is_some_and(|n| n.live()) {
            self.zoom
        } else {
            TOP
        };
        let cells = match self.map_mode {
            MapMode::Treemap => treemap(&t, root, area, &o),
            MapMode::Icicle => {
                let levels = 12.0f32;
                icicle(&t, root, area, (self.map_h / levels).max(14.0), &o)
            }
        };
        let tiles = cells
            .iter()
            .filter(|c| c.rect.w >= 1.0 && c.rect.h >= 1.0)
            .filter(|c| c.leaf || self.map_mode == MapMode::Icicle || c.depth > 0)
            .map(|c| {
                let n = t.node(c.node);
                TileView {
                    x: c.rect.x,
                    y: c.rect.y,
                    w: c.rect.w,
                    h: c.rect.h,
                    slot: if c.rest {
                        SLOT_REST
                    } else if n.is_dir() {
                        SLOT_DIR
                    } else {
                        self.slot_of(&t, c.node)
                    },
                    node: c.node,
                    leaf: c.leaf || self.map_mode == MapMode::Icicle,
                }
            })
            .collect();
        drop(t);
        self.cells = cells;
        tiles
    }

    /// Rectangle of the selection in the last built map.
    pub fn selection_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let s = self.selected?;
        self.cells
            .iter()
            .find(|c| c.node == s && !c.rest)
            .map(|c| (c.rect.x, c.rect.y, c.rect.w, c.rect.h))
    }

    /// Selection details.
    pub fn selection(&self) -> SelectionView {
        let t = read(&self.tree);
        let basis = self.basis();
        let zoom_path = if self.zoom == TOP || t.get(self.zoom).is_none() {
            "Everything scanned".into()
        } else {
            t.path(self.zoom)
        };
        let mut v = SelectionView {
            zoom_path,
            can_back: !self.back.is_empty(),
            can_forward: !self.forward.is_empty(),
            can_up: self.zoom != TOP,
            ..SelectionView::default()
        };
        let Some(id) = self
            .selected
            .filter(|&id| t.get(id).is_some_and(|n| n.live()))
        else {
            return v;
        };
        let n = t.node(id);
        let total = t.node(TOP).size(basis);
        let parent = if n.parent == TOP {
            total
        } else {
            t.node(n.parent).size(basis)
        };
        v.any = true;
        v.path = t.path(id);
        let mut d = format!(
            "Size: {} ({} on disk), {} of parent, {} of total",
            bytes(n.logical),
            bytes(n.allocated),
            pct(n.size(basis), parent),
            pct(n.size(basis), total)
        );
        if n.is_dir() {
            let c = t.counts(id);
            d.push_str(&format!(
                "\nContains {} files and {} folders",
                count(c.files),
                count(c.dirs)
            ));
            if c.errors > 0 {
                d.push_str(&format!(", {} unreadable", count(c.errors)));
            }
        }
        if n.mtime > 0 {
            d.push_str(&format!(
                "\nModified {}",
                date_time(Millis(n.mtime as u64 * 1000), self.utc_offset_min)
            ));
        }
        let f = flags_text(n.attrs);
        if !f.is_empty() {
            d.push_str(&format!("\n{f}"));
        }
        v.detail = d;
        v.can_recycle = self.scan.is_none()
            && !self.removing
            && n.parent != TOP
            && self.platform.guard.check_user_delete(&v.path).is_ok();
        v.can_refresh = self.scan.is_none() && n.is_dir();
        v
    }

    // ---- Recycling from the analyzer -----------------------------------------------

    /// Asks to move the selection to the Recycle Bin.
    pub fn request_recycle_selected(&mut self) -> StorageDirty {
        let Some(id) = self.selected else {
            return StorageDirty::default();
        };
        let (path, size, is_dir, files) = {
            let t = read(&self.tree);
            let Some(n) = t.get(id).filter(|n| n.live()) else {
                return StorageDirty::default();
            };
            (
                t.path(id),
                n.size(self.basis()),
                n.is_dir(),
                t.counts(id).files,
            )
        };
        if let Err(why) = self.platform.guard.check_user_delete(&path) {
            self.message = format!("{path} cannot be removed: {why}.");
            return StorageDirty {
                status: true,
                ..StorageDirty::default()
            };
        }
        let body = format!(
            "{path}\n{}{}\n\nIt goes to the Recycle Bin, so you can restore it. Disk space is freed only when the bin is emptied. On drives without a Recycle Bin nothing is removed.",
            bytes(size),
            if is_dir {
                format!(", {} files", count(files))
            } else {
                String::new()
            }
        );
        self.pending = Some((
            Pending::RecycleNodes(vec![id]),
            ConfirmView {
                title: format!(
                    "Move this {} to the Recycle Bin?",
                    if is_dir { "folder" } else { "file" }
                ),
                body,
                ok: "Move to Recycle Bin".into(),
                danger: false,
            },
        ));
        StorageDirty {
            confirm: true,
            ..StorageDirty::default()
        }
    }

    fn recycle_nodes(&mut self, ids: Vec<NodeId>) {
        let targets: Vec<(NodeId, Target)> = {
            let t = read(&self.tree);
            ids.into_iter()
                .filter(|&id| t.get(id).is_some_and(|n| n.live()))
                .map(|id| {
                    let n = t.node(id);
                    (
                        id,
                        Target {
                            path: t.path(id),
                            root: None,
                            file_id: None,
                            // A file rewritten since the scan is skipped; a folder's time
                            // changes with its contents, so it is not compared.
                            mtime: (!n.is_dir()).then_some(n.mtime),
                            allocated: n.allocated,
                            is_dir: n.is_dir(),
                        },
                    )
                })
                .collect()
        };
        self.removing = true;
        let remover = self.platform.remover.clone();
        let guard = self.platform.guard.clone();
        let tree = self.tree.clone();
        let notify = self.notify.clone();
        let basis = self.basis();
        let _ = std::thread::Builder::new()
            .name("reclaim-storage-recycle".into())
            .spawn(move || {
                let mut outcomes = Vec::new();
                let mut gone = Vec::new();
                for (id, target) in targets {
                    let o = remover.recycle(&target, &guard);
                    if matches!(o, ItemOutcome::Recycled(_) | ItemOutcome::Deleted(_)) {
                        gone.push(id);
                    }
                    outcomes.push((target.path, o));
                }
                let mut t = write(&tree);
                for id in gone {
                    t.remove(id);
                }
                let analysis = Analysis::of(&t, basis);
                drop(t);
                notify(StorageEvent::RemovalDone { outcomes, analysis });
            });
    }

    // ---- Confirmation --------------------------------------------------------------

    /// Open confirmation.
    pub fn confirm_view(&self) -> Option<&ConfirmView> {
        self.pending.as_ref().map(|(_, v)| v)
    }

    /// Confirms the open dialog.
    pub fn confirm(&mut self) -> StorageDirty {
        let Some((p, _)) = self.pending.take() else {
            return StorageDirty::default();
        };
        let mut d = StorageDirty {
            confirm: true,
            ..StorageDirty::default()
        };
        match p {
            Pending::RecycleNodes(ids) => {
                self.recycle_nodes(ids);
                d.selection = true;
                d.status = true;
            }
            Pending::Clean => {
                self.run_cleanup();
                d.cleanup = true;
            }
            Pending::RecycleDupes(files) => {
                self.recycle_duplicates(files);
                d.dupes = true;
            }
        }
        d
    }

    /// Closes the dialog without acting.
    pub fn dismiss(&mut self) -> StorageDirty {
        self.pending = None;
        StorageDirty {
            confirm: true,
            ..StorageDirty::default()
        }
    }

    // ---- Cleanup -------------------------------------------------------------------

    /// Examines every category (read-only) on a background thread.
    pub fn cleanup_scan(&mut self) -> StorageDirty {
        if self.cleanup.discovering || self.cleanup.running.is_some() {
            return StorageDirty::default();
        }
        self.cleanup.scans.clear();
        self.cleanup.selected.clear();
        self.cleanup.files.clear();
        self.cleanup.report = None;
        self.cleanup.discovering = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cleanup.cancel = Some(cancel.clone());
        let p = self.platform.clone();
        let notify = self.notify.clone();
        let opts = DiscoverOptions {
            now: self.now_s(),
            exclude: self.settings.cleanup_exclusions.clone(),
        };
        let spawned = std::thread::Builder::new()
            .name("reclaim-cleanup-scan".into())
            .spawn(move || {
                for cat in WINDOWS_CATEGORIES {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let scan = discover(
                        cat,
                        &p.folders,
                        &*p.fs,
                        &*p.remover,
                        &p.guard,
                        &opts,
                        &cancel,
                    );
                    notify(StorageEvent::CleanupFound(scan));
                }
                notify(StorageEvent::CleanupScanDone {
                    cancelled: cancel.load(Ordering::Relaxed),
                });
            });
        if spawned.is_err() {
            self.cleanup.discovering = false;
        }
        StorageDirty {
            cleanup: true,
            ..StorageDirty::default()
        }
    }

    /// Stops discovery or execution.
    pub fn cleanup_cancel(&mut self) -> StorageDirty {
        if let Some(c) = &self.cleanup.cancel {
            c.store(true, Ordering::Relaxed);
        }
        StorageDirty {
            cleanup: true,
            ..StorageDirty::default()
        }
    }

    /// Shows a category's details.
    pub fn cleanup_focus(&mut self, id: &str) -> StorageDirty {
        self.cleanup.focused = Some(id.to_owned());
        StorageDirty {
            cleanup: true,
            ..StorageDirty::default()
        }
    }

    /// Selects or deselects a category. A review category is selected file by file.
    pub fn cleanup_toggle(&mut self, id: &str) -> StorageDirty {
        self.cleanup.focused = Some(id.to_owned());
        let Some(cat) = WINDOWS_CATEGORIES.iter().find(|c| c.id == id) else {
            return StorageDirty::default();
        };
        let examined = self.cleanup.scans.contains_key(id);
        if !examined || self.cleanup.running.is_some() {
            return StorageDirty {
                cleanup: true,
                ..StorageDirty::default()
            };
        }
        if !self.cleanup.selected.remove(id) {
            if cat.safety == Safety::Review
                && self.cleanup.files.get(id).is_none_or(BTreeSet::is_empty)
            {
                // Nothing chosen yet: the per-file list is shown; select files there.
                self.message = "Select the files of this category one by one.".into();
            } else {
                self.cleanup.selected.insert(id.to_owned());
            }
        }
        StorageDirty {
            cleanup: true,
            ..StorageDirty::default()
        }
    }

    /// Toggles one candidate file of the focused review category.
    pub fn cleanup_toggle_file(&mut self, index: usize) -> StorageDirty {
        let Some(id) = self.cleanup.focused.clone() else {
            return StorageDirty::default();
        };
        let n = self
            .cleanup
            .scans
            .get(&id)
            .map_or(0, |s| s.candidates.len());
        if index >= n || self.cleanup.running.is_some() {
            return StorageDirty::default();
        }
        let set = self.cleanup.files.entry(id.clone()).or_default();
        if !set.remove(&index) {
            set.insert(index);
        }
        if set.is_empty() {
            self.cleanup.selected.remove(&id);
        } else {
            self.cleanup.selected.insert(id);
        }
        StorageDirty {
            cleanup: true,
            ..StorageDirty::default()
        }
    }

    fn plan(&self) -> CleanupPlan {
        let mut entries = Vec::new();
        for cat in WINDOWS_CATEGORIES {
            if !self.cleanup.selected.contains(cat.id) {
                continue;
            }
            let Some(scan) = self.cleanup.scans.get(cat.id) else {
                continue;
            };
            let selected = if cat.safety == Safety::Review {
                Some(
                    self.cleanup
                        .files
                        .get(cat.id)
                        .map(|s| s.iter().copied().collect())
                        .unwrap_or_default(),
                )
            } else {
                None
            };
            entries.push(PlanEntry {
                category: cat,
                scan: scan.clone(),
                selected,
            });
        }
        CleanupPlan { entries }
    }

    /// Asks to clean the selected categories.
    pub fn request_cleanup(&mut self) -> StorageDirty {
        let plan = self.plan();
        if plan.entries.is_empty() || self.cleanup.running.is_some() || self.cleanup.discovering {
            return StorageDirty::default();
        }
        let mut body = String::new();
        let permanent: u64 = plan
            .entries
            .iter()
            .filter(|e| e.category.action == Action::Delete)
            .map(|e| match &e.selected {
                Some(ix) => ix.len() as u64,
                None => e.scan.candidates.len() as u64,
            })
            .sum();
        if permanent > 0 {
            body.push_str(&format!(
                "{} cache and temporary files will be deleted permanently (they do not go to the Recycle Bin; programs recreate what they need).\n",
                count(permanent)
            ));
        }
        let recycle = plan.to_recycle();
        if recycle > 0 {
            body.push_str(&format!(
                "{} of your files will be moved to the Recycle Bin; you can restore them until the bin is emptied.\n",
                bytes(recycle)
            ));
        }
        if plan
            .entries
            .iter()
            .any(|e| e.category.action == Action::EmptyRecycleBin)
        {
            body.push_str(
                "The Recycle Bin will be emptied: its contents cannot be restored afterwards.\n",
            );
        }
        if plan
            .entries
            .iter()
            .any(|e| e.category.action == Action::WindowsCleanup)
        {
            body.push_str("Windows Disk Cleanup opens for system files; Windows asks for administrator approval there, not Reclaim.\n");
        }
        body.push_str(&format!(
            "\nEstimated space freed: {}. Each file is checked again right before it is removed; files in use, changed or protected are skipped and reported.",
            bytes(plan.estimate())
        ));
        let danger = plan.deletes_permanently();
        self.pending = Some((
            Pending::Clean,
            ConfirmView {
                title: format!("Clean {} selected categories?", plan.entries.len()),
                body,
                ok: if danger {
                    "Delete".into()
                } else {
                    "Clean".into()
                },
                danger,
            },
        ));
        StorageDirty {
            confirm: true,
            ..StorageDirty::default()
        }
    }

    fn run_cleanup(&mut self) {
        let plan = self.plan();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cleanup.cancel = Some(cancel.clone());
        self.cleanup.running = Some(CleanupProgress::default());
        let p = self.platform.clone();
        let notify = self.notify.clone();
        let spawned = std::thread::Builder::new()
            .name("reclaim-cleanup".into())
            .spawn(move || {
                let n2 = notify.clone();
                let report = execute(&plan, &*p.remover, &*p.volumes, &p.guard, &cancel, &|pr| {
                    n2(StorageEvent::CleanupProgress(pr));
                });
                notify(StorageEvent::CleanupDone(report));
            });
        if spawned.is_err() {
            self.cleanup.running = None;
            self.cleanup.cancel = None;
        }
    }

    /// Clears the stored cleanup history.
    pub fn clear_history(&mut self) -> StorageDirty {
        if let Some(st) = &self.platform.store {
            let _ = st.clear_history();
        }
        self.history.clear();
        StorageDirty {
            cleanup: true,
            ..StorageDirty::default()
        }
    }

    /// Cleanup screen.
    pub fn cleanup_view(&self) -> CleanupView {
        let c = &self.cleanup;
        let mut v = CleanupView {
            busy: c.discovering || c.running.is_some(),
            ..CleanupView::default()
        };
        for cat in WINDOWS_CATEGORIES {
            let scan = c.scans.get(cat.id);
            let chosen = c.files.get(cat.id);
            let (size, files) = match scan {
                None if c.discovering => ("examining…".to_owned(), String::new()),
                None => ("not examined".to_owned(), String::new()),
                Some(_) if cat.action == Action::WindowsCleanup => {
                    ("opens Windows Disk Cleanup".to_owned(), String::new())
                }
                Some(s) if cat.safety == Safety::Review => {
                    let sel: u64 = chosen
                        .map(|ix| {
                            ix.iter()
                                .filter_map(|&i| s.candidates.get(i))
                                .map(|c| c.allocated)
                                .sum()
                        })
                        .unwrap_or(0);
                    (
                        format!(
                            "{} found, {} selected",
                            bytes(s.candidates.iter().map(|c| c.allocated).sum()),
                            bytes(sel)
                        ),
                        format!("{} files", count(s.files())),
                    )
                }
                Some(s) => (
                    bytes(s.estimate()),
                    format!(
                        "{} {}{}",
                        count(s.files()),
                        if s.bin.is_some() { "items" } else { "files" },
                        if s.incomplete() {
                            ", partly unreadable"
                        } else {
                            ""
                        }
                    ),
                ),
            };
            let enabled = scan
                .is_some_and(|s| s.files() > 0 || cat.action == Action::WindowsCleanup)
                && c.running.is_none();
            v.categories.push(CategoryView {
                id: cat.id.to_owned(),
                name: cat.name.to_owned(),
                safety: safety_text(cat.safety).to_owned(),
                safety_level: match cat.safety {
                    Safety::Safe => 0,
                    Safety::Caution => 1,
                    Safety::Review => 2,
                },
                size,
                files,
                selected: c.selected.contains(cat.id),
                enabled,
                focused: c.focused.as_deref() == Some(cat.id),
            });
        }
        if let Some(id) = &c.focused
            && let Some(cat) = WINDOWS_CATEGORIES.iter().find(|x| x.id == id)
        {
            let mut d = format!(
                "{}\n\n{}\n\nAfter cleaning: {}\nRegenerates: {}\nHow: {}",
                cat.name,
                cat.description,
                cat.consequence,
                match cat.regenerates {
                    Regenerates::Yes => "yes, automatically",
                    Regenerates::Partly => "partly",
                    Regenerates::No => "no",
                },
                match cat.action {
                    Action::Delete => "deleted permanently (not to the Recycle Bin)",
                    Action::Recycle => "moved to the Recycle Bin",
                    Action::EmptyRecycleBin => "the Recycle Bin is emptied",
                    Action::WindowsCleanup => "Windows Disk Cleanup is opened",
                }
            );
            if cat.needs_admin && !self.platform.elevated {
                d.push_str("\nNeeds administrator rights: Windows asks for them in Disk Cleanup.");
            }
            if let Some(s) = c.scans.get(cat.id) {
                let found: Vec<&str> = s.roots.iter().map(|r| r.path.as_str()).collect();
                if !found.is_empty() {
                    d.push_str(&format!("\n\nLocations:\n{}", found.join("\n")));
                }
                for r in s.roots.iter().filter(|r| r.error.is_some()) {
                    if let Some(e) = &r.error {
                        d.push_str(&format!("\nNot readable ({}): {}", e.id(), r.path));
                    }
                }
                if s.too_new > 0 {
                    d.push_str(&format!(
                        "\n{} recent files left alone (they may be in use).",
                        count(s.too_new)
                    ));
                }
                if s.skipped_links > 0 {
                    d.push_str(&format!(
                        "\n{} links skipped (never followed).",
                        count(s.skipped_links)
                    ));
                }
                if s.unreadable > 0 {
                    d.push_str(&format!(
                        "\n{} entries could not be read.",
                        count(s.unreadable)
                    ));
                }
                if cat.safety == Safety::Review {
                    v.per_file = true;
                    let chosen = c.files.get(cat.id);
                    v.candidates = s
                        .candidates
                        .iter()
                        .enumerate()
                        .map(|(i, f)| CandidateView {
                            path: f.path.clone(),
                            size: bytes(f.allocated),
                            modified: if f.mtime > 0 {
                                date_time(Millis(f.mtime as u64 * 1000), self.utc_offset_min)
                            } else {
                                String::new()
                            },
                            selected: chosen.is_some_and(|ix| ix.contains(&i)),
                        })
                        .collect();
                }
            }
            v.detail = d;
        }
        let plan = self.plan();
        v.can_clean = !plan.entries.is_empty() && !v.busy;
        v.status = if let Some(p) = c.running {
            v.progress = if p.total == 0 {
                0.0
            } else {
                p.done as f32 / p.total as f32
            };
            format!("Cleaning: {} of {}", count(p.done), count(p.total))
        } else if c.discovering {
            format!(
                "Examining categories ({} of {}); nothing is changed",
                c.scans.len(),
                WINDOWS_CATEGORIES.len()
            )
        } else if c.scans.is_empty() {
            "Press Examine to measure each category. Examining changes nothing.".into()
        } else {
            format!(
                "Selected: {} estimated in {} categories{}",
                bytes(plan.estimate()),
                plan.entries.len(),
                if plan.to_recycle() > 0 {
                    format!(", {} to the Recycle Bin", bytes(plan.to_recycle()))
                } else {
                    String::new()
                }
            )
        };
        if let Some(r) = &c.report {
            v.report = report_text(r);
        }
        v.history = self
            .history
            .iter()
            .rev()
            .map(|r| {
                format!(
                    "{}  deleted {}, recycled {}, free space change {}{}",
                    date_time(Millis(r.started_ms), self.utc_offset_min),
                    bytes(r.deleted_bytes()),
                    bytes(r.recycled_bytes()),
                    signed(r.measured()),
                    if r.cancelled { " (cancelled)" } else { "" }
                )
            })
            .collect();
        v
    }

    // ---- Duplicates ----------------------------------------------------------------

    /// Finds duplicates in the scanned (or zoomed) subtree.
    pub fn find_duplicates(&mut self) -> StorageDirty {
        if !self.has_index || self.scan.is_some() || self.dupes.cancel.is_some() {
            return StorageDirty::default();
        }
        let min = self.settings.duplicate_min_kib.saturating_mul(1024).max(1);
        let files = files_from_tree(&read(&self.tree), self.zoom, min);
        let cancel = Arc::new(AtomicBool::new(false));
        self.dupes.cancel = Some(cancel.clone());
        self.dupes.progress = Some(DupProgress::default());
        self.dupes.result = None;
        self.dupes.marked.clear();
        self.dupes.message.clear();
        let fs = self.platform.fs.clone();
        let notify = self.notify.clone();
        let opts = DupOptions {
            min_size: min,
            readers: 2,
        };
        let spawned = std::thread::Builder::new()
            .name("reclaim-dupes".into())
            .spawn(move || {
                let n2 = notify.clone();
                let r =
                    reclaim_storage_engine::dupes::find(&*fs, files, &opts, &cancel, &move |p| {
                        n2(StorageEvent::DupProgress(p));
                    });
                notify(StorageEvent::DupDone(r));
            });
        if spawned.is_err() {
            self.dupes.cancel = None;
            self.dupes.progress = None;
        }
        StorageDirty {
            dupes: true,
            ..StorageDirty::default()
        }
    }

    /// Stops the duplicate search; unfinished groups stay "probable".
    pub fn cancel_duplicates(&mut self) -> StorageDirty {
        if let Some(c) = &self.dupes.cancel {
            c.store(true, Ordering::Relaxed);
        }
        StorageDirty {
            dupes: true,
            ..StorageDirty::default()
        }
    }

    /// Marks or unmarks a file. Marking the last unmarked copy of a group is refused.
    pub fn toggle_duplicate(&mut self, group: usize, file: usize) -> StorageDirty {
        let Some(r) = &self.dupes.result else {
            return StorageDirty::default();
        };
        let Some(g) = r.groups.get(group) else {
            return StorageDirty::default();
        };
        if file >= g.files.len() {
            return StorageDirty::default();
        }
        if !self.dupes.marked.remove(&(group, file)) {
            let marked_in_group = self
                .dupes
                .marked
                .iter()
                .filter(|(gi, _)| *gi == group)
                .count();
            if marked_in_group + 1 >= g.files.len() {
                self.dupes.message = "One copy of every group always stays.".into();
            } else {
                self.dupes.marked.insert((group, file));
                self.dupes.message.clear();
            }
        }
        StorageDirty {
            dupes: true,
            ..StorageDirty::default()
        }
    }

    /// Asks to recycle the marked duplicates.
    pub fn request_recycle_duplicates(&mut self) -> StorageDirty {
        let Some(r) = &self.dupes.result else {
            return StorageDirty::default();
        };
        let sel: Vec<(usize, usize)> = self.dupes.marked.iter().copied().collect();
        match validate_selection(&r.groups, &sel) {
            Ok(paths) if !paths.is_empty() => {
                let size: u64 = sel
                    .iter()
                    .filter_map(|&(g, f)| r.groups.get(g).and_then(|g| g.files.get(f)))
                    .map(|f| f.allocated)
                    .sum();
                let probable = sel.iter().any(|&(g, _)| {
                    r.groups
                        .get(g)
                        .is_some_and(|g| g.confidence == Confidence::Probable)
                });
                let files: Vec<DupFile> = sel
                    .iter()
                    .filter_map(|&(g, f)| r.groups.get(g).and_then(|g| g.files.get(f)))
                    .cloned()
                    .collect();
                self.pending = Some((
                    Pending::RecycleDupes(files),
                    ConfirmView {
                        title: format!("Move {} duplicate files to the Recycle Bin?", paths.len()),
                        body: format!(
                            "{}, one copy of each group stays.{}\n\nThe files go to the Recycle Bin, so you can restore them; space is freed when the bin is emptied.",
                            bytes(size),
                            if probable {
                                "\nSome groups are only probable duplicates (the comparison was stopped before reading whole files)."
                            } else {
                                ""
                            }
                        ),
                        ok: "Move to Recycle Bin".into(),
                        danger: false,
                    },
                ));
                StorageDirty {
                    confirm: true,
                    ..StorageDirty::default()
                }
            }
            Ok(_) => StorageDirty::default(),
            Err(SelectionError::WouldRemoveAllCopies { .. }) => {
                self.dupes.message = "One copy of every group always stays.".into();
                StorageDirty {
                    dupes: true,
                    ..StorageDirty::default()
                }
            }
            Err(SelectionError::Unknown) => StorageDirty::default(),
        }
    }

    /// Recycles duplicates with the identity and last write time seen when they were
    /// compared: a file replaced or rewritten since then is skipped, not recycled.
    fn recycle_duplicates(&mut self, files: Vec<DupFile>) {
        self.removing = true;
        let remover = self.platform.remover.clone();
        let guard = self.platform.guard.clone();
        let tree = self.tree.clone();
        let notify = self.notify.clone();
        let basis = self.basis();
        let _ = std::thread::Builder::new()
            .name("reclaim-storage-recycle".into())
            .spawn(move || {
                let mut outcomes = Vec::new();
                let mut gone = Vec::new();
                for f in files {
                    let target = Target {
                        path: f.path.clone(),
                        root: None,
                        file_id: f.identity.map(|(_, id)| id),
                        mtime: Some(f.mtime),
                        allocated: f.allocated,
                        is_dir: false,
                    };
                    let o = remover.recycle(&target, &guard);
                    if matches!(o, ItemOutcome::Recycled(_)) {
                        gone.push(f.path.clone());
                    }
                    outcomes.push((f.path, o));
                }
                let mut t = write(&tree);
                for p in gone {
                    if let Some(i) = t.find(&p) {
                        t.remove(i);
                    }
                }
                let analysis = Analysis::of(&t, basis);
                drop(t);
                notify(StorageEvent::RemovalDone { outcomes, analysis });
            });
    }

    /// Path of a file in the duplicate list.
    pub fn duplicate_path(&self, group: usize, file: usize) -> Option<String> {
        let r = self.dupes.result.as_ref()?;
        r.groups.get(group)?.files.get(file).map(|f| f.path.clone())
    }

    /// Duplicates screen.
    pub fn dupes_view(&self) -> DupesView {
        let d = &self.dupes;
        let mut v = DupesView {
            busy: d.cancel.is_some(),
            can_find: self.has_index && self.scan.is_none() && d.cancel.is_none(),
            can_recycle: !d.marked.is_empty() && !self.removing,
            ..DupesView::default()
        };
        if let Some(p) = d.progress {
            v.progress = if p.files_total == 0 {
                0.0
            } else {
                p.files_done as f32 / p.files_total as f32
            };
            v.status = format!(
                "{}: {} of {} files, {} read",
                match p.stage {
                    DupStage::Grouping => "Grouping by size",
                    DupStage::Prefix => "Comparing file starts",
                    DupStage::Full => "Comparing whole files",
                    DupStage::Done => "Finishing",
                },
                count(p.files_done),
                count(p.files_total),
                bytes(p.bytes_read)
            );
        } else if let Some(r) = &d.result {
            v.status = format!(
                "{} groups, {} could be freed by keeping one copy each{}{}{}",
                count(r.groups.len() as u64),
                bytes(r.savings()),
                if r.cancelled {
                    "; stopped early, some groups are only probable"
                } else {
                    ""
                },
                if r.errors.is_empty() {
                    String::new()
                } else {
                    format!("; {} files could not be read", count(r.errors.len() as u64))
                },
                if d.message.is_empty() {
                    String::new()
                } else {
                    format!(". {}", d.message)
                }
            );
            for (gi, g) in r.groups.iter().enumerate() {
                v.rows.push(DupRowView {
                    group: gi,
                    file: None,
                    text: format!(
                        "{} copies of {} ({})",
                        g.files.len(),
                        bytes(g.size),
                        group_confidence(g)
                    ),
                    size: format!("{} reclaimable", bytes(g.savings())),
                    marked: false,
                    header: true,
                });
                for (fi, f) in g.files.iter().enumerate() {
                    v.rows.push(DupRowView {
                        group: gi,
                        file: Some(fi),
                        text: f.path.clone(),
                        size: if f.mtime > 0 {
                            date_time(Millis(f.mtime as u64 * 1000), self.utc_offset_min)
                        } else {
                            String::new()
                        },
                        marked: d.marked.contains(&(gi, fi)),
                        header: false,
                    });
                }
            }
        } else if !self.has_index {
            v.status = "Scan a drive or folder first, then search it for duplicates.".into();
        } else {
            v.status = format!(
                "Searches the current map area for identical files of at least {} KiB. Nothing is removed without your confirmation, and one copy always stays.",
                self.settings.duplicate_min_kib
            );
        }
        v
    }

    // ---- Export --------------------------------------------------------------------

    /// Writes the index to `path` as JSON or CSV on a background thread.
    pub fn export(&mut self, path: String, csv: bool) -> StorageDirty {
        if !self.has_index || self.scan.is_some() {
            return StorageDirty::default();
        }
        self.message = "Exporting…".into();
        let tree = self.tree.clone();
        let summary = self.summary.clone();
        let notify = self.notify.clone();
        let now = (self.platform.clock)();
        let _ = std::thread::Builder::new()
            .name("reclaim-storage-export".into())
            .spawn(move || {
                let r = (|| -> std::io::Result<()> {
                    let f = std::fs::File::create(&path)?;
                    let mut w = std::io::BufWriter::new(f);
                    let t = read(&tree);
                    let o = ExportOptions::default();
                    if csv {
                        export::csv(&t, summary.as_ref(), reclaim_core::VERSION, now, &o, &mut w)?;
                    } else {
                        export::json(&t, summary.as_ref(), reclaim_core::VERSION, now, &o, &mut w)?;
                    }
                    std::io::Write::flush(&mut w)
                })();
                notify(StorageEvent::ExportDone(
                    r.map(|()| path).map_err(|e| e.to_string()),
                ));
            });
        StorageDirty {
            status: true,
            ..StorageDirty::default()
        }
    }

    // ---- Settings ------------------------------------------------------------------

    /// Settings form.
    pub fn settings_form(&self) -> StorageForm {
        StorageForm::of(&self.settings)
    }

    /// Saves the settings form.
    pub fn save_settings(&mut self, form: &StorageForm) -> Result<StorageDirty, String> {
        let s = form.parse(&self.settings)?;
        if let Some(st) = &self.platform.store {
            st.save(&s)
                .map_err(|e| format!("Could not save storage settings: {e}"))?;
        }
        let basis_changed = s.allocated_sizes != self.settings.allocated_sizes;
        self.settings = s;
        if basis_changed && self.has_index && self.scan.is_none() {
            self.analysis = Analysis::of(&read(&self.tree), self.basis());
        }
        let mut d = StorageDirty::index();
        d.settings = true;
        Ok(d)
    }

    /// Whether the process could use the fast scan (shown in settings).
    pub fn fast_scan_available(&self) -> bool {
        self.platform.fast_scan.is_some() && self.platform.elevated
    }
}

fn group_confidence(g: &DupGroup) -> &'static str {
    match g.confidence {
        Confidence::Confirmed => "identical content",
        Confidence::Probable => "probable: same size and start",
    }
}

fn signed(v: i128) -> String {
    if v < 0 {
        format!(
            "-{}",
            bytes(v.unsigned_abs().min(u128::from(u64::MAX)) as u64)
        )
    } else {
        format!("+{}", bytes(v.min(i128::from(u64::MAX)) as u64))
    }
}

/// Plain-language report of a cleanup run.
pub fn report_text(r: &CleanupReport) -> String {
    let mut s = format!(
        "{}Deleted {}, moved {} to the Recycle Bin (estimate was {}).",
        if r.cancelled { "Cancelled. " } else { "" },
        bytes(r.deleted_bytes()),
        bytes(r.recycled_bytes()),
        bytes(r.estimated)
    );
    for v in &r.volumes {
        s.push_str(&format!(
            "\nFree space on {}: {} → {} ({})",
            v.volume,
            bytes(v.free_before),
            bytes(v.free_after),
            signed(v.change())
        ));
    }
    if r.windows_cleanup_opened {
        s.push_str(
            "\nWindows Disk Cleanup was opened for system files; its results are not included.",
        );
    }
    for c in &r.categories {
        let name = WINDOWS_CATEGORIES
            .iter()
            .find(|x| x.id == c.id)
            .map_or(c.id.as_str(), |x| x.name);
        s.push_str(&format!(
            "\n{name}: {} deleted, {} recycled, {} skipped, {} failed",
            count(c.deleted),
            count(c.recycled),
            count(c.skipped),
            count(c.failed)
        ));
        for n in c.notes.iter().take(5) {
            s.push_str(&format!("\n    {n}"));
        }
    }
    s.push_str("\nFree space measured by Windows can differ from the estimate: other programs write too, and hard-linked or compressed files free less.");
    s
}

fn same_root(a: &str, b: &str) -> bool {
    let n = |s: &str| s.trim_end_matches(['\\', '/']).to_lowercase();
    n(a) == n(b)
}

fn sorted_children(t: &Tree, id: NodeId, basis: SizeBasis) -> Vec<NodeId> {
    let mut kids: Vec<NodeId> = t.children(id).collect();
    kids.sort_by(|&a, &b| {
        t.node(b)
            .size(basis)
            .cmp(&t.node(a).size(basis))
            .then_with(|| t.name(a).cmp(t.name(b)))
    });
    kids
}

fn nearest_dir(t: &Tree, path: &str) -> Option<NodeId> {
    let mut p = path.trim_end_matches(['\\', '/']).to_owned();
    loop {
        if let Some(id) = t.find(&p)
            && t.node(id).live()
        {
            return Some(if t.node(id).kind == Kind::Dir {
                id
            } else {
                t.node(id).parent
            });
        }
        let i = p.rfind(['\\', '/'])?;
        p.truncate(i);
        if p.is_empty() || p.ends_with(':') {
            return None;
        }
    }
}

fn common_ancestor(t: &Tree, ids: &[NodeId]) -> Option<NodeId> {
    let first = *ids.first()?;
    let mut chain: Vec<NodeId> = t.ancestors(first).collect();
    for &id in &ids[1..] {
        let anc: BTreeSet<NodeId> = t.ancestors(id).collect();
        chain.retain(|a| anc.contains(a));
    }
    chain.into_iter().find(|&a| a != TOP && t.node(a).is_dir())
}
