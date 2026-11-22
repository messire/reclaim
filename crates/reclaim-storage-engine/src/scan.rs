//! Parallel, cancellable, pausable directory scanner.
//!
//! Threads exist only while a scan runs: one control thread that reports progress
//! and `threads` workers inside a `std::thread::scope`. Workers take directories from
//! a shared stack (depth first keeps it small), list them without holding any lock,
//! then insert the listing into the index under a short write lock. When the scan
//! ends every thread has exited; nothing stays resident (STORAGE-ARCHITECTURE §5).
//!
//! Honesty rules: a directory that could not be read is marked `ERROR` and counted;
//! directories still queued when the scan is cancelled are marked `INCOMPLETE`; the
//! summary's state says `Complete` only if neither happened.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use reclaim_storage_core::entry::{Attrs, EntryMeta, ErrorKind, Kind};
use reclaim_storage_core::path::{NormPath, PathStyle, glob_match};
use reclaim_storage_core::ports::{DriveKind, Fs};
use reclaim_storage_core::tree::{NodeId, Tree};

/// Scan settings.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Worker threads (at least 1).
    pub threads: usize,
    /// Exclusion patterns: a pattern with a path separator excludes that folder (and
    /// what is below it); otherwise it is a name glob (`node_modules`, `*.vhdx`).
    pub exclude: Vec<String>,
    /// Do not descend into another volume mounted below a root (POSIX mounts; on
    /// Windows mount points are reparse points and never followed anyway).
    pub stay_on_volume: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            threads: 4,
            exclude: Vec::new(),
            stay_on_volume: true,
        }
    }
}

/// Worker count for a drive: one for rotational disks (parallel listing makes a disk
/// head seek between directories), four for SSD/NVMe and unknown, eight for network
/// shares (latency-bound). See docs/storage/BENCHMARKS.md for the measurements.
pub fn default_threads(kind: DriveKind, seek_penalty: Option<bool>) -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    match (kind, seek_penalty) {
        (DriveKind::Network, _) => 8,
        (_, Some(true)) | (DriveKind::Optical, _) => 1,
        _ => cores.clamp(1, 4),
    }
}

/// Where a scan is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    /// Listing directories.
    Running,
    /// Paused by the user.
    Paused,
    /// Every directory was listed (some may still have been unreadable: see errors).
    Complete,
    /// Stopped by the user; the result is partial.
    Cancelled,
}

/// Live counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanProgress {
    /// Files seen.
    pub files: u64,
    /// Directories listed.
    pub dirs: u64,
    /// Allocated bytes seen (before hard-link deduplication).
    pub bytes: u64,
    /// Unreadable directories.
    pub errors: u64,
    /// Elapsed time, pauses excluded.
    pub elapsed_ms: u64,
    /// Paused right now.
    pub paused: bool,
}

/// Result of a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanSummary {
    /// Final state (`Complete` or `Cancelled`).
    pub state: ScanState,
    /// Counters at the end.
    pub progress: ScanProgress,
    /// Directories never listed because of cancellation.
    pub incomplete_dirs: u64,
    /// Hard-link names counted once.
    pub hardlink_dups: usize,
    /// The index has unreadable or unlisted directories: totals are lower bounds.
    pub partial: bool,
}

#[derive(Debug)]
struct Job {
    node: NodeId,
    path: String,
    volume: Option<u64>,
}

#[derive(Debug, Default)]
struct Queue {
    jobs: Vec<Job>,
    active: usize,
}

/// Shared control of a running scan.
#[derive(Debug, Default)]
pub struct ScanControl {
    cancel: AtomicBool,
    paused: Mutex<bool>,
    resume: Condvar,
    files: AtomicU64,
    dirs: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
}

impl ScanControl {
    /// Requests cancellation (workers finish the directory they are listing).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.resume.notify_all();
    }

    /// Whether cancellation was requested.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Pauses or resumes.
    pub fn set_paused(&self, on: bool) {
        if let Ok(mut p) = self.paused.lock() {
            *p = on;
        }
        self.resume.notify_all();
    }

    /// Whether paused.
    pub fn is_paused(&self) -> bool {
        self.paused.lock().map(|p| *p).unwrap_or(false)
    }

    fn wait_while_paused(&self) {
        let Ok(mut p) = self.paused.lock() else {
            return;
        };
        while *p && !self.cancelled() {
            match self.resume.wait_timeout(p, Duration::from_millis(200)) {
                Ok((g, _)) => p = g,
                Err(_) => return,
            }
        }
    }
}

/// Compiled exclusion rules.
#[derive(Debug, Clone, Default)]
pub struct Exclusions {
    names: Vec<String>,
    paths: Vec<NormPath>,
    case_insensitive: bool,
}

impl Exclusions {
    /// Compiles patterns for a path style.
    pub fn new(patterns: &[String], style: PathStyle) -> Self {
        let mut e = Self {
            case_insensitive: style == PathStyle::Windows,
            ..Self::default()
        };
        for p in patterns.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
            if p.contains('\\') || p.contains('/') {
                if let Ok(n) = NormPath::new(p, style) {
                    e.paths.push(n);
                }
            } else {
                e.names.push(p.to_owned());
            }
        }
        e
    }

    /// Whether nothing is excluded.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.paths.is_empty()
    }

    /// Whether an entry (by name, and by full path when path rules exist) is excluded.
    pub fn matches(
        &self,
        name: &str,
        full_path: impl FnOnce() -> String,
        style: PathStyle,
    ) -> bool {
        if self
            .names
            .iter()
            .any(|g| glob_match(g, name, self.case_insensitive))
        {
            return true;
        }
        if self.paths.is_empty() {
            return false;
        }
        match NormPath::new(&full_path(), style) {
            Ok(p) => self.paths.iter().any(|x| p.starts_with(x)),
            Err(_) => false,
        }
    }
}

/// A running scan.
#[derive(Debug)]
pub struct ScanHandle {
    control: Arc<ScanControl>,
    thread: Option<JoinHandle<ScanSummary>>,
}

impl ScanHandle {
    /// Shared control (cancel, pause).
    pub fn control(&self) -> &Arc<ScanControl> {
        &self.control
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.control.cancel();
    }

    /// Pauses or resumes.
    pub fn set_paused(&self, on: bool) {
        self.control.set_paused(on);
    }

    /// Whether the scan thread has finished.
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits for the end.
    pub fn join(mut self) -> Option<ScanSummary> {
        self.thread.take().and_then(|t| t.join().ok())
    }
}

impl Drop for ScanHandle {
    fn drop(&mut self) {
        // Dropping a handle stops the scan; its threads never outlive the owner.
        if let Some(t) = self.thread.take() {
            self.control.cancel();
            let _ = t.join();
        }
    }
}

/// What to scan.
#[derive(Debug, Clone)]
pub enum ScanTarget {
    /// New roots (the index must be empty).
    Roots(Vec<String>),
    /// Re-list one directory of an existing index (refresh).
    Refresh(NodeId),
}

/// Progress callback (called about every 200 ms from the scan's control thread).
pub type OnProgress = Box<dyn Fn(ScanProgress) + Send>;

/// Starts a scan on a new thread.
pub fn start(
    fs: Arc<dyn Fs>,
    tree: Arc<RwLock<Tree>>,
    target: ScanTarget,
    options: ScanOptions,
    on_progress: OnProgress,
) -> ScanHandle {
    let control = Arc::new(ScanControl::default());
    let c = control.clone();
    let thread = std::thread::Builder::new()
        .name("reclaim-scan".into())
        .spawn(move || run(&*fs, &tree, target, &options, &c, &*on_progress))
        .ok();
    ScanHandle { control, thread }
}

/// Runs a scan on the calling thread (CLI, tests, benchmarks).
pub fn scan_blocking(
    fs: &dyn Fs,
    tree: &Arc<RwLock<Tree>>,
    target: ScanTarget,
    options: &ScanOptions,
    control: &ScanControl,
) -> ScanSummary {
    run(fs, tree, target, options, control, &|_| {})
}

fn write(tree: &RwLock<Tree>) -> std::sync::RwLockWriteGuard<'_, Tree> {
    tree.write().unwrap_or_else(|p| p.into_inner())
}

fn initial_jobs(
    fs: &dyn Fs,
    tree: &RwLock<Tree>,
    target: ScanTarget,
    control: &ScanControl,
) -> Vec<Job> {
    let mut jobs = Vec::new();
    match target {
        ScanTarget::Roots(paths) => {
            let mut metas = Vec::new();
            let mut failed = Vec::new();
            for p in &paths {
                match fs.stat(p) {
                    Ok(mut m) => {
                        m.name = p.clone();
                        metas.push(m);
                    }
                    Err(k) => {
                        let mut m = EntryMeta::dir(p);
                        m.allocated = Some(0);
                        failed.push((metas.len(), k));
                        metas.push(m);
                    }
                }
            }
            let mut t = write(tree);
            let ids = t.set_roots(&metas);
            for (i, k) in failed {
                t.mark_error(ids[i], k);
                control.errors.fetch_add(1, Ordering::Relaxed);
            }
            for (i, m) in metas.iter().enumerate() {
                match m.kind {
                    Kind::Dir if !t.node(ids[i]).attrs.has(Attrs::ERROR) => jobs.push(Job {
                        node: ids[i],
                        path: paths[i].clone(),
                        volume: m.volume,
                    }),
                    Kind::File => {
                        control.files.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
        }
        ScanTarget::Refresh(node) => {
            let mut t = write(tree);
            if t.get(node).is_some_and(|n| n.is_dir()) {
                let path = t.path(node);
                t.clear_dir(node);
                let volume = fs.stat(&path).ok().and_then(|m| m.volume);
                jobs.push(Job { node, path, volume });
            }
        }
    }
    jobs
}

fn run(
    fs: &dyn Fs,
    tree: &RwLock<Tree>,
    target: ScanTarget,
    options: &ScanOptions,
    control: &ScanControl,
    on_progress: &dyn Fn(ScanProgress),
) -> ScanSummary {
    let style = fs.style();
    let excl = Exclusions::new(&options.exclude, style);
    let queue = Mutex::new(Queue {
        jobs: initial_jobs(fs, tree, target, control),
        active: 0,
    });
    let wake = Condvar::new();
    let started = Instant::now();
    let paused_total = Mutex::new(Duration::ZERO);
    let threads = options.threads.max(1);

    let progress = |paused_for: Duration| ScanProgress {
        files: control.files.load(Ordering::Relaxed),
        dirs: control.dirs.load(Ordering::Relaxed),
        bytes: control.bytes.load(Ordering::Relaxed),
        errors: control.errors.load(Ordering::Relaxed),
        elapsed_ms: started.elapsed().saturating_sub(paused_for).as_millis() as u64,
        paused: control.is_paused(),
    };

    let exited = AtomicU64::new(0);
    let finished = Condvar::new();
    std::thread::scope(|s| {
        let mut spawned = 0u64;
        for i in 0..threads {
            let (queue, wake, excl, exited, finished) = (&queue, &wake, &excl, &exited, &finished);
            let ok = std::thread::Builder::new()
                .name(format!("reclaim-scan-{i}"))
                .spawn_scoped(s, move || {
                    worker(fs, tree, options, control, queue, wake, excl, style);
                    exited.fetch_add(1, Ordering::SeqCst);
                    finished.notify_all();
                })
                .is_ok();
            spawned += u64::from(ok);
        }
        if spawned == 0 {
            // No thread could be started: list on this one.
            worker(fs, tree, options, control, &queue, &wake, &excl, style);
        }
        // Progress loop: wake every 200 ms (or when a worker exits) until all are done.
        let mut pause_started: Option<Instant> = None;
        loop {
            let done = {
                let q = queue.lock().unwrap_or_else(|p| p.into_inner());
                let _ = finished
                    .wait_timeout(q, Duration::from_millis(200))
                    .unwrap_or_else(|p| p.into_inner());
                exited.load(Ordering::SeqCst) >= spawned
            };
            match (control.is_paused(), pause_started) {
                (true, None) => pause_started = Some(Instant::now()),
                (false, Some(p)) => {
                    if let Ok(mut t) = paused_total.lock() {
                        *t += p.elapsed();
                    }
                    pause_started = None;
                }
                _ => {}
            }
            let paused_for = paused_total.lock().map(|t| *t).unwrap_or_default()
                + pause_started.map_or(Duration::ZERO, |p| p.elapsed());
            if done {
                break;
            }
            on_progress(progress(paused_for));
        }
    });

    // Cancelled: whatever is still queued was never listed.
    let leftover = std::mem::take(&mut queue.lock().unwrap_or_else(|p| p.into_inner()).jobs);
    let mut t = write(tree);
    for j in &leftover {
        t.mark(j.node, Attrs::INCOMPLETE);
    }
    t.resolve_hardlinks();
    let partial = t.has_gaps();
    let hardlink_dups = t.hardlink_duplicates();
    drop(t);
    let paused_for = paused_total.lock().map(|t| *t).unwrap_or_default();
    let p = progress(paused_for);
    on_progress(ScanProgress { paused: false, ..p });
    ScanSummary {
        state: if control.cancelled() {
            ScanState::Cancelled
        } else {
            ScanState::Complete
        },
        progress: ScanProgress { paused: false, ..p },
        incomplete_dirs: leftover.len() as u64,
        hardlink_dups,
        partial,
    }
}

#[allow(clippy::too_many_arguments)] // Shared scan state, borrowed for the scope.
fn worker(
    fs: &dyn Fs,
    tree: &RwLock<Tree>,
    options: &ScanOptions,
    control: &ScanControl,
    queue: &Mutex<Queue>,
    wake: &Condvar,
    excl: &Exclusions,
    style: PathStyle,
) {
    let mut entries = Vec::with_capacity(256);
    loop {
        control.wait_while_paused();
        let job = {
            let mut q = queue.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if control.cancelled() {
                    break None;
                }
                if let Some(j) = q.jobs.pop() {
                    q.active += 1;
                    break Some(j);
                }
                if q.active == 0 {
                    break None;
                }
                q = wake
                    .wait_timeout(q, Duration::from_millis(100))
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
            }
        };
        let Some(job) = job else {
            wake.notify_all();
            return;
        };
        entries.clear();
        let listed = fs.read_dir(&job.path, &mut entries);
        if listed.is_ok() && !excl.is_empty() {
            // Excluded files are left out entirely: not counted, exported or hashed.
            // Excluded directories stay, flagged, so the user sees what was skipped.
            entries.retain(|e| {
                e.kind == Kind::Dir || !excl.matches(&e.name, || fs.join(&job.path, &e.name), style)
            });
        }
        let mut next = Vec::new();
        {
            let mut t = write(tree);
            match listed {
                Ok(()) => {
                    let ids = t.fill_dir(job.node, &entries);
                    let mut files = 0u64;
                    let mut bytes = 0u64;
                    for (e, id) in entries.iter().zip(ids) {
                        match e.kind {
                            Kind::Dir => {
                                let path = fs.join(&job.path, &e.name);
                                if !excl.is_empty() && excl.matches(&e.name, || path.clone(), style)
                                {
                                    t.mark(id, Attrs::EXCLUDED);
                                } else if options.stay_on_volume
                                    && job.volume.is_some()
                                    && e.volume.is_some()
                                    && e.volume != job.volume
                                {
                                    t.mark(id, Attrs::OTHER_VOLUME);
                                } else {
                                    next.push(Job {
                                        node: id,
                                        path,
                                        volume: e.volume.or(job.volume),
                                    });
                                }
                            }
                            _ => {
                                files += 1;
                                bytes += e.allocated.unwrap_or(e.logical);
                            }
                        }
                    }
                    control.files.fetch_add(files, Ordering::Relaxed);
                    control.bytes.fetch_add(bytes, Ordering::Relaxed);
                    control.dirs.fetch_add(1, Ordering::Relaxed);
                }
                Err(kind) => {
                    // A directory that vanished is gone, not an access problem, but it
                    // is still recorded: the user should know the listing raced.
                    t.mark_error(job.node, kind);
                    if kind != ErrorKind::NotFound {
                        control.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        let mut q = queue.lock().unwrap_or_else(|p| p.into_inner());
        q.jobs.extend(next.into_iter().rev());
        q.active -= 1;
        drop(q);
        wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFs;
    use reclaim_storage_core::tree::TOP;

    fn tree() -> Arc<RwLock<Tree>> {
        Arc::new(RwLock::new(Tree::new(PathStyle::Posix)))
    }

    fn opts(threads: usize) -> ScanOptions {
        ScanOptions {
            threads,
            ..ScanOptions::default()
        }
    }

    #[test]
    fn scans_and_aggregates() {
        for threads in [1, 4] {
            let fs = FakeFs::sample();
            let t = tree();
            let s = scan_blocking(
                &fs,
                &t,
                ScanTarget::Roots(vec!["/r".into()]),
                &opts(threads),
                &ScanControl::default(),
            );
            assert_eq!(s.state, ScanState::Complete);
            assert!(!s.partial);
            let t = t.read().unwrap();
            assert_eq!(t.node(TOP).logical, 1 + 20 + 300 + 4000);
            assert_eq!(s.progress.files, 4);
            assert_eq!(s.progress.dirs, 3);
            assert_eq!(t.counts(TOP).files, 4);
        }
    }

    #[test]
    fn errors_are_reported_not_hidden() {
        let mut fs = FakeFs::sample();
        fs.deny("/r/b");
        let t = tree();
        let s = scan_blocking(
            &fs,
            &t,
            ScanTarget::Roots(vec!["/r".into(), "/missing".into()]),
            &opts(2),
            &ScanControl::default(),
        );
        assert_eq!(s.state, ScanState::Complete);
        assert!(s.partial);
        assert_eq!(s.progress.errors, 2);
        let t = t.read().unwrap();
        assert_eq!(t.errors().len(), 2);
        let b = t.find("/r/b").unwrap();
        assert!(t.node(b).attrs.has(Attrs::ERROR));
        assert_eq!(t.node(TOP).logical, 1 + 20);
    }

    #[test]
    fn vanished_directory_is_recorded() {
        let mut fs = FakeFs::sample();
        fs.vanish("/r/b");
        let t = tree();
        let s = scan_blocking(
            &fs,
            &t,
            ScanTarget::Roots(vec!["/r".into()]),
            &opts(1),
            &ScanControl::default(),
        );
        assert!(s.partial);
        assert_eq!(s.progress.errors, 0);
        assert_eq!(t.read().unwrap().errors()[0].kind, ErrorKind::NotFound);
    }

    #[test]
    fn cancel_gives_partial_result() {
        let fs = FakeFs::wide(50, 20);
        let t = tree();
        let control = ScanControl::default();
        control.cancel();
        let s = scan_blocking(
            &fs,
            &t,
            ScanTarget::Roots(vec!["/w".into()]),
            &opts(2),
            &control,
        );
        assert_eq!(s.state, ScanState::Cancelled);
        assert!(s.partial);
        assert_eq!(s.incomplete_dirs, 1);
        let t = t.read().unwrap();
        assert!(t.node(t.roots()[0]).attrs.has(Attrs::INCOMPLETE));
    }

    #[test]
    fn pause_resume_and_cancel_from_another_thread() {
        let fs: Arc<dyn Fs> = Arc::new(FakeFs::wide(200, 50).slow(Duration::from_millis(1)));
        let t = tree();
        let seen = Arc::new(AtomicU64::new(0));
        let s2 = seen.clone();
        let h = start(
            fs,
            t.clone(),
            ScanTarget::Roots(vec!["/w".into()]),
            opts(2),
            Box::new(move |p| s2.store(p.dirs, Ordering::SeqCst)),
        );
        std::thread::sleep(Duration::from_millis(30));
        h.set_paused(true);
        std::thread::sleep(Duration::from_millis(250));
        let at_pause = seen.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(250));
        // At most the directories in flight finish while paused.
        assert!(seen.load(Ordering::SeqCst) <= at_pause + 2);
        h.set_paused(false);
        std::thread::sleep(Duration::from_millis(30));
        h.cancel();
        let s = h.join().unwrap();
        assert_eq!(s.state, ScanState::Cancelled);
        assert!(s.progress.dirs >= at_pause);
        // The partial index is consistent: the top total equals the sum of files.
        let t = t.read().unwrap();
        let sum: u64 = t
            .descendants(TOP)
            .into_iter()
            .filter(|&i| !t.node(i).is_dir())
            .map(|i| t.node(i).logical)
            .sum();
        assert_eq!(t.node(TOP).logical, sum);
    }

    #[test]
    fn exclusions_and_links() {
        let mut fs = FakeFs::sample();
        fs.link("/r/ln");
        let t = tree();
        let o = ScanOptions {
            exclude: vec!["b".into()],
            ..opts(1)
        };
        scan_blocking(
            &fs,
            &t,
            ScanTarget::Roots(vec!["/r".into()]),
            &o,
            &ScanControl::default(),
        );
        let t = t.read().unwrap();
        let b = t.find("/r/b").unwrap();
        assert!(t.node(b).attrs.has(Attrs::EXCLUDED));
        let ln = t.find("/r/ln").unwrap();
        assert_eq!(t.node(ln).kind, Kind::Link);
        assert_eq!(t.node(TOP).logical, 21);

        let e = Exclusions::new(
            &["/r/b".into(), "*.tmp".into(), " ".into()],
            PathStyle::Posix,
        );
        assert!(e.matches("x.tmp", String::new, PathStyle::Posix));
        assert!(e.matches("c", || "/r/b/c".into(), PathStyle::Posix));
        assert!(!e.matches("c", || "/r/bb".into(), PathStyle::Posix));
        assert!(Exclusions::default().is_empty());
        let w = Exclusions::new(&["Node_Modules".into()], PathStyle::Windows);
        assert!(w.matches("node_modules", String::new, PathStyle::Windows));
    }

    #[test]
    fn file_exclusions_leave_files_out() {
        let mut fs = FakeFs::sample();
        fs.add_file("/r", "big.tmp", 9000);
        fs.add_file("/r/b", "keep.txt", 7);
        let t = tree();
        let o = ScanOptions {
            exclude: vec!["*.tmp".into(), "/r/b/c".into()],
            ..opts(1)
        };
        scan_blocking(
            &fs,
            &t,
            ScanTarget::Roots(vec!["/r".into()]),
            &o,
            &ScanControl::default(),
        );
        let t = t.read().unwrap();
        assert!(t.find("/r/big.tmp").is_none());
        assert!(t.find("/r/b/c").is_none(), "path rules apply to files too");
        assert!(t.find("/r/b/keep.txt").is_some());
        assert_eq!(t.node(TOP).logical, 1 + 20 + 4000 + 7);
    }

    #[test]
    fn refresh_replaces_subtree() {
        let mut fs = FakeFs::sample();
        let t = tree();
        scan_blocking(
            &fs,
            &t,
            ScanTarget::Roots(vec!["/r".into()]),
            &opts(2),
            &ScanControl::default(),
        );
        fs.add_file("/r/b", "new", 50_000);
        fs.remove("/r/b/c");
        let b = t.read().unwrap().find("/r/b").unwrap();
        let s = scan_blocking(
            &fs,
            &t,
            ScanTarget::Refresh(b),
            &opts(2),
            &ScanControl::default(),
        );
        assert_eq!(s.state, ScanState::Complete);
        let t = t.read().unwrap();
        assert_eq!(t.node(TOP).logical, 1 + 20 + 4000 + 50_000);
        assert!(t.find("/r/b/c").is_none());
        assert!(t.find("/r/b/new").is_some());
    }

    #[test]
    fn deep_trees_and_other_volumes() {
        let fs = FakeFs::deep(2_000);
        let t = tree();
        let s = scan_blocking(
            &fs,
            &t,
            ScanTarget::Roots(vec!["/d".into()]),
            &opts(3),
            &ScanControl::default(),
        );
        assert_eq!(s.progress.dirs, 2_001);
        assert_eq!(t.read().unwrap().node(TOP).logical, 2_001);

        let mut fs = FakeFs::sample();
        fs.set_volume("/r/b", 9);
        let t = tree();
        scan_blocking(
            &fs,
            &t,
            ScanTarget::Roots(vec!["/r".into()]),
            &opts(1),
            &ScanControl::default(),
        );
        let t = t.read().unwrap();
        assert!(
            t.node(t.find("/r/b").unwrap())
                .attrs
                .has(Attrs::OTHER_VOLUME)
        );
    }

    #[test]
    fn thread_heuristic() {
        assert_eq!(default_threads(DriveKind::Fixed, Some(true)), 1);
        assert_eq!(default_threads(DriveKind::Network, None), 8);
        assert!(default_threads(DriveKind::Fixed, Some(false)) >= 1);
        assert!(default_threads(DriveKind::Fixed, None) <= 4);
    }

    #[test]
    fn file_root_and_handle_drop() {
        let fs = FakeFs::sample();
        let t = tree();
        let s = scan_blocking(
            &fs,
            &t,
            ScanTarget::Roots(vec!["/r/a".into()]),
            &opts(1),
            &ScanControl::default(),
        );
        assert_eq!(s.progress.files, 1);
        assert_eq!(t.read().unwrap().node(TOP).logical, 1);
        let fs: Arc<dyn Fs> = Arc::new(FakeFs::wide(100, 10).slow(Duration::from_millis(2)));
        let h = start(
            fs,
            tree(),
            ScanTarget::Roots(vec!["/w".into()]),
            opts(2),
            Box::new(|_| {}),
        );
        assert!(!h.is_finished());
        assert!(!h.control().cancelled());
        drop(h); // Must stop and join, not hang.
    }
}
